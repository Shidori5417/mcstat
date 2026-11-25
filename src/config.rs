//! Persistent configuration (TOML).
//!
//! OWNERSHIP: Neon (`feat/config-state`).
//! Public signatures below are the contract; bodies are to be implemented.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::net::Ipv6Addr;
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
        Self {
            refresh_interval_secs: DEFAULT_REFRESH_SECS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            servers: vec![
                ServerConfig {
                    name: "Hypixel".to_string(),
                    address: "mc.hypixel.net".to_string(),
                    port: None,
                },
                ServerConfig {
                    name: "CubeCraft".to_string(),
                    address: "play.cubecraft.net".to_string(),
                    port: None,
                },
                ServerConfig {
                    name: "2b2t".to_string(),
                    address: "2b2t.org".to_string(),
                    port: None,
                },
            ],
        }
    }
}

impl AppConfig {
    /// `<OS config dir>/mcstat/config.toml` (uses the `dirs` crate).
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        dirs::config_dir()
            .map(|dir| dir.join("mcstat").join("config.toml"))
            .ok_or(ConfigError::NoConfigDir)
    }

    /// Load the config at `path`. If the file does not exist, create parent
    /// directories, write [`AppConfig::default`] there and return it.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                let default_config = Self::default();
                default_config.save(path)?;
                Ok(default_config)
            }
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Atomically save (write temp file in same dir, then rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let serialized = toml::to_string_pretty(self)?;

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut tmp_name = path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| OsString::from("config.toml"));
        tmp_name.push(".tmp");

        let tmp_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
            _ => PathBuf::from(tmp_name),
        };

        fs::write(&tmp_path, serialized).map_err(|source| ConfigError::Io {
            path: tmp_path.clone(),
            source,
        })?;

        if let Err(source) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            });
        }

        Ok(())
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
    let name_trimmed = name.trim();
    let addr_trimmed = address.trim();
    let port_trimmed = port.trim();

    if addr_trimmed.is_empty() {
        return Err(ConfigError::Validation(
            "server address cannot be empty".to_string(),
        ));
    }

    if addr_trimmed.chars().any(char::is_whitespace) {
        return Err(ConfigError::Validation(
            "server address cannot contain whitespace".to_string(),
        ));
    }

    let (host, inline_port) = split_host_and_port(addr_trimmed)?;

    let resolved_port = match (inline_port, port_trimmed.is_empty()) {
        (Some(_), false) => {
            return Err(ConfigError::Validation(
                "port specified in both address and port field".to_string(),
            ));
        }
        (Some(raw_port), true) => Some(parse_port(raw_port)?),
        (None, false) => Some(parse_port(port_trimmed)?),
        (None, true) => None,
    };

    let final_name = if name_trimmed.is_empty() {
        host.to_string()
    } else {
        name_trimmed.to_string()
    };

    Ok(ServerConfig {
        name: final_name,
        address: host.to_string(),
        port: resolved_port,
    })
}

fn split_host_and_port(addr: &str) -> Result<(&str, Option<&str>), ConfigError> {
    if let Some(after_open) = addr.strip_prefix('[') {
        let Some((inner, rest)) = after_open.split_once(']') else {
            return Err(ConfigError::Validation(
                "unclosed '[' in IPv6 address".to_string(),
            ));
        };

        if inner.is_empty() {
            return Err(ConfigError::Validation(
                "IPv6 address inside brackets cannot be empty".to_string(),
            ));
        }

        if inner.parse::<Ipv6Addr>().is_err() {
            return Err(ConfigError::Validation(format!(
                "invalid IPv6 address '{inner}'"
            )));
        }

        if rest.is_empty() {
            return Ok((inner, None));
        }

        let Some(port_str) = rest.strip_prefix(':') else {
            return Err(ConfigError::Validation(
                "unexpected characters after ']' in address".to_string(),
            ));
        };

        if port_str.is_empty() {
            return Err(ConfigError::Validation(
                "missing port after ':' in address".to_string(),
            ));
        }

        return Ok((inner, Some(port_str)));
    }

    if addr.contains('[') || addr.contains(']') {
        return Err(ConfigError::Validation(
            "unexpected bracket in server address".to_string(),
        ));
    }

    let colon_count = addr.bytes().filter(|&b| b == b':').count();
    match colon_count {
        0 => Ok((addr, None)),
        1 => {
            let (host, port_str) = addr
                .split_once(':')
                .ok_or_else(|| ConfigError::Validation("invalid host:port address".to_string()))?;
            if host.is_empty() {
                return Err(ConfigError::Validation(
                    "server address host cannot be empty".to_string(),
                ));
            }
            if port_str.is_empty() {
                return Err(ConfigError::Validation(
                    "missing port after ':' in address".to_string(),
                ));
            }
            Ok((host, Some(port_str)))
        }
        _ => {
            if addr.parse::<Ipv6Addr>().is_err() {
                return Err(ConfigError::Validation(format!(
                    "invalid IPv6 address '{addr}'"
                )));
            }
            Ok((addr, None))
        }
    }
}

fn parse_port(raw: &str) -> Result<u16, ConfigError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ConfigError::Validation(format!(
            "invalid port '{raw}': must be digits between 1 and 65535"
        )));
    }

    let port: u16 = raw.parse().map_err(|_| {
        ConfigError::Validation(format!("invalid port '{raw}': must be between 1 and 65535"))
    })?;

    if port == 0 {
        return Err(ConfigError::Validation(
            "invalid port '0': must be between 1 and 65535".to_string(),
        ));
    }

    Ok(port)
}
