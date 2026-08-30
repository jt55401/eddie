#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Thin wrapper: widget/build.sh builds both WASM variants (eddie-lite,
# eddie-dense) for both wasm-bindgen targets along with every other runtime
# asset; `--sizes` prints raw / gzip / brotli bytes of each dist file.
#
# Usage: scripts/build-wasm-variants.sh [--js-only]

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec bash "$SCRIPT_DIR/../widget/build.sh" --sizes "$@"
