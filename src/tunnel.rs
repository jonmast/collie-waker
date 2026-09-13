use std::time::Duration;

use tokio::process::{Child, Command};

use crate::config::SshConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Builds the `ssh -N -L …` command for the tunnel to Collie.
///
/// The forward binds `ssh.tunnel_bind_addr` (not the pod loopback) so Traefik's
/// primary upstream and its health check can reach Collie through the Service.
/// Binding anything other than loopback requires `GatewayPorts=yes`.
pub fn tunnel_command(config: &SshConfig) -> Command {
    let mut command = Command::new("ssh");
    command
        .kill_on_drop(true)
        .arg("-N")
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "GatewayPorts=yes"])
        .args(["-o", "ServerAliveInterval=15"])
        .args(["-o", "ServerAliveCountMax=3"])
        .args([
            "-o",
            &format!("StrictHostKeyChecking={}", config.strict_host_key_checking),
        ])
        .args([
            "-o",
            &format!("UserKnownHostsFile={}", config.known_hosts_path),
        ])
        .args(["-i", &config.key_path])
        .args(["-p", &config.port.to_string()])
        .args([
            "-L",
            &format!(
                "{}:{}:127.0.0.1:{}",
                config.tunnel_bind_addr, config.tunnel_local_port, config.collie_server_port
            ),
        ])
        .arg(format!("{}@{}", config.user, config.host));

    command
}

fn spawn_tunnel(config: &SshConfig) -> std::io::Result<Child> {
    tunnel_command(config).spawn()
}

/// Keeps the SSH tunnel up for the lifetime of the pod, reconnecting with
/// exponential backoff. The dev box being asleep is the normal case, so a
/// failure here is expected rather than fatal.
pub async fn supervise(config: SshConfig) {
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match spawn_tunnel(&config) {
            Ok(mut child) => {
                tracing::info!(
                    host = %config.host,
                    bind = %config.tunnel_bind_addr,
                    port = config.tunnel_local_port,
                    "ssh tunnel started"
                );

                match child.wait().await {
                    Ok(status) => tracing::warn!(%status, "ssh tunnel exited"),
                    Err(error) => tracing::warn!(%error, "failed to wait on ssh tunnel"),
                }
            }
            Err(error) => tracing::warn!(%error, "failed to spawn ssh tunnel"),
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SshConfig {
        SshConfig {
            host: "192.0.2.10".to_string(),
            user: "collie".to_string(),
            port: 22,
            key_path: "/home/collie/.ssh/ssh-privatekey".to_string(),
            known_hosts_path: "/home/collie/.ssh/known_hosts".to_string(),
            strict_host_key_checking: "accept-new".to_string(),
            tunnel_bind_addr: "0.0.0.0".to_string(),
            tunnel_local_port: 8787,
            collie_server_port: 8787,
        }
    }

    fn rendered_args(config: &SshConfig) -> Vec<String> {
        tunnel_command(config)
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn forwards_the_bind_address_to_collie_loopback() {
        let args = rendered_args(&config());

        assert!(args.contains(&"0.0.0.0:8787:127.0.0.1:8787".to_string()));
        assert!(args.contains(&"collie@192.0.2.10".to_string()));
    }

    #[test]
    fn enables_gateway_ports_for_non_loopback_binds() {
        assert!(rendered_args(&config()).contains(&"GatewayPorts=yes".to_string()));
    }

    #[test]
    fn uses_the_mounted_key_and_known_hosts() {
        let args = rendered_args(&config());

        assert!(args.contains(&"/home/collie/.ssh/ssh-privatekey".to_string()));
        assert!(args.contains(&"UserKnownHostsFile=/home/collie/.ssh/known_hosts".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
    }

    #[test]
    fn honours_a_non_default_ssh_port() {
        let mut config = config();
        config.port = 2222;

        let args = rendered_args(&config);
        let port_flag = args.iter().position(|arg| arg == "-p").unwrap();
        assert_eq!(args[port_flag + 1], "2222");
    }
}
