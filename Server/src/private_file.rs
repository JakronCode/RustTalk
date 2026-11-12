//! Publish complete private files without replacing existing state.

use std::io::{self, Write};
use std::path::Path;

/// Returns false if the destination already exists. A failed write never publishes partial data.
pub fn create(path: &Path, data: &[u8]) -> io::Result<bool> {
    create_with(path, |file| file.write_all(data))
}

fn create_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<bool> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".rusttalk-{:016x}.tmp", rand::random::<u64>()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        match std::fs::hard_link(&temporary, path) {
            Ok(()) => {
                #[cfg(unix)]
                std::fs::File::open(parent)?.sync_all()?;
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    })();
    drop(file);
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_and_failed_publication_preserve_state() {
        let dir =
            std::env::temp_dir().join(format!("rusttalk-publish-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        // Simulate a process killed halfway through writing its temporary file.
        std::fs::write(dir.join(".rusttalk-interrupted.tmp"), b"partial").unwrap();
        let path = dir.join("config");
        assert!(!path.exists());
        assert!(create_with(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("injected disk write failure"))
        })
        .is_err());
        assert!(!path.exists(), "failed writes never expose the destination");
        assert!(create(&path, b"complete").unwrap());
        assert!(!create(&path, b"replacement").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");
        assert!(create(&dir.join("missing/config"), b"partial").is_err());
        assert!(!dir.join("missing/config").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
