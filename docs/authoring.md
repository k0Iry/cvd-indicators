# Author a browser indicator

An indicator is a Rust crate compiled to a standalone WebAssembly module.
Users import the `.wasm` locally in the chart; the browser runs it in an
isolated Worker with **no imports** (no DOM, network, storage, clock or JS).
It computes outputs from bars or order-book snapshots supplied by the host. The normative contract
is cvd-core [`docs/plugin-wasm-v1.md`](https://github.com/k0Iry/cvd-core/blob/master/docs/plugin-wasm-v1.md).

## 1. Create a standalone crate

Requires Rust 1.96+ (a crate outside this checkout does not inherit its
`rust-toolchain.toml`).

Start from the public example [`examples/range`](../examples/range/src/lib.rs):

1. Create a directory outside this checkout, e.g. `$HOME/my-range-indicator`.
2. Copy `examples/range/src/lib.rs` to its `src/lib.rs`.
3. Add `Cargo.toml` (the `rev` is the SDK pin from this repository's `Cargo.toml`):

```toml
[package]
name = "my-range-indicator"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
cvd-indicator-sdk = { git = "https://github.com/k0Iry/cvd-core.git", rev = "cfe735a6e664baf0bf32300b247bedca0e79af7b" }
serde_json = "1.0"

[profile.wasm-release]
inherits = "release"
panic = "abort"
opt-level = 3
lto = true
codegen-units = 1
```

4. Add `.cargo/config.toml` (required: the host rejects modules without a
   memory maximum of at most 512 MiB):

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "link-arg=--max-memory=536870912"]
```

5. Change the descriptor id (`ID`, `"example.range"` in the copied file) to
   your own stable namespaced id, e.g. `"myname.range"`. Keep it unchanged
   across versions.

## 2. Build and import

```sh
rustup target add wasm32-unknown-unknown
cargo build --profile wasm-release --target wasm32-unknown-unknown
# target/wasm32-unknown-unknown/wasm-release/my_range_indicator.wasm
```

No wasm-bindgen, wasm-pack or JavaScript is involved. Import the file in the
chart with **Indicators → Import**; it stays in your browser and nothing is
uploaded.

## 3. Implement the contract

Implement `Indicator` and export it:

```rust
impl Indicator for MyType {
    type Domain = Bars; // or Book for an order-book view
    fn descriptor() -> Descriptor { /* ... */ }
    fn new(params: &ParamValues) -> Result<Self, PluginError> { /* ... */ }
    fn compute(&mut self, i: BarsInput<'_>) -> Result<SeriesSet, PluginError> { /* ... */ }
}
export_indicator!(Bars: MyType);
```

A `Book` plugin's `compute` takes `&BookSnapshot` and returns `PriceLevels`
(see `indicators/orderbook`); export it with `export_indicator!(Book: MyType)`.
To adopt this SDK, migrate older source to the new trait and rebuild. The WASM
v1 ABI is unchanged; older binaries are not deliberately rejected, but are not
a supported compatibility target.

The descriptor declares parameters, outputs, pane placement, styling and
bounded historical context. Bar indicators compute from `i.ctx` (history/live phase and
source/aggregate mode) and return exactly one value per requested bar
`i.bars[i.output_from..]`, aligned to the input.
Use `None` (JSON `null`) for unavailable points — never a fake zero or a
shorter array.

- **History is stateless:** each history request runs on a fresh instance
  built from the load-time parameters and never touches live state.
- **Live** is one persistent instance: `live_init` with a bounded window, then
  `live_update` deltas (`append`/`replace`) with a fixed revision and strictly
  consecutive `seq`. Recompute touched bars; never accumulate across a replaced
  bar.
- Invalid parameters (checked by the SDK against your descriptor) fail the load
  with `invalid_params`.
- A panic traps: the host stops the Worker and the user must re-enable the
  indicator. Return `PluginError` for expected failures.
- Limits: module 16 MiB, input/output 32 MiB, memory 512 MiB, compute deadline
  (default 5 s). Exceeding them stops the indicator, not the chart.

## 4. Test

Follow [`conformance/`](../conformance) as the pattern: load the built `.wasm`
with an empty import object (`WasmPlugin::load`) and assert on its responses —
valid/invalid parameters, incomplete context, both modes, append/replace,
stateless history and gaps. Then import it in the chart to check rendering.
