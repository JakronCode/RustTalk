//! Installation on Windows (per user, no admin rights):
//! `%LOCALAPPDATA%\Programs\RustTalk\rusttalk.exe`, Start menu (+ optional
//! desktop) shortcut, entry in "Apps & features" (uninstall), app id for
//! toast notifications. Settings live in `%APPDATA%\RustTalk`.
//!
//! The downloaded `rusttalk-windows.exe` is its own installer: started from
//! anywhere else it offers to install itself. Dev builds (debug or started
//! from a cargo `target` folder) keep running portable as before.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

/// App user model id: toasts show "RustTalk" + icon instead of "PowerShell".
pub const AUMID: &str = "RustTalk.Client";
pub const APP_NAME: &str = "RustTalk";
const EXE_NAME: &str = "rusttalk.exe";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\RustTalk";
const AUMID_KEY: &str = r"Software\Classes\AppUserModelId\RustTalk.Client";
/// `rusttalk://` links (invites) open the installed app.
const PROTOCOL_KEY: &str = r"Software\Classes\rusttalk";

/// How this exe runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Started from the install folder: settings in AppData, updates on.
    Installed,
    /// Debug build or cargo `target` folder: portable, config in the working dir.
    Dev,
    /// Downloaded exe somewhere else: offers to install itself.
    NotInstalled,
}

/// `%LOCALAPPDATA%\Programs\RustTalk`
pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join(APP_NAME))
}

pub fn installed_exe() -> Option<PathBuf> {
    install_dir().map(|d| d.join(EXE_NAME))
}

/// `%APPDATA%\RustTalk` (settings of the installed app).
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(APP_NAME))
}

/// Mode of the running exe (computed once).
pub fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        detect_mode(exe.as_deref(), installed_exe().as_deref(), cfg!(debug_assertions), std::env::var_os("RUSTTALK_PORTABLE").is_some())
    })
}

/// Pure part of [`mode`] (testable).
pub fn detect_mode(exe: Option<&Path>, installed: Option<&Path>, debug_build: bool, portable_env: bool) -> Mode {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    if let (Some(exe), Some(inst)) = (exe, installed) {
        if norm(exe) == norm(inst) {
            return Mode::Installed;
        }
    }
    let in_target = exe.is_some_and(|e| norm(e).contains("\\target\\"));
    if debug_build || in_target || portable_env {
        Mode::Dev
    } else {
        Mode::NotInstalled
    }
}

/// Where the settings file lives: AppData when installed, else the working dir.
pub fn config_path() -> PathBuf {
    match (mode(), data_dir()) {
        (Mode::Installed, Some(dir)) => dir.join("client_config.toml"),
        _ => PathBuf::from("client_config.toml"),
    }
}

/// Installs the running exe. Returns the path of the installed exe.
pub fn install(desktop_shortcut: bool) -> Result<PathBuf> {
    let dir = install_dir().context("LOCALAPPDATA is not set")?;
    let target = dir.join(EXE_NAME);
    std::fs::create_dir_all(&dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let me = std::env::current_exe().context("Own path unknown")?;
    if !same_file(&me, &target) {
        // A running old version cannot be overwritten, but renamed.
        if target.exists() {
            let old = dir.join("rusttalk.old.exe");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&target, &old).context("RustTalk is still running - close it and try again")?;
        }
        std::fs::copy(&me, &target).with_context(|| format!("Could not copy to {}", target.display()))?;
    }
    let (png, ico) = write_icons(&dir)?;

    // Settings of a portable version (next to the exe or in the working dir) move along once.
    if let Some(data) = data_dir() {
        std::fs::create_dir_all(&data).ok();
        let new_cfg = data.join("client_config.toml");
        if !new_cfg.exists() {
            let candidates = [me.with_file_name("client_config.toml"), PathBuf::from("client_config.toml")];
            if let Some(old) = candidates.iter().find(|p| p.exists()) {
                let _ = std::fs::copy(old, &new_cfg);
            }
        }
    }

    if let Some(start_menu) = start_menu_dir() {
        std::fs::create_dir_all(&start_menu).ok();
        shortcut(&target, &ico, &start_menu.join("RustTalk.lnk"))?;
    }
    if desktop_shortcut {
        if let Some(desktop) = desktop_dir() {
            shortcut(&target, &ico, &desktop.join("RustTalk.lnk"))?;
        }
    }
    register_uninstall(&dir, &target, &ico)?;
    register_aumid(&png)?;
    register_protocol(&target, &ico)?;
    Ok(target)
}

/// Removes shortcuts, registry entries and (after this process exits) the
/// install folder. `keep_settings = false` also deletes `%APPDATA%\RustTalk`.
pub fn uninstall(keep_settings: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let _ = hkcu.delete_subkey_all(UNINSTALL_KEY);
    let _ = hkcu.delete_subkey_all(AUMID_KEY);
    let _ = hkcu.delete_subkey_all(PROTOCOL_KEY);
    for dir in [start_menu_dir(), desktop_dir()].into_iter().flatten() {
        let _ = std::fs::remove_file(dir.join("RustTalk.lnk"));
    }
    if !keep_settings {
        if let Some(data) = data_dir() {
            let _ = std::fs::remove_dir_all(data);
        }
    }
    if let Some(dir) = install_dir() {
        // The running exe cannot delete itself: a detached shell does it a moment later.
        // `raw_arg`: cmd.exe does not understand Rust's \" escaping of quoted paths.
        use std::os::windows::process::CommandExt;
        let script = format!("/C ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        std::process::Command::new("cmd")
            .raw_arg(&script)
            .current_dir(std::env::temp_dir())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .context("Could not remove the install folder")?;
    }
    Ok(())
}

/// Called on every start of the installed app: keeps the app id and the
/// uninstall entry (version) current, removes the leftover of an update.
pub fn refresh_installed() {
    let Some(dir) = install_dir() else { return };
    let _ = std::fs::remove_file(dir.join("rusttalk.old.exe"));
    if let Ok((png, ico)) = write_icons(&dir) {
        let _ = register_aumid(&png);
        let _ = register_protocol(&dir.join(EXE_NAME), &ico);
        let _ = register_uninstall(&dir, &dir.join(EXE_NAME), &ico);
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn start_menu_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs"))
}

fn desktop_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|d| PathBuf::from(d).join("Desktop"))
}

/// Logo as PNG (toasts) and ICO (shortcuts, "Apps & features").
fn write_icons(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let icon = crate::logo::window_icon();
    let img = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba).context("icon size")?;
    let png = dir.join("rusttalk.png");
    let ico = dir.join("rusttalk.ico");
    img.save_with_format(&png, image::ImageFormat::Png).context("write icon png")?;
    img.save_with_format(&ico, image::ImageFormat::Ico).context("write icon ico")?;
    Ok((png, ico))
}

fn shortcut(target: &Path, icon: &Path, lnk: &Path) -> Result<()> {
    let mut link = mslnk::ShellLink::new(target).map_err(|e| anyhow::anyhow!("shortcut: {e}"))?;
    link.set_icon_location(Some(icon.to_string_lossy().to_string()));
    if let Some(dir) = target.parent() {
        link.set_working_dir(Some(dir.to_string_lossy().to_string()));
    }
    link.set_name(Some("Lightweight voice chat".to_string()));
    link.create_lnk(lnk).map_err(|e| anyhow::anyhow!("shortcut {}: {e}", lnk.display()))
}

fn register_uninstall(dir: &Path, exe: &Path, ico: &Path) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
    let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    key.set_value("DisplayName", &APP_NAME)?;
    key.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"))?;
    key.set_value("Publisher", &APP_NAME)?;
    key.set_value("InstallLocation", &dir.to_string_lossy().to_string())?;
    key.set_value("DisplayIcon", &ico.to_string_lossy().to_string())?;
    key.set_value("UninstallString", &format!("\"{}\" --uninstall", exe.display()))?;
    key.set_value("QuietUninstallString", &format!("\"{}\" --uninstall --quiet", exe.display()))?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    key.set_value("EstimatedSize", &size_kb)?;
    Ok(())
}

fn register_protocol(exe: &Path, ico: &Path) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(PROTOCOL_KEY)?;
    key.set_value("", &"URL:RustTalk Protocol")?;
    key.set_value("URL Protocol", &"")?;
    let (icon, _) = hkcu.create_subkey(format!(r"{PROTOCOL_KEY}\DefaultIcon"))?;
    icon.set_value("", &ico.to_string_lossy().to_string())?;
    let (cmd, _) = hkcu.create_subkey(format!(r"{PROTOCOL_KEY}\shell\open\command"))?;
    cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;
    Ok(())
}

fn register_aumid(png: &Path) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(AUMID_KEY)?;
    key.set_value("DisplayName", &APP_NAME)?;
    key.set_value("IconUri", &png.to_string_lossy().to_string())?;
    key.set_value("IconBackgroundColor", &"FF5865F2")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_detection() {
        let inst = Path::new(r"C:\Users\x\AppData\Local\Programs\RustTalk\rusttalk.exe");
        let same_other_case = Path::new(r"c:\users\X\appdata\local\programs\rusttalk\RUSTTALK.EXE");
        assert_eq!(detect_mode(Some(same_other_case), Some(inst), false, false), Mode::Installed);
        assert_eq!(detect_mode(Some(inst), Some(inst), true, false), Mode::Installed, "installed wins over debug");
        let dl = Path::new(r"C:\Users\x\Downloads\rusttalk-windows.exe");
        assert_eq!(detect_mode(Some(dl), Some(inst), false, false), Mode::NotInstalled);
        assert_eq!(detect_mode(Some(dl), Some(inst), false, true), Mode::Dev, "RUSTTALK_PORTABLE");
        let cargo = Path::new(r"C:\Users\x\AppData\Local\Programs\RustTalk-src\target\release\rusttalk.exe");
        assert_eq!(detect_mode(Some(cargo), Some(inst), false, false), Mode::Dev, "cargo target = dev");
        assert_eq!(detect_mode(Some(dl), Some(inst), true, false), Mode::Dev, "debug build");
        assert_eq!(detect_mode(None, None, false, false), Mode::NotInstalled);
    }

    #[test]
    fn icons_are_written() {
        let dir = std::env::temp_dir().join(format!("rusttalk-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png, ico) = write_icons(&dir).unwrap();
        assert!(std::fs::read(&png).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(std::fs::read(&ico).unwrap().starts_with(&[0, 0, 1, 0]), "ICO header");
        std::fs::remove_dir_all(&dir).ok();
    }
}
