//! FamilyCom Daemon — the background service that powers LAN messaging.
//!
//! # Usage
//!
//! ```bash
//! familycomd                    # Start with system tray
//! familycomd --no-tray          # Start without system tray (headless)
//! familycomd --name "PC-Sala"   # Start with a specific display name
//! familycomd --port 9876        # Use a specific TCP port
//! familycomd install            # Set up autostart on login
//! familycomd uninstall          # Remove autostart configuration
//! ```
//!
//! On first run, the daemon generates a unique peer ID and prompts for
//! a display name (if running in an interactive terminal). The config
//! is saved to `~/.config/familycom/config.toml`.
//!
//! # Architecture
//!
//! The daemon spawns several concurrent tasks:
//! 1. mDNS discovery (background thread via mdns-sd)
//! 2. TCP message server (tokio task)
//! 3. IPC server on Unix socket (tokio task)
//! 4. System tray icon (dedicated thread with platform event loop)
//! 5. Main event loop in DaemonApp (tokio task)

mod app;
mod autostart;
mod client;
mod discovery;
mod ipc_server;
mod notifications;
mod server;
mod tray;

use anyhow::{Context, Result};
use app::DaemonApp;
use clap::{Parser, Subcommand};
use discovery::DiscoveryService;
use familycom_core::config::AppConfig;
use familycom_core::db::Database;
use ipc_server::IpcServer;
use notifications::NotificationManager;
use server::MessageServer;
use std::io::{self, Write};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// FamilyCom daemon — LAN messaging background service.
#[derive(Parser, Debug)]
#[command(name = "familycomd", about = "FamilyCom LAN messenger daemon")]
struct Cli {
    /// Subcommand to run (install, uninstall). If omitted, starts the daemon.
    #[command(subcommand)]
    command: Option<Command>,

    /// Display name for this machine on the network.
    /// Overrides the name in config.toml for this run.
    #[arg(short, long)]
    name: Option<String>,

    /// TCP port for peer-to-peer messaging (0 = auto-assign).
    #[arg(short, long, default_value = "0")]
    port: u16,

    /// Path to the configuration file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Path to the SQLite database file.
    #[arg(long)]
    db: Option<PathBuf>,

    /// Path to the Unix socket for IPC.
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Disable the system tray icon (run headless in terminal).
    #[arg(long)]
    no_tray: bool,
}

/// Subcommands for managing the daemon installation.
#[derive(Subcommand, Debug)]
enum Command {
    /// Set up autostart so the daemon launches on login.
    ///
    /// On Linux, creates a .desktop file in ~/.config/autostart/.
    /// On macOS, creates a LaunchAgent plist in ~/Library/LaunchAgents/.
    Install {
        /// Show what would be done without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove the autostart configuration.
    ///
    /// Removes the autostart file created by `install`.
    Uninstall {
        /// Show what would be done without making changes.
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands before initializing the full daemon.
    // Install/uninstall don't need logging, async runtime, etc.
    match &cli.command {
        Some(Command::Install { dry_run }) => {
            return autostart::install(*dry_run);
        }
        Some(Command::Uninstall { dry_run }) => {
            return autostart::uninstall(*dry_run);
        }
        None => {} // No subcommand — start the daemon
    }

    // Initialize logging.
    // The FAMILYCOM_LOG env var controls the log level (default: info).
    // Logs go to both stderr and a log file in the data directory.
    init_logging();

    // -----------------------------------------------------------------------
    // Load or create configuration
    // -----------------------------------------------------------------------
    let config_path = match &cli.config {
        Some(path) => path.clone(),
        None => AppConfig::config_file_path().context("could not determine config directory")?,
    };

    let mut config = match AppConfig::load_from(&config_path)? {
        Some(config) => {
            info!(path = %config_path.display(), "loaded config");
            config
        }
        None => {
            // First run — generate peer ID and get display name
            info!("first run detected, creating new config");
            let display_name = get_display_name()?;
            let config = AppConfig::new_first_run(&display_name);
            config.save_to(&config_path)?;
            info!(
                path = %config_path.display(),
                peer_id = %config.peer_id,
                display_name = %config.display_name,
                "saved new config"
            );
            config
        }
    };

    // CLI overrides
    if let Some(name) = &cli.name {
        config.display_name = name.clone();
    }
    if cli.port != 0 {
        config.tcp_port = cli.port;
    }

    // -----------------------------------------------------------------------
    // Open database
    // -----------------------------------------------------------------------
    let db_path = match &cli.db {
        Some(path) => path.clone(),
        None => AppConfig::default_db_path().context("could not determine data directory")?,
    };

    // Ensure the parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db = Database::open(&db_path).context("failed to open database")?;
    info!(path = %db_path.display(), "database opened");

    // -----------------------------------------------------------------------
    // Start TCP message server
    // -----------------------------------------------------------------------
    let bind_addr = format!("0.0.0.0:{}", config.tcp_port);
    let tcp_server = MessageServer::bind(&bind_addr)
        .await
        .context("failed to start TCP server")?;

    let tcp_port = tcp_server.port();
    info!(port = tcp_port, "TCP message server started");

    // -----------------------------------------------------------------------
    // Start mDNS discovery
    // -----------------------------------------------------------------------
    let peer_id = familycom_core::types::PeerId::new(&config.peer_id);
    let (discovery, discovery_rx) =
        DiscoveryService::new(peer_id, &config.display_name, tcp_port)
            .context("failed to start mDNS discovery")?;

    // -----------------------------------------------------------------------
    // Start IPC server
    // -----------------------------------------------------------------------
    let socket_path = match &cli.socket {
        Some(path) => path.clone(),
        None => AppConfig::default_socket_path(),
    };

    let ipc_server = IpcServer::bind(&socket_path)
        .await
        .context("failed to start IPC server")?;

    info!(path = %socket_path.display(), "IPC server started");

    // -----------------------------------------------------------------------
    // Create the daemon app and wire everything together
    // -----------------------------------------------------------------------
    let mut daemon_app = DaemonApp::new(db, config);
    let event_tx = daemon_app.event_sender();

    // Channels for inter-task communication
    let (message_tx, message_rx) = mpsc::channel(256);
    let (ipc_request_tx, ipc_request_rx) = mpsc::channel(64);
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

    // Spawn the TCP server accept loop
    tokio::spawn(async move {
        tcp_server.accept_loop(message_tx).await;
    });

    // Spawn the IPC server accept loop
    tokio::spawn(async move {
        ipc_server.accept_loop(ipc_request_tx, event_tx).await;
    });

    // -----------------------------------------------------------------------
    // Start system tray (if enabled)
    // -----------------------------------------------------------------------
    let tray_event_rx = if !cli.no_tray {
        let (tray_event_tx, tray_event_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            tray::run_tray(tray_event_tx, 0);
        });
        Some(tray_event_rx)
    } else {
        info!("system tray disabled (--no-tray)");
        None
    };

    // -----------------------------------------------------------------------
    // Set up notification manager
    // -----------------------------------------------------------------------
    let mut notification_mgr = NotificationManager::new();

    // Subscribe to daemon events for notifications
    let mut notification_rx = daemon_app.event_sender().subscribe();

    // Spawn notification handler task
    tokio::spawn(async move {
        loop {
            match notification_rx.recv().await {
                Ok(familycom_core::ipc::ServerMessage::NewMessage { ref message }) => {
                    if message.direction == familycom_core::types::Direction::Received {
                        // Get the sender name from the message content context
                        // We pass a preview of the message
                        let preview = if message.content.len() > 100 {
                            format!("{}...", &message.content[..message.content.floor_char_boundary(97)])
                        } else {
                            message.content.clone()
                        };
                        notification_mgr.notify_new_message("Peer", &preview);
                    }
                }
                Ok(_) => {} // Other events don't need notifications
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(missed = n, "notification handler lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    // -----------------------------------------------------------------------
    // Set up signal handler for graceful shutdown
    // -----------------------------------------------------------------------
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("received Ctrl+C, initiating shutdown");
                let _ = shutdown_tx_clone.send(()).await;
            }
            Err(e) => {
                error!(error = %e, "failed to listen for Ctrl+C");
            }
        }
    });

    // Bridge tray events from the std channel (blocking) to a tokio channel.
    // We spawn a blocking task that reads from the std receiver and forwards
    // events to a tokio mpsc channel that the async code can select! on.
    if let Some(tray_rx) = tray_event_rx {
        let (tray_async_tx, mut tray_async_rx) = mpsc::channel::<tray::TrayEvent>(16);

        // Blocking bridge thread: reads std::sync::mpsc → sends to tokio::mpsc
        tokio::task::spawn_blocking(move || {
            while let Ok(event) = tray_rx.recv() {
                if tray_async_tx.blocking_send(event).is_err() {
                    break; // Receiver dropped, daemon is shutting down
                }
            }
        });

        // Async handler: processes tray events in the tokio runtime
        let shutdown_tx_tray = shutdown_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = tray_async_rx.recv().await {
                match event {
                    tray::TrayEvent::OpenChat => {
                        tray::open_chat_in_terminal();
                    }
                    tray::TrayEvent::Quit => {
                        info!("quit requested from tray");
                        let _ = shutdown_tx_tray.send(()).await;
                        break;
                    }
                }
            }
        });
    }

    // Run the main event loop (blocks until shutdown)
    info!("daemon is running. Press Ctrl+C to stop.");
    daemon_app
        .run(discovery_rx, message_rx, ipc_request_rx, shutdown_rx)
        .await;

    // Clean shutdown
    info!("shutting down...");

    // Tell the tray's GTK event loop to quit so the blocking bridge
    // thread can exit and the tokio runtime shuts down cleanly.
    if !cli.no_tray {
        tray::request_quit();
    }

    discovery.shutdown();
    info!("daemon stopped");

    // Force exit to avoid hanging on lingering background threads from
    // external libraries (mdns-sd browse loop, GTK) that don't shut down
    // promptly. All graceful cleanup has already completed above.
    std::process::exit(0);
}

/// Prompts the user for a display name on first run.
///
/// If stdin is not a terminal (e.g., launched by autostart), falls back
/// to the system hostname.
fn get_display_name() -> Result<String> {
    // Check if we're running in an interactive terminal
    if atty_is_terminal() {
        print!("Enter a display name for this machine: ");
        io::stdout().flush()?;
        let mut name = String::new();
        io::stdin().read_line(&mut name)?;
        let name = name.trim().to_string();
        if name.is_empty() {
            // Fall back to hostname if user just pressed Enter
            return Ok(get_hostname());
        }
        Ok(name)
    } else {
        // Non-interactive — use hostname
        Ok(get_hostname())
    }
}

/// Checks if stdin is connected to a terminal.
fn atty_is_terminal() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// Gets the system hostname as a fallback display name.
fn get_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "FamilyCom-User".to_string())
}

/// Initializes the tracing logging infrastructure.
///
/// Sets up a layered subscriber that writes to:
/// 1. stderr — so logs appear in the terminal when running interactively
/// 2. A log file at `~/.local/share/familycom/daemon.log` — persists across runs
///
/// The log level is controlled by the `FAMILYCOM_LOG` environment variable.
/// Defaults to `info` if not set.
fn init_logging() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let env_filter = EnvFilter::try_from_env("FAMILYCOM_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Always log to stderr for interactive use
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr);

    // Try to set up file logging; if it fails, daemon still works with stderr only
    let file_layer = AppConfig::data_dir()
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let log_path = dir.join("daemon.log");
            // Open the log file in append mode so we don't lose previous logs
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
                .ok()
        })
        .map(|file| {
            fmt::layer()
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false) // No ANSI color codes in the log file
        });

    // Build the subscriber with both layers.
    // The file layer is optional — if the log file couldn't be opened,
    // only stderr logging is active.
    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();
}
