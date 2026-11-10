//! Gemeinsamer Code für RustTalk-Client und -Server.
//!
//! - [`protocol`]: TCP-Steuerprotokoll (serde/bincode)
//! - [`framing`]: Length-Prefixed-Framing über TCP
//! - [`audio_packet`]: UDP-Audiopakete (Header + Opus-Payload)

pub mod audio_packet;
pub mod framing;
pub mod protocol;
