# collie-waker

Wake-on-LAN "waker" that keeps [Collie](https://github.com/AltanS/collie) reachable at a single
stable URL — `https://collie.example.com` — even when the dev box it runs on is asleep.

Collie binds `127.0.0.1:8787` on the dev box (`192.0.2.10`, the same machine
[`llm-wake-proxy`](https://github.com/jonmast/llm-wake-proxy) wakes), so there is no LAN-exposed
port. This pod provides both halves of the answer: the **SSH tunnel** the cluster reaches Collie
through, and the **waker** that fires a magic packet when the box is asleep.

## Request flow

```
phone (PWA) ──HTTPS──▶ Traefik ──▶ collie-failover (TraefikService)
                                     ├─ primary  : SSH tunnel ▶ Collie @ dev box 127.0.0.1:8787
                                     └─ fallback : waker pod (fires WoL, polls, 307s back)
```

Traefik health-checks the primary on `/api/snapshot`. While the box is up, that check passes and
traffic flows through the tunnel to Collie. When the box is asleep the tunnel is down, the check
fails, and Traefik fails over to the waker, which:

1. Sends a Wake-on-LAN magic packet to `wol.macAddress`, rate-limited to at most one packet per
   `waker.wolRateLimitSeconds`.
2. Polls `waker.pollUrl` through the tunnel, sending `Host: waker.publicHost` so Collie's host
   validation passes.
3. Waits `waker.healthIntervalSeconds` — one Traefik health-check interval — so the failover has
   flipped back to the primary.
4. Issues a `307` back to the same URL, which now reaches Collie itself.
5. On `waker.timeoutSeconds` expiry, serves a `503` auto-reload page instead.

Both the tunnel and the waker live in one container: the Service exposes port `8787` (tunnel,
Traefik's primary) and `8080` (waker HTTP, the fallback).

## Configuration

Every value is read from the environment; the Helm chart renders all of them from its `waker`,
`wol`, and `ssh` value blocks.

| Env var | Default | Meaning |
| --- | --- | --- |
| `PORT` | `8080` | Waker HTTP listen port |
| `PUBLIC_HOST` | *required* | Host sent when polling Collie, and the 307 target |
| `POLL_URL` | `http://127.0.0.1:8787/api/snapshot` | Where Collie is polled, through the tunnel |
| `TIMEOUT_SECONDS` | `45` | Wake budget before the 503 page |
| `HEALTH_INTERVAL_SECONDS` | `5` | One Traefik health-check interval |
| `POLL_INTERVAL_MS` | `1000` | Delay between polls |
| `WOL_MAC_ADDRESS` | *required* | Target MAC, `XX:XX:XX:XX:XX:XX` |
| `WOL_BROADCAST_ADDR` | `255.255.255.255` | Broadcast address |
| `WOL_PORT` | `9` | Magic packet UDP port |
| `WOL_RATE_LIMIT_SECONDS` | `10` | Minimum gap between magic packets |
| `SSH_HOST` / `SSH_USER` | *required* | Dev box and tunnel user |
| `SSH_PORT` | `22` | |
| `SSH_MOUNT_PATH` | `/home/collie/.ssh` | Where the key Secret is mounted |
| `SSH_KEY_PATH` | `$SSH_MOUNT_PATH/ssh-privatekey` | |
| `SSH_KNOWN_HOSTS_PATH` | `$SSH_MOUNT_PATH/known_hosts` | |
| `SSH_STRICT_HOST_KEY_CHECKING` | `accept-new` | |
| `TUNNEL_BIND_ADDR` | `0.0.0.0` | Interface the forward binds (see below) |
| `TUNNEL_LOCAL_PORT` | `8787` | Local side of the forward |
| `COLLIE_SERVER_PORT` | `8787` | Collie's port on the dev box loopback |
| `RUST_LOG` | — | `tracing` filter, e.g. `info` |

`GET /healthz` is reserved for the kubelet liveness probe and never triggers a wake. Every other
path is a wake request.

### Why the tunnel binds `0.0.0.0`

`llm-wake-proxy` forwards to the pod loopback, which is enough for a proxy running in the same
process. Here Traefik must reach the tunnel *through the Service*, so the forward binds
`ssh.tunnelBindAddr` instead — which requires `GatewayPorts=yes` on the `ssh` invocation.

## Deployment

The chart lives at `./charts/collie-waker` and is consumed by a Flux `HelmRelease` in
[`jonmast/k8s-conf`](https://github.com/jonmast/k8s-conf) (`apps/collie-waker/`), which also owns
the namespace, the SOPS-encrypted SSH key, and the Traefik failover routes.

The pod runs with `hostNetwork: true` so the UDP broadcast actually leaves the node, and with a
**liveness probe only**. A readiness probe is deliberately omitted: this pod's Service carries both
the tunnel and the waker, so a failing readiness check would withdraw *all* endpoints and take the
Traefik primary down along with the fallback.

```sh
make lint      # cargo check + clippy + helm lint
make test      # cargo test
make render    # helm template to stdout
make build     # docker build
```

CI publishes `ghcr.io/jonmast/collie-waker` on pushes to `main` (`:latest`, `:sha-<sha>`) and on
`v*.*.*` tags (semver). Pin the HelmRelease by digest.

## Prerequisites on the dev box

1. Collie configured with `COLLIE_HOST=127.0.0.1`, `COLLIE_PORT=8787`, and
   `COLLIE_PUBLIC_HOSTS=collie.example.com`.
2. The waker's public key appended to `~<ssh-user>/.ssh/authorized_keys`.
3. `known_hosts` populated from `ssh-keyscan -H 192.0.2.10`.
4. Wake-on-LAN enabled — already proven on this box via `llm-wake-proxy`.
