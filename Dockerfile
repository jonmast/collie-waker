# syntax=docker/dockerfile:1.7

# Multi-stage build for collie-waker.
#
# Produces a distroless image containing:
#   - /usr/local/bin/collie-waker   (the waker + SSH tunnel supervisor)
#   - /usr/bin/ssh                  (for the tunnel to the dev box)
#   - /busybox/{sh,ls,cat,...}      (busybox applets for debugging)
#
# Build:    docker build -t ghcr.io/jonmast/collie-waker:dev .
# Push:     docker push ghcr.io/jonmast/collie-waker:dev
#
# Notes:
#   - Uses buildkit cache mounts for cargo registry/git/target.
#   - Sources: docker.io/library/*, gcr.io/distroless/*, busybox:stable.
#   - Final image is distroless cc-debian12:nonroot (UID 65532).

ARG RUST_VERSION=1.88
ARG DEBIAN_RELEASE=bookworm

# ---- build ------------------------------------------------------------------
FROM docker.io/library/rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS builder
WORKDIR /app
ENV CARGO_TERM_COLOR=always
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Cache mounts give proper layer-level caching for cargo registry/git/target
# without needing a separate dependency-recipe stage.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release --bins \
    && install -m 0755 target/release/collie-waker /usr/local/bin/collie-waker \
    && strip /usr/local/bin/collie-waker

# ---- ssh runtime payload (slim extract) -------------------------------------
FROM docker.io/library/debian:${DEBIAN_RELEASE}-slim AS ssh-runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /out \
    && cp /usr/bin/ssh /out/ssh \
    && cp -r /usr/lib/openssh /out/openssh \
    && cp -r /lib/x86_64-linux-gnu /out/lib-x86_64-linux-gnu

# ---- busybox for debugging (optional, exec via /busybox/sh) -------------------
FROM docker.io/library/busybox:stable-musl AS busybox
RUN mkdir -p /out/busybox /out/bin \
    && cp /bin/busybox /out/busybox/busybox \
    && for applet in $(busybox --list); do \
         ln -s busybox /out/busybox/$applet; \
         ln -s /busybox/$applet /out/bin/$applet 2>/dev/null || true; \
       done

# ---- final distroless image -------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder     /usr/local/bin/collie-waker  /usr/local/bin/collie-waker
COPY --from=busybox     /out/busybox                 /busybox
COPY --from=busybox     /out/bin                     /bin
COPY --from=ssh-runtime /out/ssh                     /usr/bin/ssh
COPY --from=ssh-runtime /out/openssh                 /usr/lib/openssh
# ssh needs libselinux/libkrb5/libfido2/libbsd/libmd which aren't in
# distroless. Overlay them; the existing libs in cc-debian12 stay in place.
COPY --from=ssh-runtime /out/lib-x86_64-linux-gnu/   /lib/x86_64-linux-gnu/

# 65532 is the distroless "nonroot" UID.
USER 65532:65532
EXPOSE 8080 8787

ENTRYPOINT ["/usr/local/bin/collie-waker"]
