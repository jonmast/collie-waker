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

## Why two pods, not one

One image, two deployments, selected by `ROLE`. The first cut ran both halves in a single pod with
two ports, which is simpler and was what the original `HelmRelease` contract described. It was
wrong.

The waker needs `hostNetwork` so its magic packet reaches the physical LAN. `hostNetwork` shares
the node's network namespace, so *every* port the pod binds is published on *every* interface of
the node. That put the SSH tunnel — a forward straight into Collie, which is arbitrary keystrokes
into a live terminal — on the LAN at `<node-ip>:8787`, bypassing Traefik, CrowdSec, and any
identity proxy in front of them. The only thing left guarding it was Collie's `X-Device-Id`
allowlist, whose value sat in plaintext in the Traefik middleware.

The whole premise of the design is that Collie binds loopback and has no LAN-exposed port. A
`hostNetwork` tunnel silently hands that back; it just moves the exposed port from the dev box to
the k8s node.

So the halves are split by blast radius: the waker keeps `hostNetwork` and can do nothing but fire
a rate-limited magic packet and redirect, while the tunnel — and the SSH key, mounted into that pod
alone — lives in an ordinary pod reachable only through its ClusterIP.

Both roles ship in one image because they share the config loader and the distroless-plus-`ssh`
Dockerfile inherited from `llm-wake-proxy`.

## Why there is no readiness probe

A failing readiness probe withdraws the pod's endpoint. For the waker that means Traefik has no
fallback at the exact moment the dev box is asleep — precisely when the fallback is the only thing
that can help. Liveness only, on both deployments.

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
