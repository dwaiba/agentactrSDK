#!/usr/bin/env sh
set -eu

IMAGE_REGISTRY="${IMAGE_REGISTRY:-ghcr.io/dwaiba}"
VERSION="${VERSION:-0.1.0}"
PLATFORM="${PLATFORM:-linux/arm64}"
RUST_VERSION="${RUST_VERSION:-1.95.0}"
IMAGE="${IMAGE_REGISTRY}/agentactr-cli:${VERSION}-linux-arm64-musl"
LATEST_IMAGE="${IMAGE_REGISTRY}/agentactr-cli:latest-linux-arm64-musl"
METADATA_DIR="${METADATA_DIR:-.agentactr/image-metadata}"
OUTPUT_FLAG="--load"

if [ "${PUSH:-0}" = "1" ]; then
  OUTPUT_FLAG="--push"
fi

mkdir -p "${METADATA_DIR}"

docker buildx build \
  --platform "${PLATFORM}" \
  --file docker/agentactr-cli-static/Dockerfile \
  --tag "${IMAGE}" \
  --tag "${LATEST_IMAGE}" \
  --build-arg "AGENTACTR_VERSION=${VERSION}" \
  --build-arg "RUST_VERSION=${RUST_VERSION}" \
  --sbom=true \
  --provenance=mode=max \
  --metadata-file "${METADATA_DIR}/agentactr-cli-static.json" \
  "${OUTPUT_FLAG}" \
  .

printf '%s\n' "${IMAGE}"
