# syntax=docker.io/docker/dockerfile-upstream:1.23.0

ARG RUST_VERSION=1.95.0
FROM rust:1.95.0-trixie@sha256:39d8cb39a54e7d1da665c4fabfdd265e532a5f836c11ab5aee27fd5c73891ce4 AS builder

ARG AGENTACTR_VERSION=0.1.0
ARG TARGET=aarch64-unknown-linux-musl

WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils \
        file \
        make \
        musl-dev \
        musl-tools \
        perl \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add "${TARGET}"

ENV CARGO_TERM_COLOR=always \
    CC_aarch64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    RUSTFLAGS="-C target-feature=+crt-static"

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --locked --release --bin agentactr --target "${TARGET}"

FROM builder AS verify
ARG TARGET=aarch64-unknown-linux-musl

RUN set -eux; \
    bin="/src/target/${TARGET}/release/agentactr"; \
    file "${bin}"; \
    readelf -l "${bin}" > /tmp/agentactr.readelf; \
    ! grep -q 'INTERP' /tmp/agentactr.readelf; \
    ldd "${bin}" 2>&1 | grep -E 'not a dynamic executable|statically linked'

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e3f945647ffb95b5839c07038d64f9811adf17308b9121d8a2b87b6a22a80a39

LABEL org.opencontainers.image.title="agentactr static CLI" \
      org.opencontainers.image.description="Static musl agentactr CLI binary only; not an agent runtime image" \
      org.opencontainers.image.source="https://github.com/dwaiba/agentactrSDK" \
      org.opencontainers.image.licenses="Apache-2.0"

ARG TARGET=aarch64-unknown-linux-musl
COPY --from=verify /src/target/${TARGET}/release/agentactr /usr/local/bin/agentactr

USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/agentactr"]
