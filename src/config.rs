use std::env;
use std::time::Duration;

use crate::wake::WakeTimings;
use crate::wol::parse_mac;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub listen_port: u16,
    pub waker: WakerConfig,
    pub wol: WolConfig,
    pub ssh: SshConfig,
}

#[derive(Clone, Debug)]
pub struct WakerConfig {
    /// Host Collie validates against; also the target of the 307.
    pub public_host: String,
    /// Where Collie is polled, through the SSH tunnel.
    pub poll_url: String,
    pub timeout: Duration,
    /// One Traefik health-check interval.
    pub health_interval: Duration,
    pub poll_interval: Duration,
}

impl WakerConfig {
    pub fn timings(&self) -> WakeTimings {
        WakeTimings {
            timeout: self.timeout,
            health_interval: self.health_interval,
            poll_interval: self.poll_interval,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WolConfig {
    pub mac: [u8; 6],
    pub broadcast: String,
    pub port: u16,
    pub rate_limit: Duration,
}

#[derive(Clone, Debug)]
pub struct SshConfig {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub key_path: String,
    pub known_hosts_path: String,
    pub strict_host_key_checking: String,
    pub tunnel_bind_addr: String,
    pub tunnel_local_port: u16,
    pub collie_server_port: u16,
}

impl AppConfig {
    /// Reads configuration from the environment. The Helm chart renders every
    /// one of these from the `waker`, `wol`, and `ssh` value blocks.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mount_path = read_var("SSH_MOUNT_PATH").unwrap_or_else(|| "/home/collie/.ssh".into());

        Ok(Self {
            listen_port: parse_var("PORT")?.unwrap_or(8080),
            waker: WakerConfig {
                public_host: require_var("PUBLIC_HOST")?,
                poll_url: read_var("POLL_URL")
                    .unwrap_or_else(|| "http://127.0.0.1:8787/api/snapshot".into()),
                timeout: seconds("TIMEOUT_SECONDS", 45)?,
                health_interval: seconds("HEALTH_INTERVAL_SECONDS", 5)?,
                poll_interval: millis("POLL_INTERVAL_MS", 1000)?,
            },
            wol: WolConfig {
                mac: parse_mac(&require_var("WOL_MAC_ADDRESS")?)
                    .map_err(|e| ConfigError(e.to_string()))?,
                broadcast: read_var("WOL_BROADCAST_ADDR")
                    .unwrap_or_else(|| "255.255.255.255".into()),
                port: parse_var("WOL_PORT")?.unwrap_or(9),
                rate_limit: seconds("WOL_RATE_LIMIT_SECONDS", 10)?,
            },
            ssh: SshConfig {
                host: require_var("SSH_HOST")?,
                user: require_var("SSH_USER")?,
                port: parse_var("SSH_PORT")?.unwrap_or(22),
                key_path: read_var("SSH_KEY_PATH")
                    .unwrap_or_else(|| format!("{mount_path}/ssh-privatekey")),
                known_hosts_path: read_var("SSH_KNOWN_HOSTS_PATH")
                    .unwrap_or_else(|| format!("{mount_path}/known_hosts")),
                strict_host_key_checking: read_var("SSH_STRICT_HOST_KEY_CHECKING")
                    .unwrap_or_else(|| "accept-new".into()),
                tunnel_bind_addr: read_var("TUNNEL_BIND_ADDR").unwrap_or_else(|| "0.0.0.0".into()),
                tunnel_local_port: parse_var("TUNNEL_LOCAL_PORT")?.unwrap_or(8787),
                collie_server_port: parse_var("COLLIE_SERVER_PORT")?.unwrap_or(8787),
            },
        })
    }
}

#[derive(Debug)]
pub struct ConfigError(String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn read_var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn require_var(name: &str) -> Result<String, ConfigError> {
    read_var(name).ok_or_else(|| ConfigError(format!("{name} is required")))
}

fn parse_var<T: std::str::FromStr>(name: &str) -> Result<Option<T>, ConfigError> {
    match read_var(name) {
        None => Ok(None),
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| ConfigError(format!("{name} is not a valid value: {value}"))),
    }
}

fn seconds(name: &str, default: u64) -> Result<Duration, ConfigError> {
    Ok(Duration::from_secs(parse_var(name)?.unwrap_or(default)))
}

fn millis(name: &str, default: u64) -> Result<Duration, ConfigError> {
    Ok(Duration::from_millis(parse_var(name)?.unwrap_or(default)))
}
