#!/usr/bin/env sh
set -eu

usage() {
  cat <<'USAGE'
Install an agentactr release binary into a user-writable bin directory.

Usage:
  scripts/install-agentactr.sh [--source PATH] [--bin-dir PATH]
                              [--install-completions]
                              [--completion-dir PATH]
                              [--shell bash|zsh|fish|powershell|elvish]
                              [--update-shell-profile]

Defaults:
  --source              Auto-detect ./agentactr or ./agentactr-*
  --bin-dir             $HOME/.local/bin
  --completion-dir      Shell-specific user completion directory

Conservative behavior:
  - Never edits shell profiles unless --update-shell-profile is passed.
  - Prints exact PATH instructions after installation.
  - Verifies the installed binary by running agentactr --version.
USAGE
}

die() {
  printf 'install-agentactr: %s\n' "$*" >&2
  exit 1
}

info() {
  printf '%s\n' "$*"
}

shell_name() {
  if [ -n "${AGENTACTR_INSTALL_SHELL:-}" ]; then
    printf '%s\n' "${AGENTACTR_INSTALL_SHELL}"
    return
  fi
  if [ -n "${SHELL:-}" ]; then
    basename "${SHELL}"
    return
  fi
  printf 'unknown\n'
}

absolute_path() {
  path="$1"
  case "${path}" in
    /*) printf '%s\n' "${path}" ;;
    *) printf '%s/%s\n' "$(pwd)" "${path}" ;;
  esac
}

shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

powershell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/''/g")"
}

detect_source() {
  if [ -x ./agentactr ]; then
    printf './agentactr\n'
    return
  fi

  found=""
  for candidate in ./agentactr-*; do
    [ -f "${candidate}" ] || continue
    [ -x "${candidate}" ] || continue
    case "${candidate}" in
      *.tar.gz|*.sha256) continue ;;
    esac
    if [ -n "${found}" ]; then
      die "multiple agentactr binaries found; pass --source PATH"
    fi
    found="${candidate}"
  done

  if [ -n "${found}" ]; then
    printf '%s\n' "${found}"
    return
  fi

  die "no executable ./agentactr or ./agentactr-* found; pass --source PATH"
}

completion_dir_for_shell() {
  selected_shell="$1"
  case "${selected_shell}" in
    bash) printf '%s/.local/share/bash-completion/completions\n' "${HOME}" ;;
    zsh) printf '%s/.local/share/zsh/site-functions\n' "${HOME}" ;;
    fish) printf '%s/.config/fish/completions\n' "${HOME}" ;;
    powershell|pwsh) printf '%s/.config/powershell/completions\n' "${HOME}" ;;
    elvish) printf '%s/.config/elvish/lib\n' "${HOME}" ;;
    *) die "unsupported shell for completions: ${selected_shell}" ;;
  esac
}

completion_file_for_shell() {
  selected_shell="$1"
  dir="$2"
  case "${selected_shell}" in
    bash) printf '%s/agentactr\n' "${dir}" ;;
    zsh) printf '%s/_agentactr\n' "${dir}" ;;
    fish) printf '%s/agentactr.fish\n' "${dir}" ;;
    powershell|pwsh) printf '%s/agentactr.ps1\n' "${dir}" ;;
    elvish) printf '%s/agentactr-completions.elv\n' "${dir}" ;;
    *) die "unsupported shell for completions: ${selected_shell}" ;;
  esac
}

profile_path_for_shell() {
  selected_shell="$1"
  case "${selected_shell}" in
    bash)
      if [ "$(uname -s 2>/dev/null || printf unknown)" = "Darwin" ]; then
        printf '%s/.bash_profile\n' "${HOME}"
      else
        printf '%s/.bashrc\n' "${HOME}"
      fi
      ;;
    zsh) printf '%s/.zshrc\n' "${HOME}" ;;
    fish) printf '%s/.config/fish/config.fish\n' "${HOME}" ;;
    powershell|pwsh) printf '%s/.config/powershell/Microsoft.PowerShell_profile.ps1\n' "${HOME}" ;;
    elvish) printf '%s/.config/elvish/rc.elv\n' "${HOME}" ;;
    *) die "unsupported shell for profile update: ${selected_shell}" ;;
  esac
}

path_line_for_shell() {
  selected_shell="$1"
  bin_dir="$2"
  case "${selected_shell}" in
    fish) printf 'fish_add_path %s\n' "$(shell_quote "${bin_dir}")" ;;
    powershell|pwsh) printf '$env:PATH = %s + [IO.Path]::PathSeparator + $env:PATH\n' "$(powershell_quote "${bin_dir}")" ;;
    elvish) printf 'set paths = [%s $@paths]\n' "$(shell_quote "${bin_dir}")" ;;
    *) printf 'export PATH="%s:$PATH"\n' "${bin_dir}" ;;
  esac
}

profile_block_for_shell() {
  selected_shell="$1"
  bin_dir="$2"
  printf '# >>> agentactr PATH >>>\n'
  path_line_for_shell "${selected_shell}" "${bin_dir}"
  printf '# <<< agentactr PATH <<<\n'
}

update_shell_profile() {
  selected_shell="$1"
  bin_dir="$2"
  profile_path="$(profile_path_for_shell "${selected_shell}")"
  profile_dir="$(dirname "${profile_path}")"
  mkdir -p "${profile_dir}"
  touch "${profile_path}"

  if grep -n '# >>> agentactr PATH >>>' "${profile_path}" >/dev/null 2>&1; then
    info "shell profile already contains an agentactr PATH block: ${profile_path}"
    return
  fi

  {
    printf '\n'
    profile_block_for_shell "${selected_shell}" "${bin_dir}"
  } >> "${profile_path}"

  info "updated shell profile: ${profile_path}"
}

print_manual_path_instructions() {
  bin_dir="$1"
  cat <<EOF

Add agentactr to PATH if it is not already visible:

  zsh:
    echo 'export PATH="${bin_dir}:\$PATH"' >> ~/.zshrc

  bash:
    echo 'export PATH="${bin_dir}:\$PATH"' >> ~/.bashrc
    # macOS login shells may use ~/.bash_profile instead.

  fish:
    fish_add_path $(shell_quote "${bin_dir}")

  PowerShell:
    Add $(powershell_quote "${bin_dir}") to the user PATH or profile explicitly.

  Elvish:
    Add this to ~/.config/elvish/rc.elv:
      set paths = [$(shell_quote "${bin_dir}") \$@paths]
EOF
}

source_path=""
bin_dir="${HOME}/.local/bin"
selected_shell="$(shell_name)"
install_completions=false
completion_dir=""
update_profile=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --source)
      [ "$#" -ge 2 ] || die "--source requires a path"
      source_path="$2"
      shift 2
      ;;
    --bin-dir)
      [ "$#" -ge 2 ] || die "--bin-dir requires a path"
      bin_dir="$2"
      shift 2
      ;;
    --shell)
      [ "$#" -ge 2 ] || die "--shell requires a value"
      selected_shell="$2"
      shift 2
      ;;
    --install-completions)
      install_completions=true
      shift
      ;;
    --completion-dir)
      [ "$#" -ge 2 ] || die "--completion-dir requires a path"
      completion_dir="$2"
      shift 2
      ;;
    --update-shell-profile)
      update_profile=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

[ -n "${HOME:-}" ] || die "HOME is required"

case "${selected_shell}" in
  pwsh) selected_shell="powershell" ;;
esac

if [ -z "${source_path}" ]; then
  source_path="$(detect_source)"
fi

[ -f "${source_path}" ] || die "source binary does not exist: ${source_path}"
[ -x "${source_path}" ] || die "source binary is not executable: ${source_path}"

bin_dir="$(absolute_path "${bin_dir}")"
mkdir -p "${bin_dir}"

install_path="${bin_dir}/agentactr"
tmp_path="${install_path}.tmp.$$"
cp "${source_path}" "${tmp_path}"
chmod 0755 "${tmp_path}"
mv "${tmp_path}" "${install_path}"

info "installed ${install_path}"
"${install_path}" --version

if [ "${install_completions}" = true ]; then
  if [ -z "${completion_dir}" ]; then
    completion_dir="$(completion_dir_for_shell "${selected_shell}")"
  fi
  completion_dir="$(absolute_path "${completion_dir}")"
  mkdir -p "${completion_dir}"
  completion_file="$(completion_file_for_shell "${selected_shell}" "${completion_dir}")"
  "${install_path}" completions "${selected_shell}" > "${completion_file}"
  chmod 0644 "${completion_file}"
  info "installed ${selected_shell} completions: ${completion_file}"
fi

if [ "${update_profile}" = true ]; then
  update_shell_profile "${selected_shell}" "${bin_dir}"
else
  print_manual_path_instructions "${bin_dir}"
  info ""
  info "To update the detected shell profile automatically, rerun with --update-shell-profile --shell ${selected_shell}."
fi
