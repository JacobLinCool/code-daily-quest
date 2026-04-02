#!/usr/bin/env bash
set -euo pipefail

REPOSITORY="${CODE_DAILY_QUEST_REPO:-JacobLinCool/code-daily-quest}"
INSTALL_DIR="${CODE_DAILY_QUEST_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${CODE_DAILY_QUEST_VERSION:-latest}"
BINARY_NAME="code-daily-quest"

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}/${arch}" in
    Darwin/arm64) echo "aarch64-apple-darwin" ;;
    Darwin/x86_64) echo "x86_64-apple-darwin" ;;
    Linux/x86_64) echo "x86_64-unknown-linux-gnu" ;;
    *)
      echo "unsupported platform: ${os}/${arch}" >&2
      exit 1
      ;;
  esac
}

download() {
  local url="$1"
  local output="$2"
  curl -fsSL --retry 3 --connect-timeout 10 "${url}" -o "${output}"
}

compute_sha256() {
  local file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${file}" | awk '{print $1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return
  fi

  echo "missing shasum/sha256sum for checksum verification" >&2
  exit 1
}

main() {
  local target archive checksum archive_url checksum_url version_prefix
  target="$(detect_target)"
  archive="${BINARY_NAME}-${target}.tar.gz"
  checksum="${archive}.sha256"

  if [[ "${VERSION}" == "latest" ]]; then
    archive_url="https://github.com/${REPOSITORY}/releases/latest/download/${archive}"
    checksum_url="https://github.com/${REPOSITORY}/releases/latest/download/${checksum}"
  else
    version_prefix="${VERSION}"
    if [[ "${version_prefix}" != v* ]]; then
      version_prefix="v${version_prefix}"
    fi
    archive_url="https://github.com/${REPOSITORY}/releases/download/${version_prefix}/${archive}"
    checksum_url="https://github.com/${REPOSITORY}/releases/download/${version_prefix}/${checksum}"
  fi

  local temp_dir archive_path checksum_path expected actual
  temp_dir="$(mktemp -d)"
  trap 'rm -rf "${temp_dir}"' EXIT
  archive_path="${temp_dir}/${archive}"
  checksum_path="${temp_dir}/${checksum}"

  download "${archive_url}" "${archive_path}"
  download "${checksum_url}" "${checksum_path}"

  expected="$(awk '{print $1}' "${checksum_path}")"
  actual="$(compute_sha256 "${archive_path}")"
  if [[ "${expected}" != "${actual}" ]]; then
    echo "checksum mismatch for ${archive}" >&2
    echo "expected ${expected}" >&2
    echo "actual   ${actual}" >&2
    exit 1
  fi

  mkdir -p "${INSTALL_DIR}"
  tar -xzf "${archive_path}" -C "${temp_dir}"
  install -m 0755 "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"

  echo "installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"
  echo "run '${BINARY_NAME} doctor' to verify local log discovery"
}

main "$@"
