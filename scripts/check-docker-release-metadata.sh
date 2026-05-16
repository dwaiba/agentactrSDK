#!/usr/bin/env sh
set -eu

for dockerfile in docker/agentactr-runtime/Dockerfile docker/agentactr-cli-static/Dockerfile; do
  test -f "${dockerfile}"

  if rg -n 'UNLICENSED' "${dockerfile}"; then
    printf 'Dockerfile has stale license metadata: %s\n' "${dockerfile}" >&2
    exit 1
  fi

  rg -n 'org.opencontainers.image.licenses="Apache-2.0"' "${dockerfile}" >/dev/null

  awk '
    /^FROM / && $2 !~ /^--/ { print FILENAME ":" FNR ":" $0 }
    /^FROM / && $2 ~ /^--/ { print FILENAME ":" FNR ":" $0 }
  ' "${dockerfile}" |
    while IFS= read -r line; do
      case "${line}" in
        *" AS verify"*) continue ;;
        *@sha256:*) ;;
        *)
          printf 'Docker FROM is not digest-pinned: %s\n' "${line}" >&2
          exit 1
          ;;
      esac
    done
done
