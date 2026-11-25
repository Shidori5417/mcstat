//! Persistent configuration (TOML).
//!
//! OWNERSHIP: Neon (`feat/config-state`).
//! Public signatures below are the contract; bodies are to be implemented.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{DEFAULT_REFRESH_SECS, DEFAULT_TIMEOUT_SECS, ServerConfig};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not determine a configuration directory")]
    NoConfigDir,
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid server: {0}")]
    Validation(String),
}

/// On-disk configuration. Example:
/// ```toml
/// refresh_interval_secs = 5
/// timeout_secs = 3
///
/// [[servers]]
/// name = "Hypixel"
/// address = "mc.hypixel.net"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_refresh")]
    pub refresh_interval_secs: u64,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
}

fn default_refresh() -> u64 {
    DEFAULT_REFRESH_SECS
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

impl Default for AppConfig {
    /// Default config written on first start: default intervals plus a few
    /// well-known public servers so the app is useful immediately.
    fn default() -> Self {
        todo!("feat/config-state")
    }
}

impl AppConfig {
    /// `<OS config dir>/mcstat/config.toml` (uses the `dirs` crate).
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        todo!("feat/config-state")
    }

    /// Load the config at `path`. If the file does not exist, create parent
    /// directories, write [`AppConfig::default`] there and return it.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        let _ = path;
        todo!("feat/config-state")
    }

    /// Atomically save (write temp file in same dir, then rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let _ = path;
        todo!("feat/config-state")
    }
}

/// Validate raw user input from the "add server" form and build a
/// [`ServerConfig`].
/// - `name`: trimmed, non-empty (if empty, use the address as name).
/// - `address`: trimmed, non-empty, no whitespace. May contain `host:port`;
///   in that case the port is split off (only if `port` input is empty).
///   IPv6 literals in brackets (`[::1]:25565`) must be handled.
/// - `port`: empty => `None` (SRV / default); otherwise 1..=65535.
pub fn validate_server_input(
    name: &str,
    address: &str,
    port: &str,
) -> Result<ServerConfig, ConfigError> {
    let _ = (name, address, port);
    todo!("feat/config-state")
}
