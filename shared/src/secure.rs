//! Encryption: TLS for the TCP control connection, ChaCha20-Poly1305 for the
//! UDP audio packets.
//!
//! **TLS:** the server has its own self-signed certificate (no CA, LAN).
//! The client trusts it on first use and remembers the certificate's SHA-256
//! fingerprint per server (like SSH); a different certificate later is
//! refused ([`PinnedVerifier`]). The TLS handshake still proves that the
//! server owns the certificate's key.
//!
//! **UDP:** every client gets a random 256-bit key in the `Welcome` (sent over
//! TLS). Packets carry a 64-bit counter; nonce = direction + counter, the
//! 28-byte header is authenticated (AAD). The server opens a packet with the
//! sender's key and seals it again with each receiver's key.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// Length of a UDP session key.
pub const UDP_KEY_LEN: usize = 32;
/// Poly1305 tag appended to every encrypted payload.
pub const TAG_LEN: usize = 16;

/// Direction byte of the nonce.
pub const DIR_TO_SERVER: u8 = 0;
pub const DIR_TO_CLIENT: u8 = 1;

/// Name the client asks for in the TLS handshake (the certificate is pinned, not the name).
pub const TLS_SERVER_NAME: &str = "rusttalk.local";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// "AB:CD:..." SHA-256 of a DER certificate (what clients pin and the server logs).
pub fn fingerprint(cert_der: &[u8]) -> String {
    Sha256::digest(cert_der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Error text when the pinned certificate does not match (the client detects it by this prefix).
pub const CERT_CHANGED: &str = "CERTIFICATE CHANGED";

/// Trust-on-first-use certificate check.
#[derive(Debug)]
pub struct PinnedVerifier {
    /// Fingerprint remembered for this server (`None` = first connect: accept and learn).
    expected: Option<String>,
    /// Fingerprint the server presented (read it after the handshake).
    pub seen: Arc<Mutex<Option<String>>>,
    provider: Arc<CryptoProvider>,
}

impl PinnedVerifier {
    pub fn new(expected: Option<String>) -> Arc<Self> {
        Arc::new(Self { expected, seen: Arc::new(Mutex::new(None)), provider: provider() })
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        *self.seen.lock().unwrap() = Some(fp.clone());
        match &self.expected {
            Some(want) if !want.eq_ignore_ascii_case(&fp) => {
                Err(rustls::Error::General(format!("{CERT_CHANGED}: {fp}")))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// TLS client config with a pinned (or to-be-learned) server certificate.
pub fn client_config(verifier: Arc<PinnedVerifier>) -> Arc<rustls::ClientConfig> {
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(cfg)
}

/// TLS server config from a DER certificate + PKCS#8 key.
pub fn server_config(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let key = PrivateKeyDer::try_from(key_der).map_err(|e| rustls::Error::General(e.to_string()))?;
    let cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert_der)], key)?;
    Ok(Arc::new(cfg))
}

/// Seals/opens audio payloads with one session key.
pub struct PacketCipher {
    aead: ChaCha20Poly1305,
    /// Next counter for packets this side sends (shared by all sending threads).
    send_counter: AtomicU64,
}

/// Never prints the key.
impl std::fmt::Debug for PacketCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PacketCipher(..)")
    }
}

impl PacketCipher {
    pub fn new(key: &[u8]) -> Option<Self> {
        (key.len() == UDP_KEY_LEN).then(|| Self { aead: ChaCha20Poly1305::new(Key::from_slice(key)), send_counter: AtomicU64::new(1) })
    }

    /// A fresh random key.
    pub fn random_key() -> Vec<u8> {
        let mut k = vec![0u8; UDP_KEY_LEN];
        getrandom::fill(&mut k).expect("random");
        k
    }

    fn nonce(direction: u8, counter: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[0] = direction;
        n[4..].copy_from_slice(&counter.to_le_bytes());
        *Nonce::from_slice(&n)
    }

    /// Next unique counter for sending.
    pub fn next_counter(&self) -> u64 {
        self.send_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Encrypts `plain`, authenticating `aad` (the packet header).
    pub fn seal(&self, direction: u8, counter: u64, aad: &[u8], plain: &[u8]) -> Vec<u8> {
        self.aead.encrypt(&Self::nonce(direction, counter), Payload { msg: plain, aad }).expect("encrypt")
    }

    /// Decrypts; `None` = forged, corrupted or wrong key.
    pub fn open(&self, direction: u8, counter: u64, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        self.aead.decrypt(&Self::nonce(direction, counter), Payload { msg: sealed, aad }).ok()
    }
}

/// Rejects replayed packets: tracks the highest authenticated counter and a
/// 128-packet window below it (IPsec-style). Call only after the AEAD check.
#[derive(Debug, Default, Clone)]
pub struct ReplayWindow {
    highest: u64,
    seen: u128,
}

impl ReplayWindow {
    pub const SIZE: u64 = 128;

    /// `true` = first time this counter is seen and it is not too old.
    pub fn accept(&mut self, counter: u64) -> bool {
        if counter == 0 {
            return false; // counters start at 1
        }
        if counter > self.highest {
            let shift = counter - self.highest;
            self.seen = if shift >= Self::SIZE { 0 } else { self.seen << shift };
            self.seen |= 1;
            self.highest = counter;
            return true;
        }
        let age = self.highest - counter;
        if age >= Self::SIZE {
            return false;
        }
        let bit = 1u128 << age;
        if self.seen & bit != 0 {
            return false;
        }
        self.seen |= bit;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_window_rejects_duplicates_and_stale_counters() {
        let mut w = ReplayWindow::default();
        assert!(!w.accept(0));
        assert!(w.accept(1));
        assert!(!w.accept(1), "duplicate");
        assert!(w.accept(5));
        assert!(w.accept(3), "late but new");
        assert!(!w.accept(3));
        assert!(w.accept(1000));
        assert!(!w.accept(5), "too old");
        assert!(w.accept(1000 - 127));
        assert!(!w.accept(1000 - 128));
        assert!(w.accept(u64::MAX));
        assert!(!w.accept(u64::MAX));
    }

    #[test]
    fn packets_roundtrip_and_tampering_is_detected() {
        let key = PacketCipher::random_key();
        let a = PacketCipher::new(&key).unwrap();
        let b = PacketCipher::new(&key).unwrap();
        let c = a.next_counter();
        assert_ne!(c, a.next_counter(), "counters are unique");
        let sealed = a.seal(DIR_TO_SERVER, c, b"header", b"opus bytes");
        assert_eq!(sealed.len(), 10 + TAG_LEN);
        assert_eq!(b.open(DIR_TO_SERVER, c, b"header", &sealed).as_deref(), Some(&b"opus bytes"[..]));
        assert!(b.open(DIR_TO_CLIENT, c, b"header", &sealed).is_none(), "other direction");
        assert!(b.open(DIR_TO_SERVER, c + 1, b"header", &sealed).is_none(), "other counter");
        assert!(b.open(DIR_TO_SERVER, c, b"HEADER", &sealed).is_none(), "header changed");
        let mut flipped = sealed.clone();
        flipped[0] ^= 1;
        assert!(b.open(DIR_TO_SERVER, c, b"header", &flipped).is_none(), "payload changed");
        let other = PacketCipher::new(&PacketCipher::random_key()).unwrap();
        assert!(other.open(DIR_TO_SERVER, c, b"header", &sealed).is_none(), "wrong key");
        assert!(PacketCipher::new(&[0u8; 5]).is_none());
    }

    #[test]
    fn fingerprints() {
        let fp = fingerprint(b"cert");
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert!(fp.contains(':'));
        assert_ne!(fp, fingerprint(b"other"));
    }
}
