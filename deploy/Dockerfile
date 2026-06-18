# fauxx-desktop: Fauxx Desktop Companion
# Copyright (C) 2026 Digital Grease
#
# This program is free software: you can redistribute it and/or modify it under
# the terms of the GNU Affero General Public License as published by the Free
# Software Foundation, either version 3 of the License, or (at your option) any
# later version. See <https://www.gnu.org/licenses/>.
#
# Container image for the Fauxx headless homelab mode (C8 #35). Multi-stage:
# build the `fauxx` CLI with the pinned toolchain, then ship a slim runtime that
# runs `fauxx serve` as a NON-root user. Build with:
#
#   docker build -f deploy/Dockerfile -t fauxx-desktop .
#
# To include the Home Assistant MQTT bridge, pass the build arg:
#
#   docker build -f deploy/Dockerfile --build-arg FEATURES=mqtt -t fauxx-desktop .

# --- build stage ----------------------------------------------------------
# The toolchain is pinned in rust-toolchain.toml (1.96.0); this base tag tracks
# a recent stable, and rustup honors the pinned channel inside the workspace.
FROM rust:1.96-bookworm AS build

# The encrypted store vendors SQLCipher + OpenSSL (bundled-sqlcipher-vendored-
# openssl), so the build needs a C toolchain and perl (for the OpenSSL build).
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential perl pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# Optional cargo features (e.g. `mqtt`). Empty by default.
ARG FEATURES=""
RUN if [ -n "$FEATURES" ]; then \
      cargo build --release -p fauxx-cli --features "$FEATURES"; \
    else \
      cargo build --release -p fauxx-cli; \
    fi

# --- runtime stage --------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Only the CA certs are needed at runtime (no system SQLCipher: it is statically
# vendored into the binary). Everything else is the static-ish Rust binary.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system fauxx \
    && useradd --system --gid fauxx --home-dir /var/lib/fauxx --shell /usr/sbin/nologin fauxx \
    && mkdir -p /var/lib/fauxx /etc/fauxx \
    && chown -R fauxx:fauxx /var/lib/fauxx

COPY --from=build /src/target/release/fauxx /usr/local/bin/fauxx

# The store, key file, and passphrase file live here (mount a volume).
VOLUME ["/var/lib/fauxx"]

ENV RUST_LOG=info

USER fauxx
WORKDIR /var/lib/fauxx

# The serve config is bind-mounted at /etc/fauxx/serve.json by the operator
# (see deploy/serve.example.json and deploy/docker-compose.yml).
ENTRYPOINT ["/usr/local/bin/fauxx", "serve", "--config", "/etc/fauxx/serve.json"]
