#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo 'usage: package-linux.sh arm64|amd64 OUTPUT_DIRECTORY' >&2
  exit 2
fi
arch="$1"
case "$arch:$(uname -m)" in
  arm64:aarch64|amd64:x86_64) ;;
  *) echo 'requested architecture does not match this Linux host' >&2; exit 2 ;;
esac
: "${CARGO_TARGET_DIR:?set an architecture-specific CARGO_TARGET_DIR}"

native_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$2"
output_dir=$(CDPATH= cd -- "$2" && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$native_dir/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then echo 'package version not found' >&2; exit 2; fi
source_archive="$output_dir/blackglass-headless-native-$version-source.tar.gz"
binary="$output_dir/bgh-$version-linux-$arch"
binary_archive="$output_dir/bgh-$version-linux-$arch.tar.gz"
checksums="$output_dir/sha256sums.txt"
license_report="$output_dir/dependency-licenses.json"
notices="$output_dir/THIRD-PARTY-LICENSES.txt"
license="$output_dir/LICENSE"
for item in "$source_archive" "$binary" "$binary_archive" "$checksums" "$license_report" "$notices" "$license"; do
  if [ -e "$item" ]; then echo "refusing to overwrite $item" >&2; exit 2; fi
done

tar --directory "$native_dir" --sort=name --mtime='@0' --owner=0 --group=0 \
  --numeric-owner -czf "$source_archive" Cargo.toml Cargo.lock build.rs src tests tools README.md LICENSE
source_sha=$(sha256sum "$source_archive" | cut -d ' ' -f 1)
build_root=$(mktemp -d /tmp/blackglass-native-package.XXXXXX)
trap 'rm -r -- "$build_root"' EXIT
tar -xzf "$source_archive" -C "$build_root"
BLACKGLASS_SOURCE_SHA256="$source_sha" cargo build --locked --release \
  --manifest-path "$build_root/Cargo.toml"
cp "$CARGO_TARGET_DIR/release/bgh" "$binary"
chmod 755 "$binary"
"$binary" build-info | grep -F "\"source_sha256\":\"$source_sha\"" >/dev/null
cp "$build_root/LICENSE" "$license"
target=$(rustc -vV | sed -n 's/^host: //p')
cargo metadata --locked --filter-platform "$target" --format-version 1 \
  --manifest-path "$build_root/Cargo.toml" \
  | python3 "$build_root/tools/license-report.py" "$notices" > "$license_report"
tar --directory "$output_dir" --sort=name --mtime='@0' --owner=0 --group=0 \
  --numeric-owner -czf "$binary_archive" "$(basename "$binary")" LICENSE THIRD-PARTY-LICENSES.txt dependency-licenses.json
(cd "$output_dir" && sha256sum "$(basename "$source_archive")" "$(basename "$binary")" \
  "$(basename "$binary_archive")" LICENSE THIRD-PARTY-LICENSES.txt dependency-licenses.json > sha256sums.txt)
echo "packaged $arch; source SHA-256 $source_sha"
