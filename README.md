# cvd-indicators

Public CVD, Flow and Order Book indicators for the [cvd-core](https://github.com/k0Iry/cvd-core)
browser chart. Each indicator is a single `.wasm` file that you import in your
browser — no plugin is installed or run on the server.

## Use

1. Download from the [v0.1.0 release](https://github.com/k0Iry/cvd-indicators/releases/tag/v0.1.0):
   - [`cvd_indicator_cvd.wasm`](https://github.com/k0Iry/cvd-indicators/releases/download/v0.1.0/cvd_indicator_cvd.wasm) — cumulative volume delta line in its own pane.
   - [`cvd_indicator_flow.wasm`](https://github.com/k0Iry/cvd-indicators/releases/download/v0.1.0/cvd_indicator_flow.wasm) — per-bar delta histogram, buy/sell range and total volume.
2. Optional: compare the downloaded files with [`SHA256SUMS`](https://github.com/k0Iry/cvd-indicators/releases/download/v0.1.0/SHA256SUMS).
3. In the chart click **Indicators → Import**, choose the file, then enable it.

Imported indicators and their settings are stored in your browser for that
site. If you clear site data or switch browser, import them again. CVD and Flow
have no parameters.

`cvd_indicator_orderbook.wasm` (from the next release; build it below until
then) is a live price ladder for Binance USDⓈ-M BTC/ETH: rows per side, optional
price grouping and base or quote (notional) units. It shows resting displayed
liquidity of one source only, and hides all levels while the book is syncing or
stale.

`cvd_indicator_example_range.wasm` in the same release is an **author example**
(rolling high/low midline and range width). Regular users don't need it.

## Build or write your own

Indicators are Rust crates compiled to `wasm32-unknown-unknown` against the
public SDK pinned in `Cargo.toml`. To build this repository's modules:

```sh
cargo build --locked --profile wasm-release --target wasm32-unknown-unknown \
  -p cvd-indicator-cvd -p cvd-indicator-flow -p cvd-indicator-orderbook -p cvd-indicator-example-range
# target/wasm32-unknown-unknown/wasm-release/cvd_indicator_{cvd,flow,orderbook,example_range}.wasm
```

Local checks: `scripts/public-gate.sh /tmp/cvd-public-gate`. Releases are
published by CI from `vX.Y.Z` tags on `master`.

To write your own indicator, follow the [authoring guide](docs/authoring.md).

## License

Apache-2.0 (see `LICENSE`).
