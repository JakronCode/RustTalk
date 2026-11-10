//! Length-Prefixed-Framing für das TCP-Steuerprotokoll.
//!
//! Format: 4 Byte Länge (Little-Endian) + bincode-Payload.

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximale Frame-Größe (Schutz gegen kaputte/bösartige Peers).
/// Grosszuegig: Chat-Bilder und Video-Frames laufen ueber denselben Kanal.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Serialisiert `msg` und schreibt es als Frame auf `writer`.
pub async fn write_frame<W, T>(writer: &mut W, msg: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = bincode::serialize(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if payload.len() > MAX_FRAME_LEN as usize {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

/// Liest ein Frame und deserialisiert es.
///
/// Gibt `Ok(None)` zurück, wenn die Verbindung sauber geschlossen wurde,
/// bevor ein neues Frame begonnen hat.
pub async fn read_frame<R, T>(reader: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf);
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid frame length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    // EOF mitten im Frame ist ein Fehler (abgebrochene Übertragung).
    reader.read_exact(&mut buf).await?;
    let msg = bincode::deserialize(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        write_frame(&mut a, &"erste Nachricht".to_string()).await.unwrap();
        write_frame(&mut a, &vec![1u32, 2, 3, u32::MAX]).await.unwrap();

        let first: Option<String> = read_frame(&mut b).await.unwrap();
        let second: Option<Vec<u32>> = read_frame(&mut b).await.unwrap();
        assert_eq!(first.as_deref(), Some("erste Nachricht"));
        assert_eq!(second, Some(vec![1, 2, 3, u32::MAX]));
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let (mut a, mut b) = duplex(64);
        write_frame(&mut a, &42u32).await.unwrap();
        drop(a); // Verbindung schließen

        let msg: Option<u32> = read_frame(&mut b).await.unwrap();
        assert_eq!(msg, Some(42));
        let eof: Option<u32> = read_frame(&mut b).await.unwrap();
        assert_eq!(eof, None);
    }

    #[tokio::test]
    async fn oversized_length_is_rejected() {
        let (mut a, mut b) = duplex(64);
        a.write_all(&(MAX_FRAME_LEN + 1).to_le_bytes()).await.unwrap();
        let res: std::io::Result<Option<Vec<u8>>> = read_frame(&mut b).await;
        assert!(res.is_err());
    }
}
