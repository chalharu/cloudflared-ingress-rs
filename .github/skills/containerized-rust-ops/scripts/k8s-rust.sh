#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  k8s-rust.sh <fmt-check|check|clippy|build|test>
  k8s-rust.sh -- <command> [args...]

Run this repository's Rust commands in a control-plane Kubernetes job with a
PVC-backed persistent clone, cargo/rustup caches, target directory, and
sccache directory under /workspace/cache/.
USAGE
}

die() {
  printf 'k8s-rust.sh: %s\n' "$*" >&2
  exit 64
}

slugify() {
  printf '%s' "$1" | tr '/:@' '---' | tr -cs '[:alnum:]._-' '-'
}

[[ $# -gt 0 ]] || {
  usage
  exit 64
}

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

command -v control-plane-run >/dev/null 2>&1 || die "control-plane-run is required"

runtime_env="${CONTROL_PLANE_RUNTIME_ENV_FILE:-${HOME:-/home/copilot}/.config/control-plane/runtime.env}"
[[ -f "${runtime_env}" ]] || die "runtime env file not found: ${runtime_env}"

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[[ -n "${repo_root}" ]] || die "run this script from inside the repository"
install_sccache_script_path="${repo_root}/.github/skills/containerized-rust-ops/scripts/install-sccache.sh"
[[ -f "${install_sccache_script_path}" ]] || die "install-sccache.sh not found: ${install_sccache_script_path}"
install_sccache_script="$(cat "${install_sccache_script_path}")"

repo_name="$(basename "${repo_root}")"
branch="${K8S_RUST_BRANCH:-$(git branch --show-current)}"
[[ -n "${branch}" ]] || die "K8S_RUST_BRANCH is required when HEAD is detached"

repo_url="${K8S_RUST_REMOTE_URL:-$(git -C "${repo_root}" remote get-url origin)}"
[[ -n "${repo_url}" ]] || die "could not determine origin URL"
namespace="${CONTROL_PLANE_K8S_NAMESPACE:-}"
[[ -n "${namespace}" ]] || die "CONTROL_PLANE_K8S_NAMESPACE must be set"
sccache_version="${SCCACHE_VERSION:-0.14.0}"
sccache_release_base_url="${SCCACHE_RELEASE_BASE_URL:-https://github.com/mozilla/sccache/releases/download}"
sccache_bootstrap_jobs="${SCCACHE_BOOTSTRAP_JOBS:-1}"

branch_key="$(slugify "${branch}")"
repo_key="$(slugify "${repo_name}")"
branch_key="${branch_key#-}"
branch_key="${branch_key%-}"
repo_key="${repo_key#-}"
repo_key="${repo_key%-}"

image="${RUST_CONTAINER_IMAGE:-docker.io/rust:1.94.0-bookworm}"
timeout="${K8S_RUST_TIMEOUT:-7200s}"

tmpenv="$(mktemp)"
trap 'rm -f "${tmpenv}"' EXIT
cp "${runtime_env}" "${tmpenv}"
tmpenv_updated="$(mktemp)"
trap 'rm -f "${tmpenv}" "${tmpenv_updated}"' EXIT
sed 's/^CONTROL_PLANE_JOB_SERVICE_ACCOUNT=.*/CONTROL_PLANE_JOB_SERVICE_ACCOUNT=/' "${tmpenv}" > "${tmpenv_updated}"
mv "${tmpenv_updated}" "${tmpenv}"

repo_url_q="$(printf '%q' "${repo_url}")"
branch_q="$(printf '%q' "${branch}")"
repo_key_q="$(printf '%q' "${repo_key}")"
branch_key_q="$(printf '%q' "${branch_key}")"
sccache_version_q="$(printf '%q' "${sccache_version}")"
sccache_release_base_url_q="$(printf '%q' "${sccache_release_base_url}")"
sccache_bootstrap_jobs_q="$(printf '%q' "${sccache_bootstrap_jobs}")"

job_script="$(cat <<EOF
set -eu
repo_url=${repo_url_q}
branch=${branch_q}
repo_key=${repo_key_q}
branch_key=${branch_key_q}
sccache_version=${sccache_version_q}
sccache_release_base_url=${sccache_release_base_url_q}
sccache_bootstrap_jobs=${sccache_bootstrap_jobs_q}
workspace_root=/workspace
src_root="\${workspace_root}/src/\${repo_key}/\${branch_key}"
cache_root="\${workspace_root}/cache/\${repo_key}/\${branch_key}"
cargo_home="\${cache_root}/cargo"
rustup_home="\${cache_root}/rustup"
target_dir="\${cache_root}/target"
sccache_dir="\${cache_root}/sccache"
mkdir -p "\${workspace_root}/src/\${repo_key}" "\${cargo_home}" "\${rustup_home}" "\${target_dir}" "\${sccache_dir}"
if [ ! -x "\${cargo_home}/bin/cargo" ]; then
  cp -a /usr/local/cargo/. "\${cargo_home}/"
fi
if [ ! -d "\${rustup_home}/toolchains" ]; then
  cp -a /usr/local/rustup/. "\${rustup_home}/"
fi
export CARGO_HOME="\${cargo_home}"
export RUSTUP_HOME="\${rustup_home}"
export PATH="\${CARGO_HOME}/bin:\${PATH}"
export CARGO_TARGET_DIR="\${target_dir}"
export SCCACHE_DIR="\${sccache_dir}"
export SCCACHE_CACHE_SIZE="\${SCCACHE_CACHE_SIZE:-10G}"
export CARGO_INCREMENTAL=0
export CARGO_TERM_PROGRESS_WHEN=never
rm -rf "\${src_root}"
git clone --branch "\${branch}" --depth 1 "\${repo_url}" "\${src_root}"
cd "\${src_root}"
cat > /tmp/install-sccache.sh <<'INSTALL_SCCACHE'
${install_sccache_script}
INSTALL_SCCACHE
chmod 0755 /tmp/install-sccache.sh
rustfmt --version >/dev/null 2>&1 || rustup component add rustfmt >/tmp/rustfmt.log 2>&1
rustfmt --version >/dev/null 2>&1 || { cat /tmp/rustfmt.log >&2; exit 1; }
cargo clippy --version >/dev/null 2>&1 || rustup component add clippy >/tmp/clippy.log 2>&1
cargo clippy --version >/dev/null 2>&1 || { cat /tmp/clippy.log >&2; exit 1; }
export SCCACHE_VERSION="\${sccache_version}"
export SCCACHE_RELEASE_BASE_URL="\${sccache_release_base_url}"
export SCCACHE_BOOTSTRAP_JOBS="\${sccache_bootstrap_jobs}"
sh /tmp/install-sccache.sh
export RUSTC_WRAPPER="\${CARGO_HOME}/bin/sccache"
if "\$@"; then
  status=0
else
  status=\$?
fi
sccache --show-stats || true
exit "\${status}"
EOF
)"

CONTROL_PLANE_RUNTIME_ENV_FILE="${tmpenv}" \
control-plane-run \
  --mode auto \
  --execution-hint long \
  --namespace "${namespace}" \
  --timeout "${timeout}" \
  --image "${image}" \
  -- sh -c "${job_script}" sh "${cmd[@]}"
