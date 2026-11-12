//! The Linux client runs directly, without the Windows installer.
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Installed,
    Dev,
    NotInstalled,
}
pub fn mode() -> Mode {
    if appimage_path().is_some() {
        Mode::Installed
    } else {
        Mode::Dev
    }
}
pub fn install_dir() -> Option<PathBuf> {
    appimage_path()?.parent().map(PathBuf::from)
}
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|p| p.join("RustTalk"))
}
pub fn config_path() -> PathBuf {
    data_dir().unwrap_or_default().join("client_config.toml")
}
pub fn refresh_installed() {
    if let Err(error) = desktop_entry() {
        tracing::warn!("Desktop integration: {error:#}");
    }
}

fn desktop_entry() -> anyhow::Result<()> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .ok_or_else(|| anyhow::anyhow!("User data directory unknown"))?;
    let apps = root.join("applications");
    let entry = apps.join("org.rusttalk.RustTalk.desktop");
    let appimage = appimage_path();
    // Development builds need an identity too, but must not replace the user's installed launcher.
    if appimage.is_none() && entry.exists() {
        return Ok(());
    }
    let executable = appimage.clone().map(Ok).unwrap_or_else(std::env::current_exe)?;
    let executable = desktop_exec(&executable)?;
    std::fs::create_dir_all(&apps)?;
    let icons = root.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&icons)?;
    std::fs::write(
        icons.join("org.rusttalk.RustTalk.svg"),
        include_str!("../../../../packaging/linux/org.rusttalk.RustTalk.svg"),
    )?;
    let template = include_str!("../../../../packaging/linux/org.rusttalk.RustTalk.desktop");
    let mut text = template.replace("Exec=rusttalk %u", &format!("Exec={executable} %u"));
    if appimage.is_none() {
        text.push_str("NoDisplay=true\n");
    }
    std::fs::write(entry, text)?;
    Ok(())
}

fn desktop_exec(path: &std::path::Path) -> anyhow::Result<String> {
    let path = path.to_str().ok_or_else(|| anyhow::anyhow!("Executable path must be UTF-8"))?;
    anyhow::ensure!(!path.contains(['\n', '\r']), "Executable path contains a newline");
    // Desktop Entry Exec has both string escaping and quoted argument escaping.
    let quoted = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    Ok(format!("\"{}\"", quoted.replace('\\', "\\\\")))
}
pub fn install(_: bool) -> anyhow::Result<PathBuf> {
    anyhow::bail!("Run the Linux binary directly; installation is not supported")
}
pub fn uninstall(_: bool) -> anyhow::Result<()> {
    anyhow::bail!("The Linux client has no installer")
}

/// Only enable replacement when running inside an AppImage mount.
pub fn appimage_path() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let mount = PathBuf::from(std::env::var_os("APPDIR")?);
    if !path.is_absolute() || !mount.is_absolute() || !std::env::current_exe().ok()?.starts_with(&mount) {
        return None;
    }
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    metadata.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_command_quotes_special_paths() {
        assert_eq!(
            desktop_exec(std::path::Path::new("/tmp/My Client.AppImage")).unwrap(),
            "\"/tmp/My Client.AppImage\""
        );
        assert!(desktop_exec(std::path::Path::new("/tmp/bad\nExec=other")).is_err());
        assert!(desktop_exec(std::path::Path::new("/tmp/100%.AppImage")).unwrap().contains("100%%"));
    }
}
