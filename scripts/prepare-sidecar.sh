#!/usr/bin/env bash
set -euo pipefail

# Build the CLI for the exact target being packaged and expose it using the
# target-triple filename required by Tauri's externalBin configuration.
target="${1:-$(rustc --print host-tuple)}"
if [[ -z "$target" ]]; then
  echo "Unable to determine the Rust target triple" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
cargo build --release --target "$target" --locked

mkdir -p src-tauri/binaries
cp "target/$target/release/codexctl" "src-tauri/binaries/codexctl-$target"
chmod +x "src-tauri/binaries/codexctl-$target"
echo "Prepared src-tauri/binaries/codexctl-$target"
