//! Autostart installation and removal for FamilyCom daemon.
//!
//! Configures the OS to launch `familycomd` automatically on user login.
//!
//! # Platform Behavior
//!
//! - **Linux**: Creates a `.desktop` file in `~/.config/autostart/`.
//!   This is the XDG Autostart standard, supported by GNOME, KDE, XFCE,
//!   and most other desktop environments.
//!
//! - **macOS**: Creates a LaunchAgent plist in `~/Library/LaunchAgents/`.
//!   launchd loads this automatically on login and keeps the daemon alive.
//!
//! # Binary Path Resolution
//!
//! The autostart config points to the *absolute path* of the currently
//! running `familycomd` binary. This means if you move the binary after
//! running `install`, you'll need to re-run `install` to update the path.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// The name of the Linux autostart desktop entry file.
const DESKTOP_FILENAME: &str = "familycom.desktop";

/// The name of the macOS LaunchAgent plist file.
const PLIST_FILENAME: &str = "com.familycom.daemon.plist";

/// Installs autostart configuration for the current platform.
///
/// If `dry_run` is true, prints what would be done without making changes.
pub fn install(dry_run: bool) -> Result<()> {
    let binary_path = std::env::current_exe()
        .context("could not determine path to familycomd binary")?;

    if cfg!(target_os = "macos") {
        install_macos(&binary_path, dry_run)
    } else {
        install_linux(&binary_path, dry_run)
    }
}

/// Removes the autostart configuration for the current platform.
///
/// If `dry_run` is true, prints what would be done without making changes.
pub fn uninstall(dry_run: bool) -> Result<()> {
    if cfg!(target_os = "macos") {
        uninstall_macos(dry_run)
    } else {
        uninstall_linux(dry_run)
    }
}

// ---------------------------------------------------------------------------
// Linux: XDG Autostart (.desktop file)
// ---------------------------------------------------------------------------

/// Returns the path to the autostart directory on Linux.
///
/// Uses `$XDG_CONFIG_HOME/autostart/` (typically `~/.config/autostart/`).
fn linux_autostart_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("could not determine XDG config directory")?;
    Ok(config_dir.join("autostart"))
}

/// Installs the autostart desktop entry on Linux.
fn install_linux(binary_path: &std::path::Path, dry_run: bool) -> Result<()> {
    let autostart_dir = linux_autostart_dir()?;
    let desktop_file = autostart_dir.join(DESKTOP_FILENAME);

    // The .desktop file follows the XDG Desktop Entry specification.
    // Key fields:
    //   - Type=Application: this is an application entry
    //   - Exec: absolute path to the daemon binary
    //   - X-GNOME-Autostart-enabled: tells GNOME to actually autostart it
    //   - Terminal=false: don't open a terminal window (daemon runs headless)
    let content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=FamilyCom\n\
         Comment=LAN Messenger Daemon\n\
         Exec={}\n\
         Icon=familycom\n\
         Terminal=false\n\
         Categories=Network;Chat;\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n",
        binary_path.display()
    );

    if dry_run {
        println!("[dry-run] Would create: {}", desktop_file.display());
        println!("[dry-run] Content:");
        println!("{content}");
        return Ok(());
    }

    // Create the autostart directory if it doesn't exist
    std::fs::create_dir_all(&autostart_dir)
        .context("failed to create autostart directory")?;

    std::fs::write(&desktop_file, content)
        .with_context(|| format!("failed to write {}", desktop_file.display()))?;

    println!("Autostart installed: {}", desktop_file.display());
    println!("FamilyCom daemon will start on your next login.");
    Ok(())
}

/// Removes the autostart desktop entry on Linux.
fn uninstall_linux(dry_run: bool) -> Result<()> {
    let autostart_dir = linux_autostart_dir()?;
    let desktop_file = autostart_dir.join(DESKTOP_FILENAME);

    if !desktop_file.exists() {
        println!("No autostart configuration found at: {}", desktop_file.display());
        return Ok(());
    }

    if dry_run {
        println!("[dry-run] Would remove: {}", desktop_file.display());
        return Ok(());
    }

    std::fs::remove_file(&desktop_file)
        .with_context(|| format!("failed to remove {}", desktop_file.display()))?;

    println!("Autostart removed: {}", desktop_file.display());
    println!("FamilyCom daemon will no longer start on login.");
    Ok(())
}

// ---------------------------------------------------------------------------
// macOS: LaunchAgent plist
// ---------------------------------------------------------------------------

/// Returns the path to the LaunchAgents directory on macOS.
fn macos_launch_agents_dir() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .context("could not determine home directory")?;
    Ok(home.join("Library").join("LaunchAgents"))
}

/// Installs the LaunchAgent plist on macOS.
///
/// The plist tells launchd to:
/// - Start the daemon on login (`RunAtLoad`)
/// - Restart it if it crashes (`KeepAlive`)
/// - Redirect stdout/stderr to log files in /tmp/
fn install_macos(binary_path: &std::path::Path, dry_run: bool) -> Result<()> {
    let agents_dir = macos_launch_agents_dir()?;
    let plist_file = agents_dir.join(PLIST_FILENAME);

    // The plist uses Apple's XML property list format.
    // launchd reads this on login and manages the daemon lifecycle.
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.familycom.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/familycomd.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/familycomd.err.log</string>
</dict>
</plist>
"#,
        binary_path.display()
    );

    if dry_run {
        println!("[dry-run] Would create: {}", plist_file.display());
        println!("[dry-run] Content:");
        println!("{content}");
        return Ok(());
    }

    std::fs::create_dir_all(&agents_dir)
        .context("failed to create LaunchAgents directory")?;

    std::fs::write(&plist_file, content)
        .with_context(|| format!("failed to write {}", plist_file.display()))?;

    println!("LaunchAgent installed: {}", plist_file.display());
    println!("FamilyCom daemon will start on your next login.");
    println!();
    println!("To start it now without logging out:");
    println!("  launchctl load {}", plist_file.display());
    Ok(())
}

/// Removes the LaunchAgent plist on macOS.
fn uninstall_macos(dry_run: bool) -> Result<()> {
    let agents_dir = macos_launch_agents_dir()?;
    let plist_file = agents_dir.join(PLIST_FILENAME);

    if !plist_file.exists() {
        println!("No LaunchAgent found at: {}", plist_file.display());
        return Ok(());
    }

    if dry_run {
        println!("[dry-run] Would remove: {}", plist_file.display());
        return Ok(());
    }

    // Unload from launchd first (ignore errors — might not be loaded)
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &plist_file.to_string_lossy()])
        .output();

    std::fs::remove_file(&plist_file)
        .with_context(|| format!("failed to remove {}", plist_file.display()))?;

    println!("LaunchAgent removed: {}", plist_file.display());
    println!("FamilyCom daemon will no longer start on login.");
    Ok(())
}
