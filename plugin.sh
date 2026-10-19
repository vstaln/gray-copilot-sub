#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/copilot-sub.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/copilot-sub" ;;
    *) rel="x86_64-apple-darwin/copilot-sub" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/copilot-sub" ;;
    *) rel="x86_64-unknown-linux-musl/copilot-sub" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/copilot-sub.exe" "$here/target/release/copilot-sub" "$here/target/debug/copilot-sub.exe" "$here/target/debug/copilot-sub"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "copilot-sub binary not found in $here" >&2
exit 1
