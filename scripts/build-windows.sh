#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.local/mingw64/usr/bin:$PATH"

cargo build --release --target x86_64-pc-windows-gnu
