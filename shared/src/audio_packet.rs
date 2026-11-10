//! UDP-Audiopakete: fester 20-Byte-Header + Opus-Payload.
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |    Magic (0x5254 = "RT")      |  Version (1)  |     Flags     |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                         Token (u64)                           |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                        Sequence (u32)                         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                       Timestamp (u32)                         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                  Opus-Payload (variabel) ...                  |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! Semantik des Token-Felds:
//! - **Client -> Server:** das beim Login vergebene `udp_token` (Registrierung
//!   der UDP-Adresse + Identifikation des Sprechers).
//! - **Server -> Client (Relay):** die `client_id` des Sprechers.

/// Magic-Bytes "RT" (RustTalk), Little-Endian.
pub const MAGIC: u16 = 0x5254;

/// Protokollversion des Audio-Headers.
pub const VERSION: u8 = 1;

/// Länge des Headers in Bytes.
pub const HEADER_LEN: usize = 20;

/// Sinnvolle Obergrenze für ein UDP-Paket im LAN (unterhalb der MTU).
pub const MAX_PACKET_LEN: usize = 1500;

/// Opus-Samplerate in Hz (fest verdrahtet im ganzen System).
pub const SAMPLE_RATE: u32 = 48_000;

/// Samples pro Opus-Frame (20 ms @ 48 kHz).
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    pub flags: u8,
    pub token: u64,
    pub sequence: u32,
    /// Beliebiger, pro Sender monoton steigender Zeitstempel
    /// (Samples seit Sendebeginn); dient der Jitter-Erkennung.
    pub timestamp: u32,
}

impl AudioHeader {
    pub fn new(token: u64, sequence: u32, timestamp: u32) -> Self {
        Self { flags: 0, token, sequence, timestamp }
    }
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
    if buf.len() < HEADER_LEN {
        return None;
    }
    let magic = u16::from_le_bytes([buf[0], buf[1]]);
    if magic != MAGIC || buf[2] != VERSION {
        return None;
    }
    let header = AudioHeader {
        flags: buf[3],
        token: u64::from_le_bytes(buf[4..12].try_into().ok()?),
        sequence: u32::from_le_bytes(buf[12..16].try_into().ok()?),
        timestamp: u32::from_le_bytes(buf[16..20].try_into().ok()?),
    };
    Some((header, &buf[HEADER_LEN..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let header = AudioHeader { flags: 0, token: 0xDEAD_BEEF_CAFE, sequence: 42, timestamp: 960 };
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
