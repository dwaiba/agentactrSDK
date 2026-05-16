#!/usr/bin/env sh
set -eu

IMAGE_REGISTRY="${IMAGE_REGISTRY:-ghcr.io/dwaiba}"
VERSION="${VERSION:-0.1.0}"
PLATFORM="${PLATFORM:-linux/arm64}"
IMAGE="${IMAGE_REGISTRY}/agentactr-runtime:${VERSION}-linux-arm64"
LATEST_IMAGE="${IMAGE_REGISTRY}/agentactr-runtime:latest-linux-arm64"
METADATA_DIR="${METADATA_DIR:-.agentactr/image-metadata}"
OUTPUT_FLAG="--load"

CODEX_NPM_VERSION="${CODEX_NPM_VERSION:-0.130.0}"
CODEX_SDK_NPM_VERSION="${CODEX_SDK_NPM_VERSION:-0.130.0}"
GO_VERSION="${GO_VERSION:-1.26.3}"
GO_LINUX_ARM64_SHA256="${GO_LINUX_ARM64_SHA256:-9d89a3ea57d141c2b22d70083f2c8459ba3890f2d9e818e7e933b75614936565}"
RUST_VERSION="${RUST_VERSION:-1.95.0}"
UV_VERSION="${UV_VERSION:-0.11.13}"
CARGO_NEXTEST_VERSION="${CARGO_NEXTEST_VERSION:-0.9.133}"
CARGO_DENY_VERSION="${CARGO_DENY_VERSION:-0.19.5}"
CARGO_MACHETE_VERSION="${CARGO_MACHETE_VERSION:-0.9.2}"
GOLANGCI_LINT_VERSION="${GOLANGCI_LINT_VERSION:-v2.12.2}"
GOVULNCHECK_VERSION="${GOVULNCHECK_VERSION:-v1.3.0}"

if [ "${PUSH:-0}" = "1" ]; then
  OUTPUT_FLAG="--push"
fi

mkdir -p "${METADATA_DIR}"

docker buildx build \
  --platform "${PLATFORM}" \
  --file docker/agentactr-runtime/Dockerfile \
  --tag "${IMAGE}" \
  --tag "${LATEST_IMAGE}" \
  --build-arg "AGENTACTR_VERSION=${VERSION}" \
  --build-arg "CODEX_NPM_VERSION=${CODEX_NPM_VERSION}" \
  --build-arg "CODEX_SDK_NPM_VERSION=${CODEX_SDK_NPM_VERSION}" \
  --build-arg "GO_VERSION=${GO_VERSION}" \
  --build-arg "GO_LINUX_ARM64_SHA256=${GO_LINUX_ARM64_SHA256}" \
  --build-arg "RUST_VERSION=${RUST_VERSION}" \
  --build-arg "UV_VERSION=${UV_VERSION}" \
  --build-arg "CARGO_NEXTEST_VERSION=${CARGO_NEXTEST_VERSION}" \
  --build-arg "CARGO_DENY_VERSION=${CARGO_DENY_VERSION}" \
  --build-arg "CARGO_MACHETE_VERSION=${CARGO_MACHETE_VERSION}" \
  --build-arg "GOLANGCI_LINT_VERSION=${GOLANGCI_LINT_VERSION}" \
  --build-arg "GOVULNCHECK_VERSION=${GOVULNCHECK_VERSION}" \
  --sbom=true \
  --provenance=mode=max \
  --metadata-file "${METADATA_DIR}/agentactr-runtime.json" \
  "${OUTPUT_FLAG}" \
  .

printf '%s\n' "${IMAGE}"
