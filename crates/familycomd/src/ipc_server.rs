//! Unix socket IPC server for TUI client connections.
//!
//! The daemon exposes a Unix domain socket that TUI clients connect to.
//! Each connected client can:
//! - Send `ClientRequest`s (list peers, send messages, etc.)
//! - Receive `ServerMessage` responses
//! - Subscribe to real-time events (new messages, peer changes)
//!
//! # Protocol
//!
//! JSON lines over Unix socket: each message is a JSON object + newline.
//! See `familycom_core::ipc` for the type definitions.
//!
//! # Multiple Clients
//!
//! Multiple TUI clients can connect simultaneously. Each gets its own
//! connection handler task. Subscribed clients all receive the same events.

use familycom_core::ipc::{self, ClientRequest, ServerMessage};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// A request from a TUI client, tagged with a response channel.
///
/// The daemon processes the request and sends the response back
/// through the `response_tx` channel. This pattern decouples the
/// IPC server from the daemon's business logic.
#[derive(Debug)]
pub struct IpcRequest {
    /// The request from the client.
    pub request: ClientRequest,
    /// Channel to send the response back to this specific client.
    pub response_tx: mpsc::Sender<ServerMessage>,
}

/// The IPC server managing the Unix socket.
pub struct IpcServer {
    /// Path to the Unix socket file.
    socket_path: PathBuf,
    /// The underlying Unix listener.
    listener: UnixListener,
}

impl IpcServer {
    /// Creates a new IPC server bound to the given socket path.
    ///
    /// If a stale socket file exists (from a previous crash), it is removed
    /// before binding. This is safe because we check for an existing daemon
    /// process via the socket — if connecting fails, it's stale.
    pub async fn bind(socket_path: &Path) -> Result<Self, std::io::Error> {
        // Remove stale socket file if it exists.
        // This handles the case where the daemon crashed without cleanup.
        if socket_path.exists() {
            info!(path = %socket_path.display(), "removing stale socket file");
            std::fs::remove_file(socket_path)?;
        }

        // Ensure the parent directory exists
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(socket_path)?;
        info!(path = %socket_path.display(), "IPC server listening");

        Ok(Self {
            socket_path: socket_path.to_owned(),
            listener,
        })
    }

    /// Runs the accept loop for IPC clients.
    ///
    /// Each connected client gets its own handler task. Incoming requests
    /// are forwarded to the daemon via `request_tx`. Real-time events are
    /// broadcast to all subscribed clients via `event_tx`.
    ///
    /// # Arguments
    ///
    /// * `request_tx` - Channel to forward client requests to the daemon.
    /// * `event_rx_factory` - A broadcast sender that clients subscribe to for real-time events.
    pub async fn accept_loop(
        self,
        request_tx: mpsc::Sender<IpcRequest>,
        event_tx: broadcast::Sender<ServerMessage>,
    ) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    debug!("accepted IPC client connection");
                    let req_tx = request_tx.clone();
                    let evt_tx = event_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_ipc_client(stream, req_tx, evt_tx).await {
                            debug!(error = %e, "IPC client disconnected");
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "failed to accept IPC connection");
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Returns the socket path.
    #[allow(dead_code)]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

/// Clean up the socket file when the server is dropped.
/// This is important so the next run doesn't find a stale socket.
impl Drop for IpcServer {
    fn drop(&mut self) {
        if self.socket_path.exists() {
            if let Err(e) = std::fs::remove_file(&self.socket_path) {
                warn!(
                    path = %self.socket_path.display(),
                    error = %e,
                    "failed to remove socket file on shutdown"
                );
            } else {
                debug!(path = %self.socket_path.display(), "removed socket file");
            }
        }
    }
}

/// Handles a single IPC client connection.
///
/// Reads JSON-line requests from the client, forwards them to the daemon,
/// and sends responses back. If the client sends `Subscribe`, it also
/// receives broadcast events.
async fn handle_ipc_client(
    stream: UnixStream,
    request_tx: mpsc::Sender<IpcRequest>,
    event_tx: broadcast::Sender<ServerMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line_buf = String::new();

    // Channel for responses to this specific client's requests
    let (response_tx, mut response_rx) = mpsc::channel::<ServerMessage>(32);

    // Whether this client is subscribed to real-time events
    let mut subscribed = false;
    let mut event_rx: Option<broadcast::Receiver<ServerMessage>> = None;

    loop {
        // Use tokio::select! to handle both:
        // 1. New requests from the client (reading from socket)
        // 2. Responses from the daemon (reading from response channel)
        // 3. Broadcast events (if subscribed)
        tokio::select! {
            // Read next request line from the client
            read_result = buf_reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => {
                        // Client disconnected (EOF)
                        debug!("IPC client disconnected (EOF)");
                        return Ok(());
                    }
                    Ok(_) => {
                        // Parse the JSON request
                        let request = match ipc::decode_request(&line_buf) {
                            Ok(req) => req,
                            Err(e) => {
                                warn!(error = %e, line = %line_buf.trim(), "invalid IPC request");
                                let error_msg = ServerMessage::Error {
                                    code: "invalid_request".to_string(),
                                    message: format!("failed to parse request: {e}"),
                                };
                                let json = ipc::encode_response(&error_msg)?;
                                writer.write_all(json.as_bytes()).await?;
                                line_buf.clear();
                                continue;
                            }
                        };

                        // Handle Subscribe specially — we set up the broadcast receiver
                        if matches!(request, ClientRequest::Subscribe) {
                            if !subscribed {
                                subscribed = true;
                                event_rx = Some(event_tx.subscribe());
                                debug!("IPC client subscribed to events");
                            }
                            // Send OK response
                            let ok = ServerMessage::Ok;
                            let json = ipc::encode_response(&ok)?;
                            writer.write_all(json.as_bytes()).await?;
                            line_buf.clear();
                            continue;
                        }

                        // Forward the request to the daemon
                        let ipc_request = IpcRequest {
                            request,
                            response_tx: response_tx.clone(),
                        };
                        if request_tx.send(ipc_request).await.is_err() {
                            error!("daemon request channel closed");
                            return Ok(());
                        }

                        line_buf.clear();
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                }
            }

            // Send response back to client
            Some(response) = response_rx.recv() => {
                let json = ipc::encode_response(&response)?;
                writer.write_all(json.as_bytes()).await?;
            }

            // Forward broadcast events to subscribed clients
            event = async {
                match &mut event_rx {
                    Some(rx) => rx.recv().await,
                    None => {
                        // If not subscribed, this branch should never resolve.
                        // We use pending() to make it sleep forever.
                        std::future::pending::<Result<ServerMessage, broadcast::error::RecvError>>().await
                    }
                }
            } => {
                match event {
                    Ok(msg) => {
                        let json = ipc::encode_response(&msg)?;
                        writer.write_all(json.as_bytes()).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(missed = n, "IPC client lagged behind on events");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("event broadcast channel closed");
                        return Ok(());
                    }
                }
            }
        }
    }
}
