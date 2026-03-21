#!/bin/sh
set -eu

die() {
  printf 'install-cargo-llvm-cov.sh: %s\n' "$*" >&2
  exit 64
}

download_file() {
  dest="$1"
  url="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --retry-delay 1 -o "$dest" "$url"
    return 0
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -t 3 --retry-connrefused --waitretry=1 -qO "$dest" "$url"
    return 0
  fi

  die "curl or wget is required to download cargo-llvm-cov releases"
}

install_from_release() {
  version="$1"
  base_url="$2"
  target_bin="$3"

  case "$(uname -m)" in
    x86_64|amd64)
      asset="cargo-llvm-cov-x86_64-unknown-linux-gnu.tar.gz"
      ;;
    aarch64|arm64)
      asset="cargo-llvm-cov-aarch64-unknown-linux-gnu.tar.gz"
      ;;
    *)
      die "no supported prebuilt cargo-llvm-cov release for $(uname -m)"
      ;;
  esac

  archive_url="${CARGO_LLVM_COV_ARCHIVE_URL:-${base_url}/download/v${version}/${asset}}"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' EXIT HUP INT TERM

  printf 'install-cargo-llvm-cov.sh: downloading %s\n' "${archive_url}" >&2
  archive_path="${tmpdir}/${asset}"
  download_file "${archive_path}" "${archive_url}"

  tar -xzf "${archive_path}" -C "${tmpdir}"
  extracted_bin="$(find "${tmpdir}" -type f -name cargo-llvm-cov -print -quit)"
  [ -n "${extracted_bin}" ] || die "could not find cargo-llvm-cov binary in ${asset}"

  install -m 0755 "${extracted_bin}" "${target_bin}"
  printf 'install-cargo-llvm-cov.sh: installed %s\n' "${target_bin}" >&2
  rm -rf "${tmpdir}"
  trap - EXIT HUP INT TERM
}

cargo_home="${CARGO_HOME:-/usr/local/cargo}"
target_bin="${cargo_home}/bin/cargo-llvm-cov"
[ -x "${target_bin}" ] && exit 0

mkdir -p "${cargo_home}/bin"
version="${CARGO_LLVM_COV_VERSION:-0.8.5}"
base_url="${CARGO_LLVM_COV_RELEASE_BASE_URL:-https://github.com/taiki-e/cargo-llvm-cov/releases}"

install_from_release "${version}" "${base_url}" "${target_bin}"
