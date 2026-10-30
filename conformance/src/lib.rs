//! cvd-wasm v1 conformance for the public browser indicator modules.
//!
//! [`WasmPlugin`] loads a REAL `wasm32-unknown-unknown` artifact (built with
//! `--profile wasm-release`) with an empty import object and drives exactly
//! the exports a browser host uses: `cvd_alloc` -> copy -> call -> copy the
//! result slot -> `cvd_dealloc`. No server runner and no native library is
//! needed. [`corpus`] is the shared deterministic request script also replayed
//! by the direct-Node and real-Chromium (actual cvd-core host) harnesses and by
//! the temporary `#[ignore]`d native-oracle parity test.

pub mod dist;

use cvd_indicator_sdk::{HistoryRequest, InputBar, LiveInit, LiveOp, LiveUpdate, Mode, Scope};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use wasmi::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

pub const OP_HISTORY: u32 = 1;
pub const OP_LIVE_INIT: u32 = 2;
pub const OP_LIVE_UPDATE: u32 = 3;
pub const STATUS_OK: i32 = 0;
pub const STATUS_ERROR: i32 = 1;
pub const STATUS_INVALID_ARGUMENT: i32 = 3;

/// The public allowlist: (cargo package, wasm file, descriptor id).
pub const PUBLIC: [(&str, &str, &str); 3] = [
    ("cvd-indicator-cvd", "cvd_indicator_cvd.wasm", "cvd"),
    ("cvd-indicator-flow", "cvd_indicator_flow.wasm", "flow"),
    (
        "cvd-indicator-example-range",
        "cvd_indicator_example_range.wasm",
        "example.range",
    ),
];

/// Directory of the wasm-release artifacts: `CVD_WASM_DIR`, else
/// `$CARGO_TARGET_DIR|<workspace>/target` + `wasm32-unknown-unknown/wasm-release`.
/// Build first: `scripts/public-gate.sh <OUT>` (packages into `<OUT>/dist`) or the
/// `package` binary; see README "Testing".
pub fn wasm_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CVD_WASM_DIR") {
        return PathBuf::from(d);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("wasm32-unknown-unknown/wasm-release")
}

pub fn wasm_bytes(file: &str) -> Vec<u8> {
    let p = wasm_dir().join(file);
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!("{}: {e}; build with `cargo build --locked --profile wasm-release --target wasm32-unknown-unknown --workspace --exclude cvd-indicators-conformance`", p.display())
    })
}

/// One instantiation of a real plugin module (one browser Worker equivalent).
pub struct WasmPlugin {
    store: Store<()>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    describe: TypedFunc<(), i32>,
    create: TypedFunc<(i32, i32), i32>,
    invoke: TypedFunc<(i32, i32, i32), i32>,
    destroy: TypedFunc<(), ()>,
    rptr: TypedFunc<(), i32>,
    rlen: TypedFunc<(), i32>,
    pub abi: i32,
    pub exports: Vec<String>,
}

impl WasmPlugin {
    pub fn load(bytes: &[u8]) -> Self {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).expect("valid wasm module");
        assert_eq!(module.imports().count(), 0, "plugin must have no imports");
        let exports = module.exports().map(|e| e.name().to_string()).collect();
        let mut store = Store::new(&engine, ());
        // Empty import object, exactly like the browser Worker.
        let linker = Linker::<()>::new(&engine);
        let inst: Instance = linker
            .instantiate_and_start(&mut store, &module)
            .expect("instantiate");
        let f = |s: &Store<()>, n: &str| {
            inst.get_func(s, n)
                .unwrap_or_else(|| panic!("missing export {n}"))
        };
        macro_rules! t {
            ($n:literal) => {
                f(&store, $n)
                    .typed(&store)
                    .unwrap_or_else(|e| panic!("{}: {e}", $n))
            };
        }
        let abi_f: TypedFunc<(), i32> = t!("cvd_wasm_abi_version");
        let mut me = Self {
            memory: inst.get_memory(&store, "memory").expect("exported memory"),
            alloc: t!("cvd_alloc"),
            dealloc: t!("cvd_dealloc"),
            describe: t!("cvd_describe"),
            create: t!("cvd_create"),
            invoke: t!("cvd_invoke"),
            destroy: t!("cvd_destroy"),
            rptr: t!("cvd_result_ptr"),
            rlen: t!("cvd_result_len"),
            store,
            abi: 0,
            exports,
        };
        me.abi = abi_f.call(&mut me.store, ()).unwrap();
        me
    }

    pub fn file(file: &str) -> Self {
        Self::load(&wasm_bytes(file))
    }

    fn result(&mut self) -> Vec<u8> {
        let p = self.rptr.call(&mut self.store, ()).unwrap() as u32 as usize;
        let n = self.rlen.call(&mut self.store, ()).unwrap() as u32 as usize;
        let mem = self.memory.data(&self.store);
        assert!(
            p.checked_add(n).is_some_and(|e| e <= mem.len()),
            "result region out of bounds"
        );
        mem[p..p + n].to_vec()
    }

    fn with_input(
        &mut self,
        input: &[u8],
        call: impl FnOnce(&mut Self, i32, i32) -> i32,
    ) -> (i32, Vec<u8>) {
        let (ptr, len) = if input.is_empty() {
            (0, 0)
        } else {
            let ptr = self
                .alloc
                .call(&mut self.store, input.len() as i32)
                .unwrap();
            assert_ne!(ptr, 0, "cvd_alloc failed");
            self.memory
                .write(&mut self.store, ptr as u32 as usize, input)
                .expect("input in bounds");
            (ptr, input.len() as i32)
        };
        let status = call(self, ptr, len);
        let out = self.result(); // copy BEFORE dealloc (result slot lifetime)
        if len > 0 {
            self.dealloc.call(&mut self.store, (ptr, len)).unwrap();
        }
        (status, out)
    }

    pub fn describe(&mut self) -> (i32, Vec<u8>) {
        let s = self.describe.call(&mut self.store, ()).unwrap();
        (s, self.result())
    }
    pub fn create(&mut self, params: &[u8]) -> (i32, Vec<u8>) {
        self.with_input(params, |m, p, l| {
            m.create.call(&mut m.store, (p, l)).unwrap()
        })
    }
    pub fn invoke(&mut self, op: u32, body: &[u8]) -> (i32, Vec<u8>) {
        self.with_input(body, |m, p, l| {
            m.invoke.call(&mut m.store, (op as i32, p, l)).unwrap()
        })
    }
    pub fn destroy(&mut self) {
        self.destroy.call(&mut self.store, ()).unwrap()
    }
    pub fn memory_bytes(&self) -> usize {
        self.memory.data(&self.store).len()
    }
    pub fn memory_max_pages(&self) -> Option<u64> {
        self.memory.ty(&self.store).maximum()
    }
}

pub fn scope(mode: Mode) -> Scope {
    Scope {
        exchange: "binance".into(),
        market: "spot".into(),
        symbol: "BTC".into(),
        tf: "1m".into(),
        mode,
    }
}

/// Synthetic loopback bar. `cvd`/`ac` are deliberately NOT the running sum of
/// the deltas, so a projection can be told apart from a recomputation.
pub fn bar(i: i64) -> InputBar {
    let f = i as f64;
    let price = 30_000.0 + 50.0 * (f / 7.0).sin();
    let (buy, sell) = (5_000.0 + 1_000.0 * (f / 3.0).sin(), 5_000.0);
    InputBar {
        time: 1_700_000_040 + 60 * i,
        po: price - 1.0,
        ph: price + 5.0,
        pl: price - 5.0,
        price_close: price,
        apo: price + 9.0,
        aph: price + 15.0,
        apl: price + 5.0,
        apc: price + 10.0,
        cvd_usdt: 1_000_000.0 + 37.0 * f,
        ac: -2_000_000.0 - 11.0 * f,
        delta_usdt: buy - sell,
        ad: 2.0 * (buy - sell),
        bvb: buy / price,
        abvb: 2.0 * buy / price,
        svb: sell / price,
        asvb: 2.0 * sell / price,
        buy_volume_usdt: buy,
        agb: 2.0 * buy,
        sell_volume_usdt: sell,
        ags: 2.0 * sell,
        trades: 10 + i as u64,
    }
}

/// Bars that exercise binary64 transport (non-representable decimals, huge,
/// tiny/subnormal magnitudes, true zeros) and the Flow nullable contract
/// (true zero, unreconciled source, aggregate-only mismatch, fp noise).
pub fn edge_bars(n: i64) -> Vec<InputBar> {
    let mut v: Vec<InputBar> = (0..n).map(bar).collect();
    for (i, b) in v.iter_mut().enumerate() {
        match i % 9 {
            1 => {
                b.buy_volume_usdt = 0.0;
                b.sell_volume_usdt = 0.0;
                b.delta_usdt = 0.0;
            }
            2 => b.delta_usdt += 5.0,
            3 => b.ad += 1000.0,
            4 => {
                b.buy_volume_usdt = 1_000_000.0;
                b.sell_volume_usdt = 400_000.0;
                b.delta_usdt = 600_000.000_5;
            }
            5 => {
                b.cvd_usdt = 0.1 + 0.2;
                b.ac = -1.0e15 - 0.3;
                b.ph += 1.0e-9;
            }
            6 => {
                b.cvd_usdt = 4.9e-324;
                b.ac = -0.0 + 2.2250738585072014e-308;
                b.apc = 123_456_789.123_456_78;
            }
            7 => {
                b.cvd_usdt = 9.007_199_254_740_993e15;
                b.ac = 1.0 / 3.0;
            }
            _ => {}
        }
    }
    v
}

#[derive(Clone, Debug)]
pub struct Step {
    pub label: String,
    pub op: u32,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub package: &'static str,
    pub wasm: &'static str,
    pub params: String,
    pub steps: Vec<Step>,
}

fn hist(label: &str, mode: Mode, from: usize, bars: &[InputBar], rev: u64) -> Step {
    let body = serde_json::to_string(&HistoryRequest {
        scope: scope(mode),
        revision: rev,
        output_from: from,
        bars: bars.to_vec(),
    })
    .unwrap();
    Step {
        label: label.into(),
        op: OP_HISTORY,
        body,
    }
}
fn init(label: &str, mode: Mode, bars: &[InputBar], rev: u64) -> Step {
    let body = serde_json::to_string(&LiveInit {
        scope: scope(mode),
        revision: rev,
        bars: bars.to_vec(),
    })
    .unwrap();
    Step {
        label: label.into(),
        op: OP_LIVE_INIT,
        body,
    }
}
fn upd(label: &str, rev: u64, seq: u64, ops: Vec<LiveOp>) -> Step {
    let body = serde_json::to_string(&LiveUpdate {
        revision: rev,
        seq,
        ops,
    })
    .unwrap();
    Step {
        label: label.into(),
        op: OP_LIVE_UPDATE,
        body,
    }
}

/// Deterministic shared script per public plugin and parameter set: history
/// (source/aggregate, warm-up/output_from, edge values, invalid input), live
/// init + append/replace/multi-op deltas, stale revision and sequence gap,
/// and stateless history interleaved with the live session.
pub fn corpus() -> Vec<Case> {
    let mut out = Vec::new();
    let sets: [(usize, &[&str]); 3] = [
        (0, &["{}"]),
        (1, &["", "{}"]),
        (
            2,
            &[
                r#"{"length":5}"#,
                r#"{"length":5,"source":"close"}"#,
                "{}",
                r#"{"length":2}"#,
                r#"{"length":1}"#,
                r#"{"source":"open"}"#,
                r#"{"unknown":1}"#,
            ],
        ),
    ];
    for (idx, params_list) in sets {
        let (package, wasm, id) = PUBLIC[idx];
        for params in params_list {
            let a = edge_bars(620);
            let mut bad = a[..30].to_vec();
            bad.swap(1, 2);
            let mut nonmono = a[..30].to_vec();
            nonmono[10].time = nonmono[9].time;
            let mut steps = vec![
                hist("history.source.full", Mode::Source, 0, &a, 1),
                hist("history.aggregate.full", Mode::Aggregate, 0, &a, 1),
                hist("history.source.warmup500", Mode::Source, 500, &a, 2),
                hist("history.aggregate.warmup7", Mode::Aggregate, 7, &a[..40], 2),
                hist("history.source.short12", Mode::Source, 0, &a[..12], 3),
                hist("history.empty_output", Mode::Source, 12, &a[..12], 3),
                hist("history.bad_order", Mode::Source, 0, &bad, 4),
                hist("history.non_monotonic", Mode::Source, 0, &nonmono, 4),
            ];
            let base: Vec<InputBar> = a[..560].to_vec();
            let mut next = a[560].clone();
            let mut after = a[561].clone();
            steps.push(init("live.init.source", Mode::Source, &base, 7));
            steps.push(upd(
                "live.append",
                7,
                1,
                vec![LiveOp::Append { bar: next.clone() }],
            ));
            // Stateless history interleaved with the persistent live session.
            steps.push(hist("history.interleaved", Mode::Aggregate, 3, &a[..50], 8));
            next.cvd_usdt = 123.0;
            next.ph += 7.0;
            next.delta_usdt = 0.0;
            next.buy_volume_usdt = 0.0;
            next.sell_volume_usdt = 0.0;
            steps.push(upd(
                "live.replace1",
                7,
                2,
                vec![LiveOp::Replace { bar: next.clone() }],
            ));
            next.cvd_usdt = 456.0;
            steps.push(upd(
                "live.replace2",
                7,
                3,
                vec![LiveOp::Replace { bar: next.clone() }],
            ));
            steps.push(upd(
                "live.replace_same",
                7,
                4,
                vec![LiveOp::Replace { bar: next.clone() }],
            ));
            after.delta_usdt += 3.0; // unreconciled -> Flow gap
            steps.push(upd(
                "live.multi_replace_append",
                7,
                5,
                vec![
                    LiveOp::Replace { bar: next.clone() },
                    LiveOp::Append { bar: after.clone() },
                ],
            ));
            steps.push(upd(
                "live.stale_revision",
                6,
                6,
                vec![LiveOp::Append {
                    bar: a[562].clone(),
                }],
            ));
            steps.push(upd(
                "live.sequence_gap",
                7,
                9,
                vec![LiveOp::Append {
                    bar: a[562].clone(),
                }],
            ));
            steps.push(upd(
                "live.append_after_rejections",
                7,
                6,
                vec![LiveOp::Append {
                    bar: a[562].clone(),
                }],
            ));
            steps.push(hist("history.after_live_repeat", Mode::Source, 0, &a, 1));
            steps.push(init("live.reinit.aggregate", Mode::Aggregate, &a[..100], 9));
            steps.push(upd(
                "live.aggregate.append",
                9,
                1,
                vec![LiveOp::Append {
                    bar: a[100].clone(),
                }],
            ));
            out.push(Case {
                name: format!(
                    "{id}:{}",
                    if params.is_empty() { "<empty>" } else { params }
                ),
                package,
                wasm,
                params: params.to_string(),
                steps,
            });
        }
    }
    out
}

/// Outcome of one call: status + raw result-slot bytes (UTF-8 JSON).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: i32,
    pub body: String,
}

/// Replays a case on a fresh real-wasm instantiation: `create` outcome,
/// then one outcome per step (skipped when create failed).
pub fn replay(case: &Case) -> (Outcome, Vec<Outcome>) {
    let mut p = WasmPlugin::file(case.wasm);
    let (s, b) = p.create(case.params.as_bytes());
    let created = Outcome {
        status: s,
        body: String::from_utf8(b).unwrap(),
    };
    let mut outs = Vec::new();
    if s == STATUS_OK {
        for st in &case.steps {
            let (s, b) = p.invoke(st.op, st.body.as_bytes());
            outs.push(Outcome {
                status: s,
                body: String::from_utf8(b).unwrap(),
            });
        }
    }
    p.destroy();
    (created, outs)
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Corpus + expected outcomes as JSON (for the Node/Chromium harnesses).
pub fn corpus_json() -> Value {
    let mut cases = Vec::new();
    for c in corpus() {
        let wasm = wasm_bytes(c.wasm);
        let mut d = WasmPlugin::load(&wasm);
        let (ds, desc) = d.describe();
        assert_eq!(ds, STATUS_OK);
        let (created, outs) = replay(&c);
        cases.push(json!({
            "name": c.name, "package": c.package, "wasm": c.wasm, "wasm_sha256": sha256_hex(&wasm),
            "params": c.params, "descriptor": String::from_utf8(desc).unwrap(),
            "create": {"status": created.status, "body": created.body},
            "steps": c.steps.iter().zip(outs.iter().map(Some).chain(std::iter::repeat(None))).map(|(s, o)| json!({
                "label": s.label, "op": s.op, "body": s.body,
                "expect": o.map(|o| json!({"status": o.status, "body": o.body})),
            })).collect::<Vec<_>>(),
        }));
    }
    json!({"format": "cvd-wasm-public-corpus/1", "cases": cases})
}
