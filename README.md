# cvd-indicators

Public reference indicator libraries for the CVD host, built on the public SDK
(`cvd-indicator-sdk`, API v1, from `cvd-core`). Each library is a Rust `cdylib`
exporting `cvd_plugin_entry_v1`; the host never loads it in-process — the
independent public `cvd-indicator-runner` does. Existing CVD history is not
imported.

| package | id | outputs | notes |
|---|---|---|---|
| `indicators/cvd` (`cvd-indicator-cvd`) | `cvd` | `cvd` line, separate pane | Projects the host's established cumulative CVD (`cvd_usdt` / aggregate `ac`). Never re-accumulates; context 0. |
| `indicators/flow` (`cvd-indicator-flow`) | `flow` | `delta` histogram, `buy_sell` dual_range, `total` readout-only line (all `pane_group: "flow"`) | Delta = `delta_usdt`/`ad`. Buy/Sell: `upper = buy`, `lower = -sell` from quote turnover, only when non-negative and reconciled with the same-scope delta (1e-9 relative, 1e-6 absolute), else `null` (gap). Total = buy + sell, same availability, shown in the Flow readout in both visual modes. Aggregate mode reads only `agb/ags/ad`. |
| `examples/range` (`cvd-indicator-example-range`) | `example.range` | `mid` line on price, `width` histogram in its own pane | Minimal third-party-style example: params (`length` int 2..=500, `source` choice), declared warm-up 500, windowed computation, `null` before a full window. |
| `conformance` | — | — | Real runner integration tests + browser fixture generator. Not a plugin. |

No private algorithms, private fixtures or numerical oracles live here.

## Metadata, parameters, versions

Descriptors are the SDK `Descriptor` (`cvd-core/docs/plugin-api-v1.md` §4).
Versions come from each crate's `Cargo.toml` (`env!("CARGO_PKG_VERSION")`).
Presentation hints: `colors`, `widths` (empty = renderer default), `readout_only`.

Parameter examples:

```json
{}                                  // cvd, flow: no parameters
{"length": 50}                      // example.range
{"length": 20, "source": "close"}   // example.range, close-only range
{"length": 1}                       // rejected: invalid_params (min 2)
```

## Build

The SDK is pinned to public `cvd-core` revision `81e77770902b7e760c66905eec473d66fb6e2fe3`.
No sibling checkout, private repository or private credentials are needed.
See [authoring an independent library](docs/authoring.md) for a standalone crate.

```sh
cargo build --release --workspace   # target/release/libcvd_indicator_{cvd,flow,example_range}.so
```

`panic = "unwind"` is required (the SDK catches panics at the ABI boundary).

## Load

Operators list libraries by absolute path in the host config (`CVD_CONFIG`,
see `cvd-core/config/cvd.example.toml`); the configured id must equal the
descriptor id. Standalone: `cvd-indicator-runner --library /abs/libcvd_indicator_flow.so`.

## Testing

Integration tests spawn the REAL runner (empty environment, cwd `/`) and load
the freshly built cdylibs. A runner binary is required (no in-process fallback):

```sh
cargo install --locked --git https://github.com/k0Iry/cvd-core --rev 81e77770902b7e760c66905eec473d66fb6e2fe3 cvd-indicator-runner
cargo build --locked --release --workspace
CVD_INDICATOR_RUNNER="$(command -v cvd-indicator-runner)" cargo test --locked --release --workspace
cargo clippy --offline --workspace --all-targets
```

Formatting: only `cargo fmt -p <package>` for this repository's packages —
avoid formatting unrelated development path dependencies.

### Browser fixture

`conformance/src/bin/web-fixture.rs` writes the fixture used by cvd-core
tests: real library output from the real runner wrapped in the host's
`/indicators`, `/history` and typed `/ws` envelopes:

```sh
CVD_INDICATOR_RUNNER=... cargo run -p cvd-indicators-conformance --bin web-fixture -- ../cvd-core/tests/fixtures/plugin-web.json
```

## License

Apache-2.0 (see `LICENSE`).
