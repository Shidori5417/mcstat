//! Persistent configuration (TOML).

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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "mcstat-test-{label}-{}-{nanos}-{seq}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_config_round_trip() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.refresh_interval_secs, DEFAULT_REFRESH_SECS);
        assert_eq!(cfg.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(cfg.servers.len(), 3);
        assert_eq!(cfg.servers[0].name, "Hypixel");
        assert_eq!(cfg.servers[0].address, "mc.hypixel.net");
        assert_eq!(cfg.servers[0].port, None);
        assert_eq!(cfg.servers[1].name, "CubeCraft");
        assert_eq!(cfg.servers[1].address, "play.cubecraft.net");
        assert_eq!(cfg.servers[1].port, None);
        assert_eq!(cfg.servers[2].name, "2b2t");
        assert_eq!(cfg.servers[2].address, "2b2t.org");
        assert_eq!(cfg.servers[2].port, None);

        let serialized = toml::to_string_pretty(&cfg).expect("serialize default config");
        let parsed: AppConfig = toml::from_str(&serialized).expect("deserialize default config");
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let empty: AppConfig = toml::from_str("").expect("parse empty toml");
        assert_eq!(empty.refresh_interval_secs, DEFAULT_REFRESH_SECS);
        assert_eq!(empty.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(empty.servers.is_empty());

        let partial: AppConfig = toml::from_str(
            r#"
refresh_interval_secs = 10

[[servers]]
name = "Local"
address = "localhost"
"#,
        )
        .expect("parse partial toml");
        assert_eq!(partial.refresh_interval_secs, 10);
        assert_eq!(partial.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(partial.servers.len(), 1);
        assert_eq!(partial.servers[0].port, None);
    }

    #[test]
    fn load_or_create_creates_missing_file_and_parents() {
        let tmp = TempDir::new("config-load-create");
        let cfg_path = tmp.path().join("nested").join("mcstat").join("config.toml");
        assert!(!cfg_path.exists());

        let loaded = AppConfig::load_or_create(&cfg_path).expect("load_or_create");
        assert_eq!(loaded, AppConfig::default());
        assert!(cfg_path.exists());

        let reloaded = AppConfig::load_or_create(&cfg_path).expect("reload existing");
        assert_eq!(reloaded, loaded);
    }

    #[test]
    fn save_then_load_round_trip() {
        let tmp = TempDir::new("config-save-load");
        let cfg_path = tmp.path().join("config.toml");

        let custom = AppConfig {
            refresh_interval_secs: 15,
            timeout_secs: 7,
            servers: vec![
                ServerConfig {
                    name: "Survival".to_string(),
                    address: "play.example.com".to_string(),
                    port: Some(25566),
                },
                ServerConfig {
                    name: "Creative".to_string(),
                    address: "::1".to_string(),
                    port: None,
                },
            ],
        };

        custom.save(&cfg_path).expect("save custom config");
        assert!(!tmp.path().join("config.toml.tmp").exists());

        let loaded = AppConfig::load_or_create(&cfg_path).expect("load saved config");
        assert_eq!(loaded, custom);
    }

    #[test]
    fn invalid_toml_returns_parse_error_with_path() {
        let tmp = TempDir::new("config-invalid-toml");
        let cfg_path = tmp.path().join("config.toml");
        fs::write(&cfg_path, "refresh_interval_secs = [not valid toml").expect("write bad toml");

        let err = AppConfig::load_or_create(&cfg_path).expect_err("should fail with Parse");
        match err {
            ConfigError::Parse { path, .. } => assert_eq!(path, cfg_path),
            other => panic!("expected ConfigError::Parse, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_file_is_reported_and_left_untouched() {
        let tmp = TempDir::new("config-unparseable");
        let cfg_path = tmp.path().join("config.toml");
        let original = "servers = \"not an array\"\n";
        fs::write(&cfg_path, original).expect("write");

        let err = AppConfig::load_or_create(&cfg_path).expect_err("type mismatch");
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        assert_eq!(fs::read_to_string(&cfg_path).expect("read back"), original);
    }

    #[test]
    fn non_utf8_garbage_fails_while_reading_and_is_not_overwritten() {
        // Current behaviour: invalid UTF-8 is an I/O error (read stage), not a
        // TOML parse error. Either way the file is never replaced.
        let tmp = TempDir::new("config-garbage");
        let cfg_path = tmp.path().join("config.toml");
        let garbage: &[u8] = &[0xff, 0xfe, 0x00, 0x81, b'[', 0xc3];
        fs::write(&cfg_path, garbage).expect("write garbage");

        let err = AppConfig::load_or_create(&cfg_path).expect_err("garbage");
        match err {
            ConfigError::Io { path, source } => {
                assert_eq!(path, cfg_path);
                assert_eq!(source.kind(), std::io::ErrorKind::InvalidData);
            }
            other => panic!("expected ConfigError::Io, got {other:?}"),
        }
        assert_eq!(fs::read(&cfg_path).expect("read back"), garbage);
    }

    #[test]
    fn path_that_is_a_directory_is_an_io_error() {
        let tmp = TempDir::new("config-is-dir");
        let dir_path = tmp.path().join("config.toml");
        fs::create_dir(&dir_path).expect("create dir");

        let err = AppConfig::load_or_create(&dir_path).expect_err("directory is not a file");
        match err {
            ConfigError::Io { path, .. } => assert_eq!(path, dir_path),
            other => panic!("expected ConfigError::Io, got {other:?}"),
        }
        assert!(dir_path.is_dir(), "directory must not be replaced");
    }

    #[test]
    fn empty_existing_file_loads_with_no_servers() {
        // Only a *missing* file gets the default server list; an existing empty
        // file is valid TOML and yields the field defaults with no servers.
        let tmp = TempDir::new("config-empty");
        let cfg_path = tmp.path().join("config.toml");
        fs::write(&cfg_path, "").expect("write empty");

        let loaded = AppConfig::load_or_create(&cfg_path).expect("empty is valid");
        assert_eq!(loaded.refresh_interval_secs, DEFAULT_REFRESH_SECS);
        assert_eq!(loaded.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(loaded.servers.is_empty());
        assert_eq!(fs::read_to_string(&cfg_path).expect("read back"), "");
    }

    #[test]
    fn unknown_keys_are_ignored_and_dropped_on_save() {
        // Current behaviour: no `deny_unknown_fields`, so a misspelled key is
        // silently ignored and falls back to its default.
        let tmp = TempDir::new("config-unknown-keys");
        let cfg_path = tmp.path().join("config.toml");
        fs::write(
            &cfg_path,
            r#"
refresh_interval = 30
timeout_secs = 7

[[servers]]
name = "Typo"
address = "mc.example.com"
motd = "ignored"
"#,
        )
        .expect("write");

        let loaded = AppConfig::load_or_create(&cfg_path).expect("unknown keys are not an error");
        assert_eq!(loaded.refresh_interval_secs, DEFAULT_REFRESH_SECS);
        assert_eq!(loaded.timeout_secs, 7);
        assert_eq!(
            loaded.servers,
            vec![ServerConfig {
                name: "Typo".into(),
                address: "mc.example.com".into(),
                port: None,
            }]
        );

        // Saving rewrites the file, so the unknown keys disappear.
        loaded.save(&cfg_path).expect("save");
        let rewritten = fs::read_to_string(&cfg_path).expect("read back");
        assert!(!rewritten.contains("refresh_interval ="), "{rewritten}");
        assert!(!rewritten.contains("motd"), "{rewritten}");
    }

    #[test]
    fn explicit_ports_are_written_and_unset_ports_are_omitted() {
        let tmp = TempDir::new("config-ports");
        let cfg_path = tmp.path().join("config.toml");
        let cfg = AppConfig {
            servers: vec![
                ServerConfig {
                    name: "Explicit".into(),
                    address: "a.example.net".into(),
                    port: Some(25570),
                },
                ServerConfig {
                    name: "FromSrv".into(),
                    address: "b.example.net".into(),
                    port: None,
                },
            ],
            ..AppConfig::default()
        };

        cfg.save(&cfg_path).expect("save");
        let text = fs::read_to_string(&cfg_path).expect("read back");
        assert!(text.contains("port = 25570"), "{text}");
        assert_eq!(text.matches("port =").count(), 1, "{text}");
        assert_eq!(AppConfig::load_or_create(&cfg_path).expect("load"), cfg);
    }

    #[test]
    fn unicode_and_escaped_server_names_round_trip_through_disk() {
        let tmp = TempDir::new("config-unicode");
        let cfg_path = tmp.path().join("config.toml");
        let cfg = AppConfig {
            servers: vec![
                ServerConfig {
                    name: "サーバー 🚀 Ünïcödé".into(),
                    address: "jp.example.net".into(),
                    port: None,
                },
                ServerConfig {
                    name: "Ελληνικά ✓ \"quoted\" back\\slash".into(),
                    address: "gr.example.net".into(),
                    port: Some(25565),
                },
            ],
            ..AppConfig::default()
        };

        cfg.save(&cfg_path).expect("save");
        let text = fs::read_to_string(&cfg_path).expect("utf-8 file");
        assert!(text.contains("サーバー 🚀 Ünïcödé"), "{text}");
        assert_eq!(AppConfig::load_or_create(&cfg_path).expect("load"), cfg);
    }

    #[test]
    fn save_fails_when_parent_is_regular_file() {
        let tmp = TempDir::new("config-unwritable");
        let blocking_file = tmp.path().join("not-a-dir");
        fs::write(&blocking_file, "file").expect("write blocking file");
        let cfg_path = blocking_file.join("config.toml");

        let err = AppConfig::default()
            .save(&cfg_path)
            .expect_err("save should fail");
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn default_path_ends_with_mcstat_config_toml() {
        if let Ok(path) = AppConfig::default_path() {
            assert!(path.ends_with(Path::new("mcstat").join("config.toml")));
        }
    }

    #[test]
    fn validate_server_input_valid_cases() {
        // Basic with trimming and empty port
        let s = validate_server_input("  Hypixel  ", "  mc.hypixel.net  ", "   ")
            .expect("valid basic server");
        assert_eq!(
            s,
            ServerConfig {
                name: "Hypixel".to_string(),
                address: "mc.hypixel.net".to_string(),
                port: None,
            }
        );

        // Empty name falls back to address
        let s = validate_server_input("   ", "play.example.org", "").expect("fallback name");
        assert_eq!(s.name, "play.example.org");
        assert_eq!(s.address, "play.example.org");
        assert_eq!(s.port, None);

        // Explicit port boundaries 1 and 65535
        let s = validate_server_input("Min", "example.org", "1").expect("port 1");
        assert_eq!(s.port, Some(1));
        let s = validate_server_input("Max", "example.org", "65535").expect("port 65535");
        assert_eq!(s.port, Some(65535));

        // Inline host:port when port field is empty; empty name uses stripped host
        let s = validate_server_input("", "mc.example.com:25570", "").expect("inline host:port");
        assert_eq!(
            s,
            ServerConfig {
                name: "mc.example.com".to_string(),
                address: "mc.example.com".to_string(),
                port: Some(25570),
            }
        );

        // Bracketed IPv6 with inline port strips brackets
        let s = validate_server_input("Local6", "[::1]:25565", "").expect("bracketed ipv6 + port");
        assert_eq!(
            s,
            ServerConfig {
                name: "Local6".to_string(),
                address: "::1".to_string(),
                port: Some(25565),
            }
        );

        // Bracketed IPv6 without inline port + fallback name
        let s = validate_server_input("", "[::1]", "").expect("bracketed ipv6 no port");
        assert_eq!(
            s,
            ServerConfig {
                name: "::1".to_string(),
                address: "::1".to_string(),
                port: None,
            }
        );

        // Bracketed IPv6 without inline port + separate port field
        let s = validate_server_input("Local6", "[2001:db8::1]", "25566")
            .expect("bracketed ipv6 + field port");
        assert_eq!(
            s,
            ServerConfig {
                name: "Local6".to_string(),
                address: "2001:db8::1".to_string(),
                port: Some(25566),
            }
        );

        // Bare IPv6 without port
        let s = validate_server_input("Loopback", "::1", "").expect("bare ipv6");
        assert_eq!(
            s,
            ServerConfig {
                name: "Loopback".to_string(),
                address: "::1".to_string(),
                port: None,
            }
        );

        // Bare IPv6 with explicit port field
        let s = validate_server_input("Loopback", "2001:db8::1", "25565")
            .expect("bare ipv6 + field port");
        assert_eq!(
            s,
            ServerConfig {
                name: "Loopback".to_string(),
                address: "2001:db8::1".to_string(),
                port: Some(25565),
            }
        );
    }

    #[test]
    fn validate_server_input_invalid_cases() {
        let invalid_cases = [
            // Empty / whitespace-only address
            ("Name", "", ""),
            ("Name", "   ", ""),
            // Whitespace inside address
            ("Name", "mc.hypixel. net", ""),
            ("Name", "mc.hypixel.\tnet", ""),
            // Invalid ports in port field
            ("Name", "mc.hypixel.net", "0"),
            ("Name", "mc.hypixel.net", "65536"),
            ("Name", "mc.hypixel.net", "999999"),
            ("Name", "mc.hypixel.net", "abc"),
            ("Name", "mc.hypixel.net", "-1"),
            ("Name", "mc.hypixel.net", "+25565"),
            // Invalid inline ports
            ("Name", "mc.hypixel.net:0", ""),
            ("Name", "mc.hypixel.net:65536", ""),
            ("Name", "mc.hypixel.net:abc", ""),
            ("Name", "mc.hypixel.net:", ""),
            ("Name", ":25565", ""),
            // Port specified in both address and port field
            ("Name", "mc.hypixel.net:25565", "25565"),
            ("Name", "mc.hypixel.net:25565", "25566"),
            ("Name", "[::1]:25565", "25565"),
            // Malformed IPv6 / brackets
            ("Name", "[::1", ""),
            ("Name", "[]:25565", ""),
            ("Name", "[::1]:", ""),
            ("Name", "[::1]extra", ""),
            ("Name", "[not-ipv6]:25565", ""),
            ("Name", "foo]bar", ""),
            ("Name", "foo[bar", ""),
            ("Name", "a:b:c", ""),
        ];

        for (name, addr, port) in invalid_cases {
            let err = validate_server_input(name, addr, port).expect_err(&format!(
                "expected Validation error for ({name:?}, {addr:?}, {port:?})"
            ));
            assert!(
                matches!(err, ConfigError::Validation(_)),
                "expected Validation for ({name:?}, {addr:?}, {port:?}), got {err:?}"
            );
        }
    }
}
