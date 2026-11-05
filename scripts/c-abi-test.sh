#!/usr/bin/env bash
# Builds the cdylib and runs the C test against it (test-only C; not part of the engine).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p fontisan-ffi ${PROFILE_FLAG:-}
dir=target/${PROFILE_DIR:-debug}
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
${CC:-cc} -std=c11 -Wall -Wextra -Werror -Icrates/fontisan/include \
  crates/fontisan-ffi/tests/abi_test.c -L"$dir" -lfontisan -o "$out/abi_test"
case "$(uname -s)" in
  Darwin) DYLD_LIBRARY_PATH="$dir" "$out/abi_test" ;;
  *)      LD_LIBRARY_PATH="$dir" "$out/abi_test" ;;
esac
