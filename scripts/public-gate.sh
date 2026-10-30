#!/usr/bin/env bash
# Public local gate (also what CI runs). One command:
#   scripts/public-gate.sh <OUT_DIR>
# Env:
#   CVD_SDK_PATH        LOCAL-ONLY development override of the SDK (an unpublished
#                       cvd-core checkout's crates/indicator-sdk). Never commit it.
#   CVD_WASM_HOST_DIR   dir with cvd-core host-bridge.js, plugin-worker.js and
#                       cvd_browser_host.wasm -> enables the real-Chromium smoke
#   CVD_REQUIRE_BROWSER=1  fail instead of skipping when CVD_WASM_HOST_DIR is unset
#   CVD_RELEASE_VERSION release tag recorded in the manifest
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?usage: scripts/public-gate.sh <OUT_DIR>}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
cd "$ROOT"
CFG=()
PKG=()
if [[ -n "${CVD_SDK_PATH:-}" ]]; then
  echo "!! LOCAL-ONLY SDK override: $CVD_SDK_PATH (artifacts are not publishable)"
  CFG=("--config=patch.\"https://github.com/k0Iry/cvd-core.git\".cvd-indicator-sdk.path=\"$CVD_SDK_PATH\"")
  PKG=(--allow-local-sdk)
fi
if [[ -n "${CVD_RELEASE_VERSION:-}" ]]; then PKG+=(--version "$CVD_RELEASE_VERSION"); fi
echo "== native unit tests (algorithms)"
cargo test "${CFG[@]}" --locked --release -p cvd-indicator-cvd -p cvd-indicator-flow -p cvd-indicator-example-range
echo "== packaging-output safety + CI workflow policy (local)"
cargo test "${CFG[@]}" --locked --release -p cvd-indicators-conformance --test dist_safety --test workflow_policy
echo "== package public allowlist (wasm-release, reproducible, manifest + SHA256SUMS)"
cargo run "${CFG[@]}" --locked --release -q -p cvd-indicators-conformance --bin package -- "$OUT/dist" "${PKG[@]}" > "$OUT/package.log"
( cd "$OUT/dist" && sha256sum -c SHA256SUMS )
echo "== cvd-wasm v1 conformance on the packaged .wasm"
CVD_WASM_DIR="$OUT/dist" cargo test "${CFG[@]}" --locked --release -p cvd-indicators-conformance --test wasm
CVD_WASM_DIR="$OUT/dist" cargo run "${CFG[@]}" --locked --release -q -p cvd-indicators-conformance --bin corpus -- "$OUT/corpus.json"
if [[ -n "${CVD_WASM_HOST_DIR:-}" ]]; then
  echo "== real Chromium through the actual cvd-core browser host"
  echo "-- harness failure path: early browser death must settle bounded and clean up"
  CVD_WASM_DIR="$OUT/dist" CVD_CORPUS="$OUT/corpus.json" CVD_SMOKE_FAULT=kill-browser node conformance/browser/public-smoke.mjs
  CVD_WASM_DIR="$OUT/dist" CVD_CORPUS="$OUT/corpus.json" node conformance/browser/public-smoke.mjs
elif [[ "${CVD_REQUIRE_BROWSER:-0}" == 1 ]]; then
  echo "CVD_WASM_HOST_DIR required" >&2; exit 1
else
  echo "!! browser smoke skipped (CVD_WASM_HOST_DIR unset)"
fi
cargo clippy "${CFG[@]}" --locked --workspace --all-targets -- -D warnings
echo "PUBLIC GATE PASS: $OUT/dist"
