//! Configuration management for FamilyCom.
//!
//! The config file lives at a platform-appropriate location:
//! - Linux: `~/.config/familycom/config.toml`
//! - macOS: `~/Library/Application Support/familycom/config.toml`
//!
//! On first run, no config file exists. The daemon detects this and
//! creates one with a fresh `peer_id` and the user's chosen display name.
//!
//! # Config File Format (TOML)
//!
//! ```toml
//! peer_id = "550e8400-e29b-41d4-a716-446655440000"
//! display_name = "PC-Sala"
//! tcp_port = 0        # 0 means auto-assign
//! # network_interface = "enp5s0"  # optional: restrict mDNS to this interface
//! ```

use crate::types::PeerId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur when loading or saving configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file at {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse config file at {path}: {source}")]
    ParseFile {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("failed to write config file at {path}: {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("could not determine config directory for this platform")]
    NoConfigDir,
}

/// The persisted configuration for this FamilyCom instance.
///
/// This is what gets saved to and loaded from the TOML config file.
/// All fields have sensible defaults except `peer_id` which must be
/// generated on first run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Unique identifier for this machine (UUID v4, generated once).
    pub peer_id: String,

    /// Human-readable name for this machine (chosen by user).
    pub display_name: String,

    /// TCP port for peer-to-peer messaging.
    /// `0` means the OS assigns a random available port.
    #[serde(default)]
    pub tcp_port: u16,

    /// Optional: terminal command used by the tray icon's "Open Chat" action.
    /// If not set, a platform-appropriate default is used.
    #[serde(default)]
    pub terminal_command: Option<String>,

    /// Optional: restrict mDNS to this network interface (e.g. "enp5s0").
    /// If not set, the default-route interface is auto-detected.
    /// Useful when Docker or VPN interfaces cause mDNS conflicts.
    #[serde(default)]
    pub network_interface: Option<String>,
}

impl AppConfig {
    /// Returns the platform-appropriate config directory path.
    ///
    /// - Linux: `~/.config/familycom/`
    /// - macOS: `~/Library/Application Support/familycom/`
    ///
    /// Returns `None` if the platform's config directory can't be determined
    /// (very rare — would mean $HOME is not set).
    pub fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("familycom"))
    }

    /// Returns the full path to the config file.
    pub fn config_file_path() -> Result<PathBuf, ConfigError> {
        Ok(Self::config_dir()
            .ok_or(ConfigError::NoConfigDir)?
            .join("config.toml"))
    }

    /// Returns the platform-appropriate data directory for storing the database and logs.
    ///
    /// - Linux: `~/.local/share/familycom/`
    /// - macOS: `~/Library/Application Support/familycom/`
    pub fn data_dir() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("familycom"))
    }

    /// Returns the default path for the SQLite database.
    pub fn default_db_path() -> Result<PathBuf, ConfigError> {
        Ok(Self::data_dir()
            .ok_or(ConfigError::NoConfigDir)?
            .join("familycom.db"))
    }

    /// Returns the default path for the Unix socket used for IPC.
    ///
    /// Uses `$XDG_RUNTIME_DIR` on Linux (typically `/run/user/1000/`),
    /// falling back to `/tmp/familycom-{uid}.sock`.
    pub fn default_socket_path() -> PathBuf {
        if let Some(runtime_dir) = dirs::runtime_dir() {
            runtime_dir.join("familycom.sock")
        } else {
            // Fallback: use /tmp with the process's PID-derived user identifier.
            // std::process::id() is cross-platform and doesn't need libc.
            // We use a fixed name per user by reading $USER env var.
            let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
            PathBuf::from(format!("/tmp/familycom-{user}.sock"))
        }
    }

    /// Loads the config from the default config file path.
    ///
    /// Returns `Ok(None)` if the config file doesn't exist yet (first run).
    /// Returns `Ok(Some(config))` if the file was loaded successfully.
    /// Returns `Err(...)` if the file exists but can't be read or parsed.
    pub fn load() -> Result<Option<Self>, ConfigError> {
        let path = Self::config_file_path()?;
        Self::load_from(&path)
    }

    /// Loads the config from a specific file path.
    ///
    /// Returns `Ok(None)` if the file doesn't exist.
    pub fn load_from(path: &Path) -> Result<Option<Self>, ConfigError> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::ReadFile {
            path: path.to_owned(),
            source: e,
        })?;
        let config: Self =
            toml::from_str(&content).map_err(|e| ConfigError::ParseFile {
                path: path.to_owned(),
                source: e,
            })?;
        Ok(Some(config))
    }

    /// Saves this config to the default config file path.
    ///
    /// Creates the parent directory if it doesn't exist.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_file_path()?;
        self.save_to(&path)
    }

    /// Saves this config to a specific file path.
    ///
    /// Creates the parent directory if it doesn't exist.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::WriteFile {
                path: path.to_owned(),
                source: e,
            })?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content).map_err(|e| ConfigError::WriteFile {
            path: path.to_owned(),
            source: e,
        })?;
        Ok(())
    }

    /// Creates a new config for first-run with a fresh peer ID.
    pub fn new_first_run(display_name: &str) -> Self {
        Self {
            peer_id: PeerId::generate().to_string(),
            display_name: display_name.to_string(),
            tcp_port: 0,
            terminal_command: None,
            network_interface: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn config_roundtrip() {
        // Create a config, save it to a temp file, load it back
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let config = AppConfig {
            peer_id: "test-peer-id".to_string(),
            display_name: "Mi Computador".to_string(),
            tcp_port: 9876,
            terminal_command: None,
            network_interface: None,
        };

        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap().unwrap();

        assert_eq!(loaded.peer_id, "test-peer-id");
        assert_eq!(loaded.display_name, "Mi Computador");
        assert_eq!(loaded.tcp_port, 9876);
    }

    #[test]
    fn config_missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let result = AppConfig::load_from(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn config_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("deep").join("nested").join("config.toml");

        let config = AppConfig::new_first_run("Test");
        config.save_to(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn config_spanish_display_name() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let config = AppConfig {
            peer_id: "id".to_string(),
            display_name: "Habitación de Mamá".to_string(),
            tcp_port: 0,
            terminal_command: None,
            network_interface: None,
        };

        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Habitación de Mamá");
    }

    #[test]
    fn first_run_generates_unique_ids() {
        let a = AppConfig::new_first_run("A");
        let b = AppConfig::new_first_run("B");
        assert_ne!(a.peer_id, b.peer_id);
    }
}
