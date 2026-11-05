#!/usr/bin/env bash
# Builds the cdylib and runs the C test against it (test-only C; not part of the engine).
# The read-side checks need the corpus (cargo xtask corpus-fetch): FONTISAN_CORPUS_DIR or .corpus/fontisan.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
cargo build -p fontisan-ffi ${PROFILE_FLAG:-}
dir=${CARGO_TARGET_DIR:-target}/${PROFILE_DIR:-debug}
fonts=${FONTISAN_CORPUS_DIR:-$root/.corpus/fontisan}/corpus/fonts
[ -d "$fonts" ] || { echo "corpus fonts missing at $fonts" >&2; exit 1; }
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
mkdir "$out/mixed"
for f in TestTTF.ttf TestTTC.ttc TestDFONT.dfont TestT1-Regular.pfb; do cp "$fonts/$f" "$out/mixed/"; done
echo "not a font" > "$out/mixed/README.txt"
${CC:-cc} -std=c11 -D_POSIX_C_SOURCE=200809L -Wall -Wextra -Werror -pthread -Icrates/fontisan/include \
  crates/fontisan-ffi/tests/abi_test.c -L"$dir" -lfontisan -o "$out/abi_test"
case "$(uname -s)" in
  Darwin) DYLD_LIBRARY_PATH="$dir" "$out/abi_test" "$fonts" "$out/mixed" ;;
  *)      LD_LIBRARY_PATH="$dir" "$out/abi_test" "$fonts" "$out/mixed" ;;
esac
