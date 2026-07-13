# syntax=docker.io/docker/dockerfile-upstream:1.23.0

ARG RUST_VERSION=1.96.0

FROM rust:1.97.0-trixie@sha256:44637ff22d0a6571a221bfaf137849711ad02ff4723dbb4736e297538f6a3e60 AS agentactr-builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --locked --release --bin agentactr \
    && /src/target/release/agentactr --help >/dev/null

FROM node:26.3.0-trixie-slim@sha256:aa27a5fbf5acb298116a38133794f080406c6f8dfe52e2e2836bb55dc7cae8f0

ARG AGENTACTR_VERSION=0.1.0
ARG CODEX_NPM_VERSION=0.130.0
ARG CODEX_SDK_NPM_VERSION=0.130.0
ARG GO_VERSION=1.26.3
ARG GO_LINUX_ARM64_SHA256=9d89a3ea57d141c2b22d70083f2c8459ba3890f2d9e818e7e933b75614936565
ARG RUST_VERSION=1.96.0
ARG UV_VERSION=0.11.13
ARG CARGO_NEXTEST_VERSION=0.9.133
ARG CARGO_DENY_VERSION=0.19.5
ARG CARGO_MACHETE_VERSION=0.9.2
ARG GOLANGCI_LINT_VERSION=v2.12.2
ARG GOVULNCHECK_VERSION=v1.3.0
ARG TARGETARCH

LABEL org.opencontainers.image.title="agentactr runtime" \
      org.opencontainers.image.description="Linux runtime surface for agentactr Docker execution backend" \
      org.opencontainers.image.source="https://github.com/dwaiba/agentactrSDK" \
      org.opencontainers.image.licenses="Apache-2.0"

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/opt/rust/cargo \
    GOBIN=/usr/local/bin \
    GOPATH=/opt/go \
    RUSTUP_HOME=/opt/rust/rustup \
    PATH=/usr/local/go/bin:/opt/rust/cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        coreutils \
        curl \
        dash \
        file \
        findutils \
        git \
        gnupg \
        jq \
        openssh-client \
        pkg-config \
        procps \
        python3 \
        python3-pip \
        python3-venv \
        tar \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g corepack && corepack enable

RUN set -eux; \
    npm install -g "@openai/codex@${CODEX_NPM_VERSION}" "@openai/codex-sdk@${CODEX_SDK_NPM_VERSION}"; \
    codex --version; \
    node --version; \
    npm --version; \
    corepack --version

RUN set -eux; \
    arch="${TARGETARCH:-arm64}"; \
    case "${arch}" in \
      arm64) go_arch="arm64"; go_sha="${GO_LINUX_ARM64_SHA256}" ;; \
      *) echo "unsupported TARGETARCH=${arch}; this bootstrap image is linux/arm64 only" >&2; exit 1 ;; \
    esac; \
    curl -fsSLo /tmp/go.tgz "https://go.dev/dl/go${GO_VERSION}.linux-${go_arch}.tar.gz"; \
    echo "${go_sha}  /tmp/go.tgz" | sha256sum -c -; \
    rm -rf /usr/local/go; \
    tar -C /usr/local -xzf /tmp/go.tgz; \
    rm /tmp/go.tgz; \
    go version

RUN set -eux; \
    curl -fsSLo /tmp/rustup-init.sh https://sh.rustup.rs; \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain "${RUST_VERSION}"; \
    rm /tmp/rustup-init.sh; \
    rustc --version; \
    cargo --version

RUN set -eux; \
    cargo_install() { \
      crate="$1"; \
      version="$2"; \
      if [ "${version}" = "latest" ]; then \
        cargo install --locked "${crate}"; \
      else \
        cargo install --locked "${crate}" --version "${version}"; \
      fi; \
    }; \
    cargo_install cargo-nextest "${CARGO_NEXTEST_VERSION}"; \
    cargo_install cargo-deny "${CARGO_DENY_VERSION}"; \
    cargo_install cargo-machete "${CARGO_MACHETE_VERSION}"; \
    cargo nextest --version; \
    cargo deny --version; \
    cargo machete --version

RUN set -eux; \
    go_install() { \
      package="$1"; \
      version="$2"; \
      if [ "${version}" = "latest" ]; then \
        go install "${package}@latest"; \
      else \
        go install "${package}@${version}"; \
      fi; \
    }; \
    go_install github.com/golangci/golangci-lint/v2/cmd/golangci-lint "${GOLANGCI_LINT_VERSION}"; \
    go_install golang.org/x/vuln/cmd/govulncheck "${GOVULNCHECK_VERSION}"; \
    golangci-lint --version; \
    govulncheck -version

RUN set -eux; \
    if [ "${UV_VERSION}" = "latest" ]; then \
      curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh; \
    else \
      curl -LsSf "https://astral.sh/uv/${UV_VERSION}/install.sh" | env UV_INSTALL_DIR=/usr/local/bin sh; \
    fi; \
    uv --version

COPY --from=agentactr-builder /src/target/release/agentactr /usr/local/bin/agentactr

RUN set -eux; \
    agentactr --help >/dev/null

RUN useradd --create-home --uid 10001 --shell /bin/bash agentactr \
    && mkdir -p /work /artifacts /trace /home/agentactr/.codex \
    && chown -R agentactr:agentactr /work /artifacts /trace /home/agentactr

USER agentactr:agentactr
WORKDIR /work

ENV CODEX_HOME=/home/agentactr/.codex \
    PYTHONUNBUFFERED=1

CMD ["bash"]
