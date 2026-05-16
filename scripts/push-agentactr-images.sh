#!/usr/bin/env sh
set -eu

IMAGE_REGISTRY="${IMAGE_REGISTRY:-ghcr.io/dwaiba}"
VERSION="${VERSION:-0.1.0}"

docker push "${IMAGE_REGISTRY}/agentactr-cli:${VERSION}-linux-arm64-musl"
docker push "${IMAGE_REGISTRY}/agentactr-cli:latest-linux-arm64-musl"
docker push "${IMAGE_REGISTRY}/agentactr-runtime:${VERSION}-linux-arm64"
docker push "${IMAGE_REGISTRY}/agentactr-runtime:latest-linux-arm64"
