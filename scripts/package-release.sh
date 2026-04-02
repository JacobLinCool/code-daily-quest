#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <binary-path> <dist-dir>" >&2
  exit 1
fi

target="$1"
binary_path="$2"
dist_dir="$3"
binary_name="code-daily-quest"
archive_name="${binary_name}-${target}.tar.gz"
checksum_name="${archive_name}.sha256"
archive_path="${dist_dir}/${archive_name}"
checksum_path="${dist_dir}/${checksum_name}"

mkdir -p "${dist_dir}"
tar -C "$(dirname "${binary_path}")" -czf "${archive_path}" "$(basename "${binary_path}")"

if command -v shasum >/dev/null 2>&1; then
  hash="$(shasum -a 256 "${archive_path}" | awk '{print $1}')"
elif command -v sha256sum >/dev/null 2>&1; then
  hash="$(sha256sum "${archive_path}" | awk '{print $1}')"
else
  echo "missing shasum/sha256sum for checksum generation" >&2
  exit 1
fi

printf '%s  %s\n' "${hash}" "${archive_name}" > "${checksum_path}"
echo "packaged ${archive_name}"
