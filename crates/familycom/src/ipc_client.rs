//! IPC client for connecting to the FamilyCom daemon.
//!
//! Connects to the daemon's Unix domain socket and provides typed methods
//! for sending requests and receiving responses/events.
//!
//! # Usage
//!
//! ```no_run
//! # async fn example() {
//! let mut client = IpcClient::connect().await.unwrap();
//! client.subscribe().await.unwrap();
//!
//! // Send a request
//! client.send(&ClientRequest::ListPeers).await.unwrap();
//!
//! // Read the response
//! let response = client.recv().await.unwrap();
//! # }
//! ```

use familycom_core::config::AppConfig;
use familycom_core::ipc::{self, ClientRequest, ServerMessage};
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::UnixStream;
use tracing::debug;

/// Errors that can occur in the IPC client.
#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("could not connect to daemon at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("daemon is not running (socket not found at {0})")]
    DaemonNotRunning(PathBuf),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("connection to daemon closed")]
    Disconnected,

    #[error("IPC protocol error: {0}")]
    Protocol(String),
}

/// Client connection to the FamilyCom daemon.
///
/// Wraps a Unix socket connection with typed request/response methods.
/// The connection is split into a reader and writer so we can read
/// responses/events while sending requests without blocking.
pub struct IpcClient {
    /// Buffered reader for receiving JSON lines from the daemon.
    reader: BufReader<ReadHalf<UnixStream>>,
    /// Writer for sending JSON lines to the daemon.
    writer: WriteHalf<UnixStream>,
    /// Buffer reused for reading lines (avoids repeated allocation).
    line_buf: String,
}

impl IpcClient {
    /// Connects to the daemon at the default socket path.
    ///
    /// Returns a helpful error if the daemon is not running.
    pub async fn connect() -> Result<Self, IpcClientError> {
        let path = AppConfig::default_socket_path();
        Self::connect_to(&path).await
    }

    /// Connects to the daemon at a specific socket path.
    pub async fn connect_to(path: &PathBuf) -> Result<Self, IpcClientError> {
        if !path.exists() {
            return Err(IpcClientError::DaemonNotRunning(path.clone()));
        }

        let stream = UnixStream::connect(path).await.map_err(|e| {
            IpcClientError::Connect {
                path: path.clone(),
                source: e,
            }
        })?;

        let (reader, writer) = tokio::io::split(stream);
        let reader = BufReader::new(reader);

        debug!(path = %path.display(), "connected to daemon");

        Ok(Self {
            reader,
            writer,
            line_buf: String::with_capacity(4096),
        })
    }

    /// Sends a request to the daemon.
    pub async fn send(&mut self, request: &ClientRequest) -> Result<(), IpcClientError> {
        let json = ipc::encode_request(request)
            .map_err(|e| IpcClientError::Protocol(e.to_string()))?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Reads the next message from the daemon.
    ///
    /// This can be either a response to a previous request, or a pushed
    /// event (if subscribed). Returns `Err(Disconnected)` if the daemon
    /// closes the connection.
    pub async fn recv(&mut self) -> Result<ServerMessage, IpcClientError> {
        self.line_buf.clear();
        let bytes_read = self.reader.read_line(&mut self.line_buf).await?;
        if bytes_read == 0 {
            return Err(IpcClientError::Disconnected);
        }
        let msg = ipc::decode_response(&self.line_buf)
            .map_err(|e| IpcClientError::Protocol(e.to_string()))?;
        Ok(msg)
    }

    /// Subscribes to real-time events from the daemon.
    ///
    /// After subscribing, `recv()` will also return pushed events
    /// (NewMessage, PeerOnline, PeerOffline, etc.) in addition to
    /// request responses.
    pub async fn subscribe(&mut self) -> Result<(), IpcClientError> {
        self.send(&ClientRequest::Subscribe).await?;
        // Wait for the Ok acknowledgment
        let response = self.recv().await?;
        match response {
            ServerMessage::Ok => Ok(()),
            ServerMessage::Error { code, message } => {
                Err(IpcClientError::Protocol(format!("{code}: {message}")))
            }
            _ => Err(IpcClientError::Protocol(
                "unexpected response to Subscribe".to_string(),
            )),
        }
    }
}
