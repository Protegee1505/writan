//! Server configuration: environment variables only (ADR-0007 §10).

use std::path::PathBuf;

use crate::auth::LockoutPolicy;

/// Everything the server reads from the environment at startup.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to listen on (`WRITAN_BIND`, default `0.0.0.0`; an installer
    /// that terminates TLS in a reverse proxy sets `127.0.0.1`).
    pub bind: std::net::IpAddr,
    /// TCP port (`WRITAN_PORT`, default 8787).
    pub port: u16,
    /// Directory holding `writan-server.sqlite` (`WRITAN_DATA_DIR`, default
    /// `writan-data`).
    pub data_dir: PathBuf,
    /// Whether `POST /api/v1/accounts` is open (`WRITAN_REGISTRATION=closed`
    /// refuses new accounts).
    pub registration_open: bool,
    /// Largest accepted blob (`WRITAN_MAX_BLOB_KIB`, default 4096 KiB).
    pub max_blob_bytes: usize,
    /// Per-vault quota (`WRITAN_VAULT_QUOTA_MIB`, default 512 MiB; 0 =
    /// unlimited).
    pub vault_quota_bytes: u64,
    /// Pairing-code lifetime (`WRITAN_PAIRING_TTL_SECS`, default 600 s).
    pub pairing_ttl_secs: u64,
    /// Failed-login lockout shape (fixed policy, not configurable).
    pub lockout: LockoutPolicy,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 8787,
            data_dir: PathBuf::from("writan-data"),
            registration_open: true,
            max_blob_bytes: 4 * 1024 * 1024,
            vault_quota_bytes: 512 * 1024 * 1024,
            pairing_ttl_secs: 600,
            lockout: LockoutPolicy::default(),
        }
    }
}

impl ServerConfig {
    /// Read the environment; a malformed value is a startup error naming the
    /// variable (fail fast, never a silent default).
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// [`ServerConfig::from_env`] against an arbitrary lookup, so the
    /// parsing is testable without touching the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut config = Self::default();
        if let Some(value) = lookup("WRITAN_BIND") {
            config.bind = value.trim().parse().map_err(|_| {
                format!("WRITAN_BIND must be an address like 0.0.0.0 or 127.0.0.1, got `{value}`")
            })?;
        }
        if let Some(value) = lookup("WRITAN_PORT") {
            config.port = parse("WRITAN_PORT", &value)?;
        }
        if let Some(value) = lookup("WRITAN_DATA_DIR") {
            if value.trim().is_empty() {
                return Err("WRITAN_DATA_DIR must not be empty".to_string());
            }
            config.data_dir = PathBuf::from(value.trim());
        }
        if let Some(value) = lookup("WRITAN_REGISTRATION") {
            config.registration_open = match value.trim() {
                "open" => true,
                "closed" => false,
                other => {
                    return Err(format!(
                        "WRITAN_REGISTRATION must be `open` or `closed`, got `{other}`"
                    ));
                }
            };
        }
        if let Some(value) = lookup("WRITAN_MAX_BLOB_KIB") {
            let kib: u64 = parse("WRITAN_MAX_BLOB_KIB", &value)?;
            config.max_blob_bytes = usize::try_from(kib.saturating_mul(1024))
                .map_err(|_| "WRITAN_MAX_BLOB_KIB is too large".to_string())?;
        }
        if let Some(value) = lookup("WRITAN_VAULT_QUOTA_MIB") {
            let mib: u64 = parse("WRITAN_VAULT_QUOTA_MIB", &value)?;
            config.vault_quota_bytes = mib.saturating_mul(1024 * 1024);
        }
        if let Some(value) = lookup("WRITAN_PAIRING_TTL_SECS") {
            config.pairing_ttl_secs = parse("WRITAN_PAIRING_TTL_SECS", &value)?;
        }
        Ok(config)
    }
}

fn parse<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{name} must be a number, got `{value}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_match_the_documented_values() {
        let config = ServerConfig::from_lookup(lookup(&[])).expect("empty environment");
        assert_eq!(
            config.bind,
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(config.port, 8787);
        assert_eq!(config.data_dir, PathBuf::from("writan-data"));
        assert!(config.registration_open);
        assert_eq!(config.max_blob_bytes, 4 * 1024 * 1024);
        assert_eq!(config.vault_quota_bytes, 512 * 1024 * 1024);
        assert_eq!(config.pairing_ttl_secs, 600);
    }

    #[test]
    fn environment_overrides_parse() {
        let config = ServerConfig::from_lookup(lookup(&[
            ("WRITAN_BIND", "127.0.0.1"),
            ("WRITAN_PORT", "9000"),
            ("WRITAN_DATA_DIR", "/data"),
            ("WRITAN_REGISTRATION", "closed"),
            ("WRITAN_MAX_BLOB_KIB", "1024"),
            ("WRITAN_VAULT_QUOTA_MIB", "0"),
            ("WRITAN_PAIRING_TTL_SECS", "120"),
        ]))
        .expect("all valid");
        assert_eq!(
            config.bind,
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
        assert_eq!(config.port, 9000);
        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert!(!config.registration_open);
        assert_eq!(config.max_blob_bytes, 1024 * 1024);
        assert_eq!(config.vault_quota_bytes, 0);
        assert_eq!(config.pairing_ttl_secs, 120);
    }

    #[test]
    fn malformed_values_name_their_variable() {
        for (key, value) in [
            ("WRITAN_PORT", "soon"),
            ("WRITAN_BIND", "everywhere"),
            ("WRITAN_MAX_BLOB_KIB", "-1"),
            ("WRITAN_VAULT_QUOTA_MIB", "lots"),
            ("WRITAN_PAIRING_TTL_SECS", "later"),
        ] {
            let error =
                ServerConfig::from_lookup(lookup(&[(key, value)])).expect_err("must be refused");
            assert!(error.contains(key), "{error} should name {key}");
        }
        let error = ServerConfig::from_lookup(lookup(&[("WRITAN_REGISTRATION", "maybe")]))
            .expect_err("must be refused");
        assert!(error.contains("WRITAN_REGISTRATION"));
        let error = ServerConfig::from_lookup(lookup(&[("WRITAN_DATA_DIR", "  ")]))
            .expect_err("must be refused");
        assert!(error.contains("WRITAN_DATA_DIR"));
    }
}
