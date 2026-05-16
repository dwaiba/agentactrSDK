#!/usr/bin/env sh
set -eu

required_gate_workflows="
.github/workflows/architecture.yml
.github/workflows/build.yml
.github/workflows/ci.yml
.github/workflows/security.yml
"

for workflow in ${required_gate_workflows}; do
  test -f "${workflow}"
  rg -n '^  pull_request:$' "${workflow}" >/dev/null
  rg -n '^  merge_group:$' "${workflow}" >/dev/null
  rg -n '^    branches:$' "${workflow}" >/dev/null
  rg -n '^      - main$' "${workflow}" >/dev/null
  rg -n '^concurrency:$' "${workflow}" >/dev/null
  rg -n '^  cancel-in-progress: true$' "${workflow}" >/dev/null
  if rg -n 'secrets\.' "${workflow}"; then
    printf 'PR and merge-queue gates must not reference repository secrets: %s\n' "${workflow}" >&2
    exit 1
  fi
done

if rg -n '^  pull_request_target:' .github/workflows/*.yml; then
  printf 'pull_request_target is forbidden for this repository because fork PRs are untrusted.\n' >&2
  exit 1
fi

for workflow in .github/workflows/*.yml; do
  test -f "${workflow}"
  if rg -n 'uses: [^@[:space:]]+($|[[:space:]]+#)' "${workflow}"; then
    printf 'Every external action reference must include an explicit ref: %s\n' "${workflow}" >&2
    exit 1
  fi
  job_count="$(awk '
    /^jobs:$/ { in_jobs = 1; next }
    in_jobs && /^[^[:space:]]/ { in_jobs = 0 }
    in_jobs && /^  [A-Za-z0-9_-]+:$/ { count++ }
    END { print count + 0 }
  ' "${workflow}")"
  timeout_count="$(rg -c '^    timeout-minutes:' "${workflow}" || true)"
  if [ "${job_count}" -gt 0 ] && [ "${timeout_count}" -lt "${job_count}" ]; then
    printf 'Every workflow job must declare timeout-minutes: %s\n' "${workflow}" >&2
    exit 1
  fi
done

for workflow in .github/workflows/build.yml; do
  test -f "${workflow}"
  rg -n "if: github.event_name != 'push'" "${workflow}" >/dev/null
  if rg -n 'docker/setup-buildx-action@' "${workflow}" >/dev/null; then
    qemu_line="$(rg -n 'docker/setup-qemu-action@' "${workflow}" | head -n 1 | cut -d: -f1 || true)"
    buildx_line="$(rg -n 'docker/setup-buildx-action@' "${workflow}" | head -n 1 | cut -d: -f1 || true)"
    if [ -z "${qemu_line}" ] || [ -z "${buildx_line}" ] || [ "${qemu_line}" -ge "${buildx_line}" ]; then
      printf 'QEMU must be registered before Buildx in %s\n' "${workflow}" >&2
      exit 1
    fi
  fi
done

for workflow in .github/workflows/nightly.yml .github/workflows/release.yml .github/workflows/docker-main.yml; do
  test -f "${workflow}"
  rg -n 'depot/setup-action@' "${workflow}" >/dev/null
  rg -n 'depot/build-push-action@' "${workflow}" >/dev/null
  rg -n 'project: \$\{\{ vars\.DEPOT_PROJECT_ID \}\}' "${workflow}" >/dev/null
  rg -n 'token: \$\{\{ secrets\.DEPOT_TOKEN \}\}' "${workflow}" >/dev/null
  if rg -n 'docker/setup-buildx-action@|docker/setup-qemu-action@|scripts/build-agentactr-(runtime|cli-static)\.sh' "${workflow}"; then
    printf 'Trusted Docker image builds must use Depot, not local buildx scripts: %s\n' "${workflow}" >&2
    exit 1
  fi
done

rg -n '^  push:$' .github/workflows/docker-main.yml >/dev/null
rg -n '^      - main$' .github/workflows/docker-main.yml >/dev/null
rg -n 'call: check' .github/workflows/docker-main.yml >/dev/null

if rg -n 'scripts/build-agentactr-(runtime|cli-static)\.sh|PUSH: "1"|--push' \
  .github/workflows/build.yml .github/workflows/ci.yml .github/workflows/architecture.yml .github/workflows/security.yml; then
  printf 'PR and merge-queue gates must not publish or run full Docker image builds; keep release/nightly image builds separate and Depot-backed.\n' >&2
  exit 1
fi
