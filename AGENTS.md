# AGENTS.md

## Project

Rust binary + Helm chart. The binary maintains an SSH tunnel to the dev box and serves the
Wake-on-LAN fallback handler; the chart at `charts/collie-waker` is consumed by a Flux
`HelmRelease` in `jonmast/k8s-conf` (`apps/collie-waker/`).

`charts/collie-waker/values.yaml` is a **contract** with that HelmRelease. Before renaming or
removing anything under the `waker`, `wol`, `ssh`, `image`, `controllers`, `service`, or
`persistence` blocks, check `apps/collie-waker/release.yaml` in `k8s-conf` and change both
together.

## Checks

```sh
make lint    # cargo check + cargo clippy -D warnings + helm lint
make test    # cargo test
make render  # helm template to stdout
```

`helm lint` alone will not catch a bad value template — the bjw-s common chart only templates
certain fields, and errors surface as an `Error:` string embedded in rendered YAML. Always run
`make render` and read the output after touching `values.yaml`.

## Conventions

Mirror [`llm-wake-proxy`](https://github.com/jonmast/llm-wake-proxy): same Dockerfile shape
(distroless + extracted `ssh`), Makefile targets, workflow layout, and doc set. Reuse its WoL and
SSH code rather than reinventing.

Side-effecting work sits behind small traits (`MagicPacketSender`, `ColliePoller`, `WakeService`)
so the wake sequence is tested with `tokio::time` paused rather than real sleeps. Keep it that
way: no test should broadcast a real packet or wait a real second.

## Agent skills

### Issue tracker

GitHub Issues at `jonmast/collie-waker`, managed via the `gh` CLI.

### Domain docs

Single-context: `CONTEXT.md` at the repo root.
