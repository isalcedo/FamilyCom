//! FamilyCom TUI — terminal user interface for chatting with peers.
//!
//! Connects to the local `familycomd` daemon via Unix socket and provides
//! an interactive terminal interface for sending and receiving messages.
//!
//! # Usage
//!
//! ```bash
//! familycom                      # Connect to daemon and open TUI
//! familycom --set-name "Nuevo"   # Change display name and exit
//! ```
//!
//! The daemon must be running before starting the TUI. If it's not,
//! you'll see a helpful error message with instructions.

mod app;
mod event;
mod ipc_client;
mod ui;

use anyhow::{Context, Result};
use app::{Action, TuiApp};
use clap::Parser;
use crossterm::{
    event::EventStream,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use familycom_core::ipc::ClientRequest;
use ipc_client::IpcClient;
use ratatui::prelude::*;
use std::io::stdout;
use std::time::Duration;
use tokio_stream::StreamExt;

/// FamilyCom TUI client — chat with peers on your local network.
#[derive(Parser, Debug)]
#[command(name = "familycom", about = "FamilyCom LAN messenger TUI client")]
struct Cli {
    /// Change this machine's display name and exit.
    #[arg(long)]
    set_name: Option<String>,

    /// Path to the daemon's Unix socket.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to a file (not to stderr, which would mess up the TUI).
    // In production we'd log to ~/.local/share/familycom/tui.log,
    // but for now we just disable logging unless FAMILYCOM_LOG is set.
    if std::env::var("FAMILYCOM_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_env("FAMILYCOM_LOG"))
            .with_writer(std::io::stderr)
            .init();
    }

    let cli = Cli::parse();

    // Handle --set-name: change name and exit without opening TUI
    if let Some(name) = &cli.set_name {
        return set_display_name(name, &cli.socket).await;
    }

    // Connect to the daemon
    let socket_path = cli
        .socket
        .unwrap_or_else(familycom_core::config::AppConfig::default_socket_path);

    let mut client = match IpcClient::connect_to(&socket_path).await {
        Ok(client) => client,
        Err(ipc_client::IpcClientError::DaemonNotRunning(path)) => {
            eprintln!("Error: el daemon de FamilyCom no esta corriendo.");
            eprintln!();
            eprintln!("Inicia el daemon primero:");
            eprintln!("  familycomd");
            eprintln!();
            eprintln!("(buscando socket en: {})", path.display());
            std::process::exit(1);
        }
        Err(e) => {
            return Err(e).context("failed to connect to daemon");
        }
    };

    // Subscribe to real-time events
    client.subscribe().await.context("failed to subscribe")?;

    // Request initial data
    client.send(&ClientRequest::GetConfig).await?;
    client.send(&ClientRequest::ListPeers).await?;

    // Run the TUI
    run_tui(client).await
}

/// Runs the interactive TUI main loop.
///
/// This function takes over the terminal (raw mode, alternate screen)
/// and runs until the user quits. It handles:
/// - Terminal events (keyboard input)
/// - IPC messages from the daemon (peer updates, new messages)
/// - Periodic screen refresh
async fn run_tui(mut client: IpcClient) -> Result<()> {
    // Set up terminal for TUI rendering.
    // Raw mode: disables line buffering and echo, so we get each keypress.
    // Alternate screen: switches to a separate screen buffer, so our TUI
    // doesn't mess up the user's previous terminal content.
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    // Set up a panic hook that restores the terminal before printing
    // the panic message. Without this, a panic would leave the terminal
    // in raw mode with the alternate screen active — very confusing.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
        original_hook(info);
    }));

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = TuiApp::new();

    // Event stream from crossterm — delivers keyboard/mouse events asynchronously
    let mut event_stream = EventStream::new();

    // Tick interval for periodic UI refresh (e.g., updating timestamps)
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    // Read initial responses from daemon (Config and PeerList)
    for _ in 0..2 {
        if let Ok(Ok(msg)) = tokio::time::timeout(Duration::from_secs(2), client.recv()).await {
            app.handle_action(Action::ServerMessage(msg));
        }
    }

    app.status = "Conectado".to_string();

    // Main event loop
    loop {
        // Render the current state
        terminal.draw(|frame| ui::layout::render(frame, &app))?;

        // Wait for the next event (terminal input, daemon message, or tick)
        tokio::select! {
            // Terminal input events
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(evt)) => {
                        if let Some(action) = event::handle_event(&evt, &app) {
                            match action {
                                Action::SendMessage => {
                                    handle_send_message(&mut app, &mut client).await;
                                }
                                other => {
                                    app.handle_action(other);
                                }
                            }
                        }
                    }
                    Some(Err(_)) => {} // Input error, ignore
                    None => break,     // Event stream ended
                }
            }

            // Messages from the daemon (responses and pushed events)
            result = client.recv() => {
                match result {
                    Ok(msg) => {
                        // If we got a PeerList, also request messages for selected peer
                        let should_fetch = matches!(&msg,
                            familycom_core::ipc::ServerMessage::PeerList { .. }
                        );

                        app.handle_action(Action::ServerMessage(msg));

                        if should_fetch {
                            fetch_selected_peer_messages(&app, &mut client).await;
                        }
                    }
                    Err(ipc_client::IpcClientError::Disconnected) => {
                        app.status = "Desconectado del daemon".to_string();
                        // Could implement reconnection logic here
                    }
                    Err(e) => {
                        app.status = format!("Error: {e}");
                    }
                }
            }

            // Periodic tick for UI refresh
            _ = tick.tick() => {
                // Just triggers a redraw
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// Handles the SendMessage action: sends the input text to the selected peer.
async fn handle_send_message(app: &mut TuiApp, client: &mut IpcClient) {
    let content = app.input.trim().to_string();
    if content.is_empty() {
        return;
    }

    let peer_id = match app.selected_peer_id() {
        Some(id) => id.clone(),
        None => {
            app.status = "No hay peer seleccionado".to_string();
            return;
        }
    };

    // Clear the input buffer
    app.take_input();

    // Add the message to local display immediately (optimistic update)
    let message = familycom_core::types::Message {
        id: familycom_core::types::MessageId::generate(),
        peer_id: peer_id.clone(),
        direction: familycom_core::types::Direction::Sent,
        content: content.clone(),
        timestamp: familycom_core::types::Timestamp::now(),
        delivered: false,
    };
    app.messages.entry(peer_id.clone()).or_default().push(message);
    app.messages_scroll = 0;

    // Send via IPC to daemon
    if let Err(e) = client
        .send(&ClientRequest::SendMessage {
            peer_id,
            content,
        })
        .await
    {
        app.status = format!("Error enviando: {e}");
    }
}

/// Requests message history for the currently selected peer.
async fn fetch_selected_peer_messages(app: &TuiApp, client: &mut IpcClient) {
    if let Some(peer_id) = app.selected_peer_id() {
        let _ = client
            .send(&ClientRequest::GetMessages {
                peer_id: peer_id.clone(),
                limit: 100,
                before: None,
            })
            .await;
    }
}

/// Handles the --set-name CLI option.
async fn set_display_name(name: &str, socket: &Option<std::path::PathBuf>) -> Result<()> {
    let socket_path = socket
        .clone()
        .unwrap_or_else(familycom_core::config::AppConfig::default_socket_path);

    let mut client = IpcClient::connect_to(&socket_path)
        .await
        .context("could not connect to daemon")?;

    client
        .send(&ClientRequest::SetDisplayName {
            name: name.to_string(),
        })
        .await?;

    let response = client.recv().await?;
    match response {
        familycom_core::ipc::ServerMessage::Ok => {
            println!("Display name changed to: {name}");
            Ok(())
        }
        familycom_core::ipc::ServerMessage::Error { message, .. } => {
            eprintln!("Error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}
