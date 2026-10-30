# Author a browser indicator (cvd-wasm v1)

An indicator is a Rust crate compiled to a standalone WebAssembly module.
Users import the `.wasm` file locally in the CVD chart; the browser runs it in
an isolated Worker with **no imports**: no DOM, network, storage, clock or JS.
The module only computes values from the bars the host gives it. The normative
contract is cvd-core `docs/plugin-wasm-v1.md` (ABI v1).

## A standalone crate

1. Create a directory outside this checkout, e.g. `$HOME/my-range-indicator`.
2. Copy `examples/range/src/lib.rs` into its `src/lib.rs`.
3. `Cargo.toml` (use the same cvd-core `rev` as this repository's `Cargo.toml`):

```toml
[package]
name = "my-range-indicator"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
cvd-indicator-sdk = { git = "https://github.com/k0Iry/cvd-core.git", rev = "<CVD_CORE_SDK_REV>" }
serde_json = "1.0"

[profile.wasm-release]
inherits = "release"
panic = "abort"
opt-level = 3
lto = true
codegen-units = 1
```

4. `.cargo/config.toml` (required: the host rejects modules without a declared
   memory maximum <= 512 MiB):

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "link-arg=--max-memory=536870912"]
```

5. Build:

```sh
rustup target add wasm32-unknown-unknown
cargo build --profile wasm-release --target wasm32-unknown-unknown
# target/wasm32-unknown-unknown/wasm-release/my_range_indicator.wasm
```

No wasm-bindgen, wasm-pack or JavaScript is involved. Before sharing, change the
descriptor id from `example.range` to your own stable namespaced id.

## Implement the contract

Implement `Indicator` and call `export_indicator!(MyType)` (on wasm32 it emits
the cvd-wasm exports). The descriptor declares parameters, output kinds, pane
placement, styling and bounded historical context. Compute from
`ComputeContext` (explicit history/live phase and requested source/aggregate
mode); emit one output position per requested bar. Emit `None`/JSON `null`
for unavailable points — never a fake zero or a shorter reindexed array.

Semantics you can rely on:

- **History is stateless.** Every history request runs on a fresh instance
  built from the load-time parameters; it never sees or changes live state.
- **Live** is one persistent instance: `live_init` with a bounded window, then
  `live_update` deltas (`append`/`replace`) with a fixed revision and strictly
  consecutive `seq`. Recompute the touched bars; never accumulate across a
  replaced bar.
- Parameters are validated by the SDK against your descriptor at load; invalid
  parameters fail the load with `invalid_params`.
- A panic is a WebAssembly trap: the host terminates the Worker and the user
  must re-enable the indicator. Return `PluginError` for expected failures.
- Limits: module 16 MiB, input/output 32 MiB, memory 512 MiB, compute deadline
  (default 5 s). Exceeding them stops the plugin, not the chart.

## Verify

Use `conformance/` as the pattern: load the built `.wasm` with an empty import
object (`WasmPlugin::load`) and assert on its actual responses — valid/invalid
parameters, incomplete context, both modes, append/replace, stateless history,
gaps, non-finite rejection and declared output styles. Then import it in the
chart (Indicators → Import) to check rendering. Browser import is local only;
nothing is uploaded.
