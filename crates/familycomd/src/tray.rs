//! System tray icon and menu.
//!
//! Creates a system tray icon with a context menu for the daemon.
//! The tray icon indicates that the daemon is running and provides
//! quick access to the TUI and shutdown.
//!
//! # Platform Requirements
//!
//! - **Linux**: Requires GTK3 and libappindicator3. The tray icon runs
//!   on a dedicated thread with its own GTK event loop.
//! - **macOS**: Uses NSApplication run loop. Must run on the main thread.
//!
//! # Architecture
//!
//! The tray runs its own event loop (GTK on Linux, Cocoa on macOS) on
//! a separate thread. It communicates with the daemon's tokio runtime
//! via channels:
//!
//! ```text
//! Tray Thread                    Tokio Runtime
//! ┌──────────────┐              ┌──────────────┐
//! │ GTK/Cocoa    │──TrayEvent──>│ DaemonApp    │
//! │ event loop   │              │ main loop    │
//! └──────────────┘              └──────────────┘
//! ```

use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use std::sync::mpsc as std_mpsc;
use tray_icon::TrayIconBuilder;
use tracing::{debug, error, info};

/// Events from the tray icon to the daemon.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// User clicked "Open Chat" — daemon should launch the TUI.
    OpenChat,
    /// User clicked "Quit" — daemon should shut down.
    Quit,
}

/// Starts the system tray icon on the current thread.
///
/// **This function blocks** — it runs the platform's event loop (GTK/Cocoa).
/// Call it from a dedicated thread, not from the tokio runtime.
///
/// # Arguments
///
/// * `event_tx` - Channel to send tray events to the daemon's main loop.
/// * `peer_count` - Initial peer count to display in the status menu item.
///
/// # Returns
///
/// Only returns when the tray event loop exits (e.g., after Quit).
pub fn run_tray(event_tx: std_mpsc::Sender<TrayEvent>, _peer_count: usize) {
    // Initialize GTK (required on Linux before creating tray/menu widgets).
    // If GTK init fails (e.g., no display available), the daemon continues
    // without a tray icon — equivalent to running with --no-tray.
    #[cfg(target_os = "linux")]
    if let Err(e) = gtk::init() {
        error!(error = %e, "failed to initialize GTK, running without tray icon");
        return;
    }

    // Load the icon from the embedded PNG bytes.
    // include_bytes! embeds the file at compile time, so no runtime file I/O.
    let icon_bytes = include_bytes!("../../../assets/icon.png");
    let icon = load_icon(icon_bytes);

    // Build the context menu
    let menu = Menu::new();

    let open_item = MenuItem::new("Abrir Chat", true, None);
    let status_item = MenuItem::new("Estado: En linea", false, None);
    let quit_item = MenuItem::new("Salir", true, None);

    // Store the IDs for matching events later
    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();

    menu.append(&open_item).expect("failed to add menu item");
    menu.append(&PredefinedMenuItem::separator()).expect("failed to add separator");
    menu.append(&status_item).expect("failed to add menu item");
    menu.append(&PredefinedMenuItem::separator()).expect("failed to add separator");
    menu.append(&quit_item).expect("failed to add menu item");

    // Create the tray icon
    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("FamilyCom - LAN Messenger")
        .with_icon(icon)
        .build()
        .expect("failed to create tray icon");

    info!("system tray icon created");

    // Subscribe to menu events
    let menu_rx = MenuEvent::receiver();

    // Run the platform event loop.
    // On Linux, we must run the GTK main loop so that libappindicator
    // can process D-Bus signals and actually render the tray icon.
    // A simple sleep-polling loop is not enough — without GTK event
    // dispatch, the icon is created internally but never appears.
    #[cfg(target_os = "linux")]
    {
        // Poll for menu events from within the GTK event loop.
        // glib::timeout_add_local runs a callback at regular intervals
        // on the GTK thread, which is exactly what we need.
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if let Ok(event) = menu_rx.try_recv() {
                if event.id() == &open_id {
                    debug!("tray: Open Chat clicked");
                    if event_tx.send(TrayEvent::OpenChat).is_err() {
                        gtk::main_quit();
                        return gtk::glib::ControlFlow::Break;
                    }
                } else if event.id() == &quit_id {
                    debug!("tray: Quit clicked");
                    let _ = event_tx.send(TrayEvent::Quit);
                    gtk::main_quit();
                    return gtk::glib::ControlFlow::Break;
                }
            }
            gtk::glib::ControlFlow::Continue
        });

        // Blocks until gtk::main_quit() is called from the timeout callback.
        gtk::main();
    }

    // On non-Linux platforms, use a simple polling loop with sleep.
    #[cfg(not(target_os = "linux"))]
    {
        loop {
            if let Ok(event) = menu_rx.try_recv() {
                if event.id() == &open_id {
                    debug!("tray: Open Chat clicked");
                    if event_tx.send(TrayEvent::OpenChat).is_err() {
                        break;
                    }
                } else if event.id() == &quit_id {
                    debug!("tray: Quit clicked");
                    let _ = event_tx.send(TrayEvent::Quit);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    info!("tray event loop exited");
}

/// Loads a tray icon from PNG bytes.
///
/// The tray-icon crate requires an `Icon` in RGBA format.
/// We use the `image` crate to decode the PNG and extract the raw pixels.
fn load_icon(png_bytes: &[u8]) -> tray_icon::Icon {
    let img = image::load_from_memory(png_bytes)
        .expect("failed to decode embedded icon PNG")
        .into_rgba8();

    let (width, height) = img.dimensions();
    let rgba = img.into_raw();

    tray_icon::Icon::from_rgba(rgba, width, height)
        .expect("failed to create tray icon from RGBA data")
}

/// Launches the TUI in a new terminal window.
///
/// Tries to find an appropriate terminal emulator and opens the
/// `familycom` binary in it.
pub fn open_chat_in_terminal() {
    // Try to find the familycom binary in PATH or next to familycomd
    let familycom_path = find_familycom_binary();

    let result = if cfg!(target_os = "macos") {
        // macOS: use `open` to launch Terminal.app
        std::process::Command::new("open")
            .args(["-a", "Terminal", &familycom_path])
            .spawn()
    } else {
        // Linux: try common terminal emulators in order of preference
        try_linux_terminals(&familycom_path)
    };

    match result {
        Ok(_) => info!("launched TUI in terminal"),
        Err(e) => error!(error = %e, "failed to launch TUI in terminal"),
    }
}

/// Finds the familycom binary path.
///
/// First checks if it's in the same directory as familycomd,
/// then falls back to looking in PATH.
fn find_familycom_binary() -> String {
    // Try same directory as current binary
    if let Ok(current_exe) = std::env::current_exe() {
        let sibling = current_exe.with_file_name("familycom");
        if sibling.exists() {
            return sibling.to_string_lossy().to_string();
        }
    }
    // Fallback: assume it's in PATH
    "familycom".to_string()
}

/// Tries to launch a terminal emulator on Linux.
///
/// First checks the `$TERMINAL` environment variable (the standard way to
/// specify a preferred terminal on Linux), then falls back to a list of
/// common terminal emulators in order of popularity.
fn try_linux_terminals(command: &str) -> Result<std::process::Child, std::io::Error> {
    // Try the user's preferred terminal first ($TERMINAL is the de facto
    // standard on Linux for specifying the default terminal emulator).
    if let Ok(term) = std::env::var("TERMINAL") {
        if !term.is_empty() {
            debug!(terminal = %term, "trying $TERMINAL");
            match std::process::Command::new(&term)
                .args(["-e", command])
                .spawn()
            {
                Ok(child) => return Ok(child),
                Err(e) => {
                    debug!(terminal = %term, error = %e, "$TERMINAL not available, trying fallbacks");
                }
            }
        }
    }

    // List of terminal emulators to try, with their -e flag
    let terminals = [
        ("x-terminal-emulator", vec!["-e"]),    // Debian/Ubuntu default
        ("foot", vec!["--"]),                   // Popular Wayland terminal (Arch, Sway)
        ("alacritty", vec!["-e"]),              // Popular modern terminal
        ("kitty", vec!["--"]),                  // Another popular choice
        ("wezterm", vec!["start", "--"]),       // Cross-platform GPU terminal
        ("gnome-terminal", vec!["--", "--"]),   // GNOME (legacy)
        ("kgx", vec!["-e"]),                    // GNOME Console (modern replacement)
        ("konsole", vec!["-e"]),                // KDE
        ("xfce4-terminal", vec!["-e"]),         // XFCE
        ("xterm", vec!["-e"]),                  // Fallback
    ];

    let mut last_error = std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no terminal emulator found — set $TERMINAL to your preferred terminal",
    );

    for (terminal, args) in &terminals {
        let mut cmd_args: Vec<&str> = args.clone();
        cmd_args.push(command);

        match std::process::Command::new(terminal)
            .args(&cmd_args)
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(e) => {
                debug!(terminal, error = %e, "terminal not available, trying next");
                last_error = e;
            }
        }
    }

    Err(last_error)
}

/// Asks the tray's GTK event loop to quit.
///
/// Called during daemon shutdown so the tray thread exits cleanly,
/// which in turn unblocks the `spawn_blocking` bridge task.
/// Safe to call even if the tray was never started (GTK handles it).
pub fn request_quit() {
    #[cfg(target_os = "linux")]
    gtk::glib::idle_add(|| {
        gtk::main_quit();
        gtk::glib::ControlFlow::Break
    });
}
