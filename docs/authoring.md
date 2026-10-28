# Build an independent indicator library

The SDK/ABI has one authority: public cvd-core revision `81e77770902b7e760c66905eec473d66fb6e2fe3`.
Libraries only compute values; the host owns market data, persistence, input
validation, process supervision and browser rendering. Never put JavaScript
or host mutation commands in library output. The first supported target is Linux.

## A standalone crate, outside this workspace

1. Create a directory outside this checkout, for example `$HOME/my-range-indicator`.
2. Copy `examples/range/src/lib.rs` into its `src/lib.rs`.
3. Use this complete `Cargo.toml`:

```toml
[package]
name = "my-range-indicator"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
cvd-indicator-sdk = { git = "https://github.com/k0Iry/cvd-core.git", rev = "81e77770902b7e760c66905eec473d66fb6e2fe3" }
serde_json = "1.0"

[profile.release]
panic = "unwind"
```

Run `cargo build --release` in that directory. The output is
`target/release/libmy_range_indicator.so`; it uses no sibling/private crates.
Before installing alongside the example, change its descriptor id from
`example.range` to your own stable namespaced id and update the display name.
Keep configured id and descriptor id identical.

## Implement the contract

Follow the copied example's `Indicator` implementation and
`export_indicator!` entry point. Its descriptor declares parameters, output
kinds, pane placement, styling, and bounded historical context. Compute from
`ComputeContext`, including its explicit history/live phase and requested
mode; emit one output position per required input time. Emit `None`/JSON null
for unavailable points, never a fake zero or a shorter reindexed array.

Keep `panic = "unwind"` so the SDK catches ordinary Rust panics at the C ABI
boundary. Abort, memory faults and hangs still require runner supervision.
The runner invokes a versioned C-compatible ABI, not Rust's native ABI.

## Integrate and verify

Build/install the same pinned public `cvd-indicator-runner` as in the root
README. List the absolute library path and matching id in the operator's core
TOML config. Reconfigure/restart core; recompiling core is not required.

Use `conformance/` as the real-process test pattern. Test valid and invalid
parameters, incomplete context, both source/aggregate modes, append/forming
replacement, restart/replay, gaps, non-finite rejection and declared output
styles. Do not merely call the Rust algorithm directly: exercise the built
`.so` under the separate runner and inspect its actual protocol response.

Only install operator-trusted libraries. A native process boundary contains
ordinary failures; it does not sandbox arbitrary malicious native code.
