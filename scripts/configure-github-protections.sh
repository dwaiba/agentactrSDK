#!/usr/bin/env sh
set -eu

repo="${1:-dwaiba/agentactrSDK}"
branch="${2:-main}"
mode="${3:---dry-run}"

required_checks='[
  "Architecture / SOLID boundaries",
  "Build / Rust workspace build",
  "Build / Dockerfile checks",
  "CI / Rust quality gate",
  "Security / CodeQL",
  "Security / RustSec audit",
  "Security / Supply-chain metadata"
]'

if [ "${mode}" != "--apply" ]; then
  cat <<EOF
Dry run only.

This script configures branch protection for ${repo}:${branch} when run with --apply:

  scripts/configure-github-protections.sh ${repo} ${branch} --apply

Required checks:
${required_checks}

Before running --apply:
- push the first commit so ${branch} exists remotely;
- verify gh auth has admin permission for ${repo};
- enable first-time contributor workflow approval in GitHub repository settings;
- restrict v*.*.* tag creation to maintainers through a repository ruleset.
EOF
  exit 0
fi

tmp="${TMPDIR:-/tmp}/agentactr-branch-protection-$$.json"
trap 'rm -f "${tmp}"' EXIT

cat >"${tmp}" <<EOF
{
  "required_status_checks": {
    "strict": true,
    "contexts": ${required_checks}
  },
  "enforce_admins": true,
  "required_pull_request_reviews": {
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false,
    "required_approving_review_count": 1,
    "require_last_push_approval": true
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false,
  "required_conversation_resolution": true,
  "lock_branch": false,
  "allow_fork_syncing": true
}
EOF

gh api \
  --method PUT \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  "/repos/${repo}/branches/${branch}/protection" \
  --input "${tmp}" >/dev/null

printf 'Configured branch protection for %s:%s\n' "${repo}" "${branch}"
