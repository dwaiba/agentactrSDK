#!/usr/bin/env sh
set -eu

VERSION="${VERSION:-0.1.0}"
REPO="${REPO:-dwaiba/agentactrSDK}"
REF="${REF:-main}"
RELEASE_NOTES_FILE="${RELEASE_NOTES_FILE:-docs/releases/${VERSION}.md}"

test -f "${RELEASE_NOTES_FILE}" || {
  echo "release notes file not found: ${RELEASE_NOTES_FILE}" >&2
  exit 1
}

gh workflow run release.yml \
  -R "${REPO}" \
  --ref "${REF}" \
  -f "version=${VERSION}" \
  -f "release_notes=$(cat "${RELEASE_NOTES_FILE}")"
