//! UDP-Audiopakete: fester 32-Byte-Header + Opus-Payload.
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |    Magic (0x5254 = "RT")      |  Version (4)  |     Flags     |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                         Token (u64)                           |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                        Sequence (u32)                         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                       Timestamp (u32)                         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                Counter (u64, nonce of the AEAD)               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                        Stream ID (u32)                       |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |   Opus payload, ChaCha20-Poly1305 encrypted (+16 byte tag)    |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! Semantik des Token-Felds:
//! - **Client -> Server:** das beim Login vergebene `udp_token` (Registrierung
//!   der UDP-Adresse + Identifikation des Sprechers).
//! - **Server -> Client (Relay):** die `client_id` des Sprechers.

/// Magic-Bytes "RT" (RustTalk), Little-Endian.
pub const MAGIC: u16 = 0x5254;

/// Protokollversion des Audio-Headers (2 = encrypted, 3 = ping/pong, 4 = stream IDs).
pub const VERSION: u8 = 4;

/// Länge des Headers in Bytes.
pub const HEADER_LEN: usize = 32;

/// Sinnvolle Obergrenze für ein UDP-Paket im LAN (unterhalb der MTU).
pub const MAX_PACKET_LEN: usize = 1500;

/// Opus-Samplerate in Hz (fest verdrahtet im ganzen System).
pub const SAMPLE_RATE: u32 = 48_000;

/// Header flag: this packet is computer audio of a screen share (not a voice).
pub const FLAG_SCREEN_AUDIO: u8 = 0x01;

/// Header flag: sealed control payload ([`Control`]) instead of Opus.
/// Ping: client -> server, `sequence` = ping id. Pong: server -> client,
/// token 0, `sequence` = the ping id it answers.
pub const FLAG_CONTROL: u8 = 0x80;

/// Plaintext length of every control payload (ping is padded, so a pong is
/// never larger than the ping that caused it).
pub const CONTROL_LEN: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Ping,
    /// Server counters of this session (both wrap): audio packets it accepted
    /// from the client, and audio packets it relayed to the client.
    Pong { audio_received: u32, audio_relayed: u32 },
}

impl Control {
    const PING: u8 = 1;
    const PONG: u8 = 2;

    pub fn encode(&self) -> [u8; CONTROL_LEN] {
        let mut out = [0u8; CONTROL_LEN];
        match *self {
            Control::Ping => out[0] = Self::PING,
            Control::Pong { audio_received, audio_relayed } => {
                out[0] = Self::PONG;
                out[1..5].copy_from_slice(&audio_received.to_le_bytes());
                out[5..9].copy_from_slice(&audio_relayed.to_le_bytes());
            }
        }
        out
    }

    /// `None` for a wrong length, unknown kind or non-zero ping padding.
    pub fn decode(payload: &[u8]) -> Option<Self> {
        let p: &[u8; CONTROL_LEN] = payload.try_into().ok()?;
        match p[0] {
            Self::PING if p[1..].iter().all(|&b| b == 0) => Some(Control::Ping),
            Self::PONG => Some(Control::Pong {
                audio_received: u32::from_le_bytes(p[1..5].try_into().ok()?),
                audio_relayed: u32::from_le_bytes(p[5..9].try_into().ok()?),
            }),
            _ => None,
        }
    }
}

/// Samples pro Opus-Frame (20 ms @ 48 kHz).
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    pub flags: u8,
    pub stream_id: u32,
    pub token: u64,
    pub sequence: u32,
    /// Beliebiger, pro Sender monoton steigender Zeitstempel
    /// (Samples seit Sendebeginn); dient der Jitter-Erkennung.
    pub timestamp: u32,
    /// Per-key unique counter (nonce); set by [`seal_packet`].
    pub counter: u64,
}

impl AudioHeader {
    pub fn new(token: u64, sequence: u32, timestamp: u32) -> Self {
        Self { flags: 0, stream_id: 0, token, sequence, timestamp, counter: 0 }
    }
}

/// Builds an encrypted packet: header (with a fresh counter) + sealed payload.
pub fn seal_packet(header: &AudioHeader, cipher: &crate::secure::PacketCipher, direction: u8, payload: &[u8]) -> Vec<u8> {
    let mut h = *header;
    h.counter = cipher.next_counter();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + crate::secure::TAG_LEN);
    encode(&h, &[], &mut out);
    let sealed = cipher.seal(direction, h.counter, &out[..HEADER_LEN], payload);
    out.extend_from_slice(&sealed);
    out
}

/// Opens a packet decoded with [`decode`]: `raw` = the whole packet.
/// `None` = forged / wrong key.
pub fn open_packet(header: &AudioHeader, raw: &[u8], cipher: &crate::secure::PacketCipher, direction: u8) -> Option<Vec<u8>> {
    if raw.len() < HEADER_LEN + crate::secure::TAG_LEN {
        return None;
    }
    cipher.open(direction, header.counter, &raw[..HEADER_LEN], &raw[HEADER_LEN..])
}

/// Kodiert Header + Payload in `out` (wird vorher geleert).
pub fn encode(header: &AudioHeader, payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.push(VERSION);
    out.push(header.flags);
    out.extend_from_slice(&header.token.to_le_bytes());
    out.extend_from_slice(&header.sequence.to_le_bytes());
    out.extend_from_slice(&header.timestamp.to_le_bytes());
    out.extend_from_slice(&header.counter.to_le_bytes());
    out.extend_from_slice(&header.stream_id.to_le_bytes());
    out.extend_from_slice(payload);
}

/// Bequeme Variante von [`encode`], die einen neuen Vec zurückgibt.
pub fn encode_to_vec(header: &AudioHeader, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER_LEN + payload.len());
    encode(header, payload, &mut v);
    v
}

/// Dekodiert ein Paket. Gibt `None` zurück bei falscher Magic/Version
/// oder zu kurzem Puffer.
pub fn decode(buf: &[u8]) -> Option<(AudioHeader, &[u8])> {
    if buf.len() < HEADER_LEN || buf.len() > MAX_PACKET_LEN {
        return None;
    }
    let magic = u16::from_le_bytes([buf[0], buf[1]]);
    if magic != MAGIC || buf[2] != VERSION {
        return None;
    }
    let header = AudioHeader {
        flags: buf[3],
        stream_id: u32::from_le_bytes(buf[28..32].try_into().ok()?),
        token: u64::from_le_bytes(buf[4..12].try_into().ok()?),
        sequence: u32::from_le_bytes(buf[12..16].try_into().ok()?),
        timestamp: u32::from_le_bytes(buf[16..20].try_into().ok()?),
        counter: u64::from_le_bytes(buf[20..28].try_into().ok()?),
    };
    if !matches!(header.flags, 0 | FLAG_SCREEN_AUDIO | FLAG_CONTROL) || (header.flags != FLAG_SCREEN_AUDIO && header.stream_id != 0) { return None; }
    Some((header, &buf[HEADER_LEN..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_id_is_authenticated_and_header_is_bounded() {
        use crate::secure::{PacketCipher, DIR_TO_SERVER};
        let cipher = PacketCipher::new(&[7; 32]).unwrap();
        let mut header = AudioHeader::new(5, 1, 960);
        header.flags = FLAG_SCREEN_AUDIO;
        header.stream_id = 9;
        let mut packet = seal_packet(&header, &cipher, DIR_TO_SERVER, b"opus");
        let parsed = decode(&packet).unwrap().0;
        assert_eq!(parsed.stream_id, 9);
        assert_eq!(open_packet(&parsed, &packet, &cipher, DIR_TO_SERVER).unwrap(), b"opus");
        packet[28] ^= 1;
        assert!(open_packet(&decode(&packet).unwrap().0, &packet, &cipher, DIR_TO_SERVER).is_none());
        for len in 0..HEADER_LEN { assert!(decode(&packet[..len]).is_none()); }
        packet[3] = FLAG_CONTROL | FLAG_SCREEN_AUDIO;
        assert!(decode(&packet).is_none());
        packet[3] = 0;
        assert!(decode(&packet).is_none());
        assert!(decode(&vec![0; MAX_PACKET_LEN + 1]).is_none());
    }

    #[test]
    fn roundtrip() {
        let header = AudioHeader { stream_id: 0, flags: 0, token: 0xDEAD_BEEF_CAFE, sequence: 42, timestamp: 960, counter: 7 };
        let payload = [0xABu8; 137];
        let buf = encode_to_vec(&header, &payload);
        assert_eq!(buf.len(), HEADER_LEN + payload.len());

        let (decoded_header, decoded_payload) = decode(&buf).expect("decode");
        assert_eq!(decoded_header, header);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn empty_payload_roundtrip() {
        // Leere Payload = Registrierungs-Ping.
        let header = AudioHeader::new(7, 0, 0);
        let buf = encode_to_vec(&header, &[]);
        let (h, p) = decode(&buf).expect("decode");
        assert_eq!(h, header);
        assert!(p.is_empty());
    }

    #[test]
    fn rejects_bad_magic() {
        let header = AudioHeader::new(1, 1, 1);
        let mut buf = encode_to_vec(&header, &[1, 2, 3]);
        buf[0] = 0x00;
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let header = AudioHeader::new(1, 1, 1);
        let mut buf = encode_to_vec(&header, &[1, 2, 3]);
        buf[2] = 99;
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn rejects_short_buffer() {
        let buf = [0u8; HEADER_LEN - 1];
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn sealed_packets_roundtrip() {
        use crate::secure::{PacketCipher, DIR_TO_SERVER};
        let key = PacketCipher::random_key();
        let (tx, rx) = (PacketCipher::new(&key).unwrap(), PacketCipher::new(&key).unwrap());
        let mut header = AudioHeader::new(9, 3, 960);
        header.flags = FLAG_SCREEN_AUDIO;
        let pkt = seal_packet(&header, &tx, DIR_TO_SERVER, b"opus");
        let (h, _) = decode(&pkt).unwrap();
        assert_eq!((h.token, h.sequence, h.flags), (9, 3, FLAG_SCREEN_AUDIO));
        assert_eq!(open_packet(&h, &pkt, &rx, DIR_TO_SERVER).as_deref(), Some(&b"opus"[..]));
        // Changing the header (e.g. the token) breaks the tag.
        let mut forged = pkt.clone();
        forged[4] ^= 0xFF;
        let (fh, _) = decode(&forged).unwrap();
        assert!(open_packet(&fh, &forged, &rx, DIR_TO_SERVER).is_none());
        // An empty payload (registration ping) is authenticated too.
        let ping = seal_packet(&AudioHeader::new(9, 0, 0), &tx, DIR_TO_SERVER, &[]);
        let (ph, _) = decode(&ping).unwrap();
        assert_eq!(open_packet(&ph, &ping, &rx, DIR_TO_SERVER).as_deref(), Some(&[][..]));
    }

    #[test]
    fn control_payloads_roundtrip_and_reject_garbage() {
        let pong = Control::Pong { audio_received: 7, audio_relayed: u32::MAX };
        assert_eq!(Control::decode(&pong.encode()), Some(pong));
        assert_eq!(Control::decode(&Control::Ping.encode()), Some(Control::Ping));
        let mut bad = Control::Ping.encode();
        bad[3] = 1;
        assert!(Control::decode(&bad).is_none(), "ping padding must be zero");
        assert!(Control::decode(&pong.encode()[..CONTROL_LEN - 1]).is_none(), "truncated");
        assert!(Control::decode(&[2u8; CONTROL_LEN + 1]).is_none(), "oversized");
        assert!(Control::decode(&[9u8; CONTROL_LEN]).is_none(), "unknown kind");
        assert!(Control::decode(&[]).is_none());
    }

    #[test]
    fn encode_reuses_buffer() {
        let mut out = Vec::new();
        encode(&AudioHeader::new(1, 1, 1), &[0x55; 64], &mut out);
        let len1 = out.len();
        encode(&AudioHeader::new(2, 2, 2), &[0xAA; 32], &mut out);
        assert!(out.len() < len1);
        let (h, p) = decode(&out).unwrap();
        assert_eq!(h.token, 2);
        assert_eq!(p.len(), 32);
    }
}
