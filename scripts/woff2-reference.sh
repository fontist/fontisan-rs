#!/usr/bin/env bash
# Decode every WOFF2 in a corpus with the pinned Google `woff2_decompress`
# (built by fontisan's scripts/build-woff2.sh) so tests/woff_decode.rs can
# compare the Rust decoder against it byte for byte.
#
#   scripts/woff2-reference.sh <woff2_decompress> <out dir> <dir with .woff2 files>...
#
# Output: <out dir>/<basename> (input copy) and <basename>.ref (the tool names a collection's output .ttf,
# so the format is detected by magic bytes in the test, not by extension).
# Then: FONTISAN_WOFF2_REF_DIR=<out dir> cargo test --test woff_decode
set -euo pipefail
tool=$1 out=$2
shift 2
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
find "$@" -name '*.woff2' | sort | while read -r f; do
  base=$(basename "$f")
  cp "$f" "$work/in.woff2"
  rm -f "$work/in.ttf"
  "$tool" "$work/in.woff2" >/dev/null
  cp "$f" "$out/$base"
  mv "$work/in.ttf" "$out/$base.ref"
  echo "ref: $base"
done
