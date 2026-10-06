#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
# Cargo rebuilds when the embedded profile changes; startup reapplies it once.
exec cargo run --offline -p veloren-voxygen --bin veloren-voxygen --no-default-features -- "$@"
