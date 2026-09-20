#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then
  echo 'usage: package-relocation.sh PACKAGE_DIRECTORY' >&2
  exit 2
fi
package_dir="$1"
test_dir=$(mktemp -d /tmp/blackglass-native-relocation.XXXXXX)
trap 'rm -r -- "$test_dir"' EXIT
cp "$package_dir"/* "$test_dir/"
(
  cd "$test_dir"
  sha256sum -c sha256sums.txt
  binary=$(find . -maxdepth 1 -name 'bgh-*-linux-*' ! -name '*.tar.gz' -type f | head -n 1)
  source=$(find . -maxdepth 1 -name '*-source.tar.gz' -type f | head -n 1)
  archive=$(find . -maxdepth 1 -name 'bgh-*-linux-*.tar.gz' -type f | head -n 1)
  test -n "$binary" && test -n "$source" && test -n "$archive"
  source_sha=$(sha256sum "$source" | cut -d ' ' -f 1)
  "$binary" build-info | grep -F "\"source_sha256\":\"$source_sha\"" >/dev/null
  tar -tzf "$archive" | grep -Fx LICENSE >/dev/null
  tar -tzf "$archive" | grep -Fx THIRD-PARTY-LICENSES.txt >/dev/null
  tar -tzf "$archive" | grep -Fx dependency-licenses.json >/dev/null
  tar -tzf "$archive" | grep -Fx bgh.service.example >/dev/null
  if tar -tzf "$source" | grep -Ei '(^|/)(app\.asar|obsidian.*\.js|cli\.js)$'; then
    echo 'source archive contains an upstream bundle' >&2
    exit 1
  fi
)
echo 'relocated package verified'
