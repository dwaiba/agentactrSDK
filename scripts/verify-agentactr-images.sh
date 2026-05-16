#!/usr/bin/env sh
set -eu

IMAGE_REGISTRY="${IMAGE_REGISTRY:-ghcr.io/dwaiba}"
VERSION="${VERSION:-0.1.0}"
CLI_IMAGE="${CLI_IMAGE:-${IMAGE_REGISTRY}/agentactr-cli:${VERSION}-linux-arm64-musl}"
RUNTIME_IMAGE="${RUNTIME_IMAGE:-${IMAGE_REGISTRY}/agentactr-runtime:${VERSION}-linux-arm64}"

docker run --rm "${CLI_IMAGE}" --help >/dev/null

docker run --rm "${RUNTIME_IMAGE}" codex --version
docker run --rm "${RUNTIME_IMAGE}" codex app-server --help >/dev/null
docker run --rm "${RUNTIME_IMAGE}" git --version
docker run --rm "${RUNTIME_IMAGE}" go version
docker run --rm "${RUNTIME_IMAGE}" rustc --version
docker run --rm "${RUNTIME_IMAGE}" cargo nextest --version
docker run --rm "${RUNTIME_IMAGE}" cargo deny --version
docker run --rm "${RUNTIME_IMAGE}" cargo machete --version
docker run --rm "${RUNTIME_IMAGE}" golangci-lint --version
docker run --rm "${RUNTIME_IMAGE}" govulncheck -version
docker run --rm "${RUNTIME_IMAGE}" uv --version
docker run --rm "${RUNTIME_IMAGE}" node --version
docker run --rm "${RUNTIME_IMAGE}" npm --version
docker run --rm "${RUNTIME_IMAGE}" corepack --version
docker run --rm "${RUNTIME_IMAGE}" sh -lc 'test -f /sys/fs/cgroup/cgroup.controllers && test -f /proc/pressure/memory'
