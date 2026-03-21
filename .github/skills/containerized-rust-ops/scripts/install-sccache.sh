#!/bin/sh
set -eu

die() {
  printf 'install-sccache.sh: %s\n' "$*" >&2
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

  die "curl or wget is required to download sccache releases"
}

install_from_release() {
  version="$1"
  base_url="$2"
  target_bin="$3"

  case "$(uname -m)" in
    x86_64|amd64)
      asset="sccache-v${version}-x86_64-unknown-linux-musl.tar.gz"
      ;;
    aarch64|arm64)
      asset="sccache-v${version}-aarch64-unknown-linux-musl.tar.gz"
      ;;
    armv7l|armv7)
      asset="sccache-v${version}-armv7-unknown-linux-musleabi.tar.gz"
      ;;
    i686|i386)
      asset="sccache-v${version}-i686-unknown-linux-musl.tar.gz"
      ;;
    riscv64|riscv64gc)
      asset="sccache-v${version}-riscv64gc-unknown-linux-musl.tar.gz"
      ;;
    s390x)
      asset="sccache-v${version}-s390x-unknown-linux-gnu.tar.gz"
      ;;
    *)
      return 1
      ;;
  esac

  archive_url="${base_url}/v${version}/${asset}"
  checksum_url="${archive_url}.sha256"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' EXIT HUP INT TERM

  printf 'install-sccache.sh: downloading %s\n' "${asset}" >&2
  archive_path="${tmpdir}/${asset}"
  checksum_path="${archive_path}.sha256"
  download_file "${archive_path}" "${archive_url}"
  download_file "${checksum_path}" "${checksum_url}"

  checksum="$(awk 'NR == 1 { print $1; exit }' < "${checksum_path}" | tr -d '\r\n')"
  [ -n "${checksum}" ] || die "downloaded checksum for ${asset} was empty"
  printf '%s  %s\n' "${checksum}" "${archive_path}" | sha256sum -c -

  tar -xzf "${archive_path}" -C "${tmpdir}"
  extracted_bin="$(find "${tmpdir}" -type f -name sccache -print -quit)"
  [ -n "${extracted_bin}" ] || die "could not find sccache binary in ${asset}"

  install -m 0755 "${extracted_bin}" "${target_bin}"
  printf 'install-sccache.sh: installed %s\n' "${target_bin}" >&2
  rm -rf "${tmpdir}"
  trap - EXIT HUP INT TERM
}

cargo_home="${CARGO_HOME:-/usr/local/cargo}"
target_bin="${cargo_home}/bin/sccache"
[ -x "${target_bin}" ] && exit 0

mkdir -p "${cargo_home}/bin"
version="${SCCACHE_VERSION:-0.14.0}"
base_url="${SCCACHE_RELEASE_BASE_URL:-https://github.com/mozilla/sccache/releases/download}"

if ! install_from_release "${version}" "${base_url}" "${target_bin}"; then
  printf 'install-sccache.sh: no matching prebuilt release for %s, falling back to cargo install\n' "$(uname -m)" >&2
  CARGO_BUILD_JOBS="${SCCACHE_BOOTSTRAP_JOBS:-1}" cargo install --locked sccache
fi
