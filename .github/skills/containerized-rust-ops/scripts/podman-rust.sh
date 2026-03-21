#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  podman-rust.sh <fmt-check|check|clippy|build|test>
  podman-rust.sh -- <command> [args...]

Run this repository's Rust commands inside docker.io/rust:1.94.0-bookworm with
persistent rustup, cargo, target, and sccache directories rooted under
`git rev-parse --git-path .copilot-cache/...`.
USAGE
}

die() {
  printf 'podman-rust.sh: %s\n' "$*" >&2
  exit 64
}

[[ $# -gt 0 ]] || {
  usage
  exit 64
}

image="${RUST_CONTAINER_IMAGE:-docker.io/rust:1.94.0-bookworm}"
podman_cmd=(env -u CONTAINER_HOST podman)
case "$1" in
  fmt-check)
    shift
    [[ $# -eq 0 ]] || die "fmt-check does not accept extra arguments"
    cmd=(cargo fmt --all --check)
    ;;
  check)
    shift
    [[ $# -eq 0 ]] || die "check does not accept extra arguments"
    cmd=(cargo check --workspace --all-targets)
    ;;
  clippy)
    shift
    [[ $# -eq 0 ]] || die "clippy does not accept extra arguments"
    cmd=(cargo clippy --workspace --all-targets -- -D warnings)
    ;;
  build)
    shift
    [[ $# -eq 0 ]] || die "build does not accept extra arguments"
    cmd=(cargo build --workspace)
    ;;
  test)
    shift
    [[ $# -eq 0 ]] || die "test does not accept extra arguments"
    cmd=(cargo test --workspace --all-targets)
    ;;
  --)
    shift
    [[ $# -gt 0 ]] || die "-- must be followed by a command"
    cmd=("$@")
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    die "unknown preset: $1"
    ;;
esac

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[[ -n "${repo_root}" ]] || die "run this script from inside the repository"

sccache_version="${SCCACHE_VERSION:-0.14.0}"
sccache_release_base_url="${SCCACHE_RELEASE_BASE_URL:-https://github.com/mozilla/sccache/releases/download}"
sccache_bootstrap_jobs="${SCCACHE_BOOTSTRAP_JOBS:-1}"
rustup_cache="$(git -C "${repo_root}" rev-parse --git-path .copilot-cache/podman-rustup)"
cargo_cache="$(git -C "${repo_root}" rev-parse --git-path .copilot-cache/podman-cargo)"
target_cache="$(git -C "${repo_root}" rev-parse --git-path .copilot-cache/podman-target)"
sccache_cache="$(git -C "${repo_root}" rev-parse --git-path .copilot-cache/podman-sccache)"
[[ "${rustup_cache}" == /* ]] || rustup_cache="${repo_root}/${rustup_cache}"
[[ "${cargo_cache}" == /* ]] || cargo_cache="${repo_root}/${cargo_cache}"
[[ "${target_cache}" == /* ]] || target_cache="${repo_root}/${target_cache}"
[[ "${sccache_cache}" == /* ]] || sccache_cache="${repo_root}/${sccache_cache}"

mkdir -p "${rustup_cache}" "${cargo_cache}" "${target_cache}" "${sccache_cache}"

bootstrap_toolchain() {
  if [[ -x "${cargo_cache}/bin/cargo" ]]; then
    return
  fi

  "${podman_cmd[@]}" run --rm -i \
    -v "${rustup_cache}:/host-rustup" \
    -v "${cargo_cache}:/host-cargo" \
    "${image}" \
    sh -c 'cp -R /usr/local/rustup/. /host-rustup/ && cp -R /usr/local/cargo/. /host-cargo/'
}

ensure_tools() {
  "${podman_cmd[@]}" run --rm -i \
    -e SCCACHE_VERSION="${sccache_version}" \
    -e SCCACHE_RELEASE_BASE_URL="${sccache_release_base_url}" \
    -e SCCACHE_BOOTSTRAP_JOBS="${sccache_bootstrap_jobs}" \
    -v "${repo_root}:/workspace" \
    -w /workspace \
    -v "${rustup_cache}:/usr/local/rustup" \
    -v "${cargo_cache}:/usr/local/cargo" \
    "${image}" \
    sh -c '
      set -eu
      export CARGO_HOME=/usr/local/cargo
      export RUSTUP_HOME=/usr/local/rustup
      export PATH=/usr/local/cargo/bin:$PATH
      rustfmt --version >/dev/null 2>&1 || rustup component add rustfmt >/tmp/rustfmt.log 2>&1
      rustfmt --version >/dev/null 2>&1 || { cat /tmp/rustfmt.log >&2; exit 1; }
      cargo clippy --version >/dev/null 2>&1 || rustup component add clippy >/tmp/clippy.log 2>&1
      cargo clippy --version >/dev/null 2>&1 || { cat /tmp/clippy.log >&2; exit 1; }
      sh .github/skills/containerized-rust-ops/scripts/install-sccache.sh
    '
}

bootstrap_toolchain
ensure_tools

"${podman_cmd[@]}" run --rm -i \
  -e CARGO_TERM_PROGRESS_WHEN=never \
  -v "${repo_root}:/workspace" \
  -w /workspace \
  -v "${rustup_cache}:/usr/local/rustup" \
  -v "${cargo_cache}:/usr/local/cargo" \
  -v "${target_cache}:/workspace/target" \
  -v "${sccache_cache}:/var/cache/sccache" \
  "${image}" \
  sh -c '
    set -eu
    export CARGO_HOME=/usr/local/cargo
    export RUSTUP_HOME=/usr/local/rustup
    export PATH=/usr/local/cargo/bin:$PATH
    export CARGO_TARGET_DIR=/workspace/target
    export SCCACHE_DIR=/var/cache/sccache
    export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"
    export RUSTC_WRAPPER=/usr/local/cargo/bin/sccache
    export CARGO_INCREMENTAL=0
    if "$@"; then
      status=0
    else
      status=$?
    fi
    sccache --show-stats || true
    exit "${status}"
  ' sh "${cmd[@]}"
