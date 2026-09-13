# Context: collie-waker

## Glossary

**Waker**
The HTTP handler on port 8080. It is Traefik's *fallback* upstream, so it only ever sees traffic
when the dev box is asleep. Every request it receives is, by definition, evidence that the box
needs waking. It has no notion of routes: any path other than `/healthz` triggers the wake
sequence and is then redirected verbatim.

**Tunnel**
The `ssh -N -L` forward on port 8787. It is Traefik's *primary* upstream. It is supervised for the
lifetime of the pod, independent of any wake request, and reconnects with exponential backoff
(1s → 30s). While the dev box is asleep the tunnel is **expected** to be failing; that is the
signal Traefik's health check reads, not an error condition.

**Failover flip**
Traefik does not learn the primary has recovered until its next health check. So after Collie
answers, the waker sleeps `HEALTH_INTERVAL_SECONDS` before redirecting. Skipping this wait sends
the client straight back to the waker, producing a redirect loop.

**Wake budget**
`TIMEOUT_SECONDS`, the total time the waker will hold a request open. On expiry the client gets a
`503` carrying a `<meta http-equiv="refresh">` page rather than an error, so a phone left on the
tab retries on its own.

**Magic packet rate limit**
`WOL_RATE_LIMIT_SECONDS`. A PWA typically fires several parallel requests (snapshot, icons, API),
each of which enters the wake sequence and would otherwise broadcast its own packet. The limiter
is shared process-wide, so a burst of requests produces one packet.

## Why one container, not a sidecar

The tunnel and the waker are two ports on one pod. A sidecar split was considered and rejected:
the `HelmRelease` contract in `k8s-conf` declares a single `controllers.main.containers.main` with
both ports, one `ssh.*` value block, and one SSH key mount. Running the tunnel in-process also
reuses `llm-wake-proxy`'s Dockerfile work of getting `/usr/bin/ssh` into a distroless image.

## Why there is no readiness probe

The Service exposes both the tunnel (primary) and the waker (fallback), and readiness is
pod-scoped, not port-scoped. A failing readiness probe on the waker's HTTP port would remove the
pod from *both* endpoint sets, taking down the primary at the exact moment the fallback was also
struggling. Liveness only.

## Relationship to llm-wake-proxy

Same dev box, same MAC, same SSH tunnel pattern, same chart and CI conventions. The WoL magic
packet construction and the `ssh` invocation are lifted from `llm-wake-proxy`'s `src/host.rs`.

The projects differ in what they do once the box is awake: `llm-wake-proxy` *proxies* requests
through itself for the session's lifetime and manages `llama-server` units over an SSH helper
binary. `collie-waker` holds no traffic — it redirects and gets out of the way, because Traefik's
failover, not this process, owns the routing decision. There is no host-side helper binary.

## Traefik

The cluster runs Traefik 3.7.8 (k3s bundled, configured via
`infrastructure/traefik/config.yaml` in `k8s-conf`). This supports both the `failover`
TraefikService and the `hostname` field on a service health check, which the routes in
`apps/collie-waker/routes.yaml` depend on.
