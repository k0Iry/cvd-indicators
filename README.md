# cvd-indicators

Public CVD, Flow and Order Book indicators for the [cvd-core](https://github.com/k0Iry/cvd-core)
browser chart. Each indicator is a single `.wasm` file that you import in your
browser — no plugin is installed or run on the server.

## Use

1. Download the latest published `master` build (the rolling [`dev` pre-release](https://github.com/k0Iry/cvd-indicators/releases/tag/dev)):
   - [`cvd_indicator_cvd.wasm`](https://github.com/k0Iry/cvd-indicators/releases/download/dev/cvd_indicator_cvd.wasm) — cumulative volume delta line in its own pane.
   - [`cvd_indicator_flow.wasm`](https://github.com/k0Iry/cvd-indicators/releases/download/dev/cvd_indicator_flow.wasm) — per-bar delta histogram, buy/sell range and total volume.
   - [`cvd_indicator_orderbook.wasm`](https://github.com/k0Iry/cvd-indicators/releases/download/dev/cvd_indicator_orderbook.wasm) — live price ladder for the BTC/ETH spot and perpetual sources the server offers (Binance, Coinbase): rows per side, optional price grouping and base or quote (notional) units. It shows resting displayed liquidity of the selected source only, within the supplied depth (upstream limits apply; unknown beyond it), and hides all levels while the book is syncing or stale.
2. Optional: compare the downloaded files with [`SHA256SUMS`](https://github.com/k0Iry/cvd-indicators/releases/download/dev/SHA256SUMS); [`manifest.json`](https://github.com/k0Iry/cvd-indicators/releases/download/dev/manifest.json) names the source commit.
3. In the chart click **Indicators → Import**, choose the file, then enable it.

Imported indicators and their settings are stored in your browser for that
site. If you clear site data or switch browser, import them again. CVD and Flow
have no parameters.

Upgrading: deeper book input (up to 5000 levels per side) needs an Order Book
built against the updated SDK — download/rebuild it and import it again.
Updating the server alone does not replace the plugin stored in your browser.

`dev` updates after successful CI if that commit is still the latest on `master`.
Download links may briefly return 404 during replacement; retry. For a fixed
version, choose a `vX.Y.Z` tag on the [releases page](https://github.com/k0Iry/cvd-indicators/releases).
Version v0.1.0 predates Order Book.

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

Local checks: `scripts/public-gate.sh /tmp/cvd-public-gate` (needs Rust, Node 22
and `jq`). The real-Chromium smoke runs only when `CVD_WASM_HOST_DIR` points to a
cvd-core host bundle; set `CVD_REQUIRE_BROWSER=1` to fail instead of skip.
Formal releases come from `vX.Y.Z` tags on `master`.

To write your own indicator, follow the [authoring guide](docs/authoring.md).

## License

Apache-2.0 (see `LICENSE`).
