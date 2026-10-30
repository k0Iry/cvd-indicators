# cvd-indicators

Public reference indicators for the CVD browser chart, built on the public SDK
(`cvd-indicator-sdk`, from `cvd-core`) as standalone **cvd-wasm v1** modules
(`wasm32-unknown-unknown`, no imports, no JS glue). Every visitor imports a
`.wasm` locally in the browser; the chart executes it with native WebAssembly
inside an application-owned Worker. There is no server-side plugin execution,
no account and no install step on the server.

| package | wasm asset | id | outputs | notes |
|---|---|---|---|---|
| `indicators/cvd` (`cvd-indicator-cvd`) | `cvd_indicator_cvd.wasm` | `cvd` | `cvd` line, separate pane | Projects the server's established cumulative CVD (`cvd_usdt` / aggregate `ac`). Never re-accumulates; context 0. The authoritative cumulative series stays server-side. |
| `indicators/flow` (`cvd-indicator-flow`) | `cvd_indicator_flow.wasm` | `flow` | `delta` histogram, `buy_sell` dual_range, `total` readout-only line (all `pane_group: "flow"`) | Delta = `delta_usdt`/`ad`. Buy/Sell: `upper = buy`, `lower = -sell`, only when non-negative and reconciled with the same-scope delta (1e-9 relative, 1e-6 absolute), else `null` (gap). Total = buy + sell, same availability. Aggregate mode reads only `agb/ags/ad`. |
| `examples/range` (`cvd-indicator-example-range`) | `cvd_indicator_example_range.wasm` | `example.range` | `mid` line on price, `width` histogram in its own pane | Third-party-style example: params (`length` int 2..=500, `source` choice), declared warm-up 500, `null` before a full window. |
| `conformance` | — | — | — | Executes the real `.wasm` (Rust harness), shared corpus, reproducible packager, real-Chromium smoke. Not a plugin. |

No private algorithms, private fixtures or numerical oracles live here.

## Parameters

```json
{}                                  // cvd, flow: no parameters
{"length": 50}                      // example.range
{"length": 20, "source": "close"}   // example.range, close-only range
{"length": 1}                       // rejected at load: invalid_params (min 2)
```

## Build

The SDK is pinned to an immutable public `cvd-core` revision in `Cargo.toml`
(`[workspace.dependencies]`). No sibling checkout, private repository or
credential is needed.

```sh
cargo build --locked --profile wasm-release --target wasm32-unknown-unknown \
  -p cvd-indicator-cvd -p cvd-indicator-flow -p cvd-indicator-example-range
# target/wasm32-unknown-unknown/wasm-release/cvd_indicator_{cvd,flow,example_range}.wasm
```

`.cargo/config.toml` adds the required linear-memory maximum
(`--max-memory=536870912`); `[profile.wasm-release]` uses `panic = "abort"`
(a trap is fatal to the plugin Worker, which the host retires).

Release packaging (what CI publishes), reproducible and path-remapped:

```sh
cargo run --locked --release -p cvd-indicators-conformance --bin package -- dist
# dist/{cvd_indicator_cvd.wasm, cvd_indicator_flow.wasm, cvd_indicator_example_range.wasm, manifest.json, SHA256SUMS}
```

`manifest.json` records release, ABI (wasm 1, descriptor 1), the SDK git pin,
toolchain and per-asset id/version/size/sha256.

## Use

Download the assets from a GitHub release, check `sha256sum -c SHA256SUMS`, then
in the chart open **Indicators → Import** and pick the `.wasm`. Enable/disable
and parameters are stored locally in your browser.

## Testing

```sh
scripts/public-gate.sh /tmp/cvd-public-gate
```

runs the native algorithm unit tests, packages the allowlist, runs the cvd-wasm
v1 conformance suite against the packaged `.wasm` (module profile, descriptor,
algorithm expectations, history/live/deltas, source/aggregate, nullable gaps,
binary64 bit-exactness, stateless history) and writes the shared corpus. With
`CVD_WASM_HOST_DIR` pointing at a cvd-core `static/wasm-host` copy plus
`cvd_browser_host.wasm`, it also replays the corpus through the actual browser
host in headless Chromium (loopback only, no external requests), then clippy.

Formatting: only `cargo fmt -p <package>` for this repository's packages.

## CI and releases

- Pushes, pull requests and manual `workflow_dispatch` runs: the full gate
  (building the browser host from the same pinned cvd-core revision). Nothing
  is published, including a manual run on a tag.
- Pushing a tag `vX.Y.Z` whose commit is on the default branch `master`: the
  same gate, then the `release` job publishes exactly the five allowlisted
  files above. No target/cache directory is ever uploaded. The job uses the
  `public-release` environment; required reviewers on it are optional repo
  settings and are not enforced by this repository.
- The trigger/publication policy is checked locally by
  `cargo test -p cvd-indicators-conformance --test workflow_policy`.

`package <OUT_DIR>` only writes to a new or empty directory, or to a dist it
created earlier (marker `.cvd-public-dist`, allowlisted files only). Anything
else is refused untouched; outputs are built and validated before an existing
dist is replaced, so a failed build keeps the previous one.

## License

Apache-2.0 (see `LICENSE`).
