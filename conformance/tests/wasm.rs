//! Public conformance through the REAL wasm32 artifacts (cvd-wasm v1 ABI):
//! module profile, descriptor identity, algorithm expectations (ported
//! unchanged from the retired runner suite), live deltas, stateless history.
//! Build artifacts first (README: Testing). Data is synthetic loopback input.

use cvd_indicator_sdk::{
    codes, Descriptor, HistoryRequest, InputBar, LiveInit, LiveOp, LiveUpdate, Mode, Output,
    OutputKind, Pane, PluginError, SeriesValues,
};
use cvd_indicators_conformance::*;

struct P {
    m: WasmPlugin,
    d: Descriptor,
    desc_json: Vec<u8>,
}

impl P {
    fn new(file: &str, params: &str) -> Result<Self, PluginError> {
        let mut m = WasmPlugin::file(file);
        assert_eq!(m.abi, 1);
        let (s, desc_json) = m.describe();
        assert_eq!(s, STATUS_OK);
        let d: Descriptor = serde_json::from_slice(&desc_json).unwrap();
        let (s, b) = m.create(params.as_bytes());
        if s != STATUS_OK {
            assert_eq!(s, STATUS_ERROR);
            return Err(serde_json::from_slice(&b).unwrap());
        }
        Ok(Self { m, d, desc_json })
    }
    fn call(&mut self, op: u32, body: Vec<u8>) -> Result<Output, PluginError> {
        match self.m.invoke(op, &body) {
            (STATUS_OK, b) => Ok(serde_json::from_slice(&b).unwrap()),
            (STATUS_ERROR, b) => Err(serde_json::from_slice(&b).unwrap()),
            (s, _) => panic!("status {s}"),
        }
    }
    fn history(
        &mut self,
        mode: Mode,
        from: usize,
        bars: &[InputBar],
    ) -> Result<Output, PluginError> {
        let b = serde_json::to_vec(&HistoryRequest {
            scope: scope(mode),
            revision: 1,
            output_from: from,
            bars: bars.to_vec(),
        })
        .unwrap();
        self.call(OP_HISTORY, b)
    }
    fn live_init(
        &mut self,
        rev: u64,
        mode: Mode,
        bars: &[InputBar],
    ) -> Result<Output, PluginError> {
        let b = serde_json::to_vec(&LiveInit {
            scope: scope(mode),
            revision: rev,
            bars: bars.to_vec(),
        })
        .unwrap();
        self.call(OP_LIVE_INIT, b)
    }
    fn live_update(&mut self, rev: u64, seq: u64, ops: Vec<LiveOp>) -> Result<Output, PluginError> {
        let b = serde_json::to_vec(&LiveUpdate {
            revision: rev,
            seq,
            ops,
        })
        .unwrap();
        self.call(OP_LIVE_UPDATE, b)
    }
}

fn bars(n: i64) -> Vec<InputBar> {
    (0..n).map(bar).collect()
}

fn line(out: &Output, key: &str) -> Vec<Option<f64>> {
    match &out.series[key] {
        SeriesValues::Line { values } | SeriesValues::Histogram { values } => values.clone(),
        other => panic!("{key}: unexpected {other:?}"),
    }
}

fn conformance(file: &str, params: &str) {
    let mut r = P::new(file, params).unwrap();
    let d = r.d.clone();
    d.validate().unwrap();
    assert!(!d.version.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&r.desc_json).unwrap();
    assert_eq!(
        json["abi"], 1,
        "descriptor JSON is the SDK document verbatim"
    );
    let ctx = d.context.bars_for("1m") as usize;
    let input = bars(ctx as i64 + 40);
    for mode in [Mode::Source, Mode::Aggregate] {
        let out = r.history(mode, ctx, &input).unwrap();
        out.validate(
            &d,
            Some(&input[ctx..].iter().map(|b| b.time).collect::<Vec<_>>()),
        )
        .unwrap();
        assert_eq!(r.history(mode, ctx, &input).unwrap(), out, "deterministic");
    }
    let mut bad = input.clone();
    bad.swap(1, 2);
    assert_eq!(
        r.history(Mode::Source, 0, &bad).unwrap_err().code,
        codes::INVALID_INPUT
    );
    assert_eq!(
        P::new(file, r#"{"unknown":1}"#).err().unwrap().code,
        codes::INVALID_PARAMS
    );
}

#[test]
fn modules_match_the_cvd_wasm_v1_profile() {
    let allowed = [
        "__data_end",
        "__heap_base",
        "cvd_alloc",
        "cvd_create",
        "cvd_dealloc",
        "cvd_describe",
        "cvd_destroy",
        "cvd_invoke",
        "cvd_result_len",
        "cvd_result_ptr",
        "cvd_wasm_abi_version",
        "memory",
    ];
    for (_, file, id) in PUBLIC {
        let bytes = wasm_bytes(file);
        assert!(bytes.len() <= 16 << 20, "{file} too large");
        assert_eq!(&bytes[..8], b"\0asm\x01\0\0\0", "{file}: core module v1");
        let mut m = WasmPlugin::load(&bytes);
        assert_eq!(m.abi, 1);
        let max = m
            .memory_max_pages()
            .expect("declared memory maximum required");
        assert!(max <= 8192, "{file}: max {max} pages");
        assert!(m.memory_bytes() <= 256 * 65536, "{file}: initial memory");
        for e in &m.exports {
            assert!(
                allowed.contains(&e.as_str()),
                "{file}: unexpected export {e}"
            );
        }
        for r in &allowed[2..] {
            assert!(m.exports.iter().any(|e| e == r), "{file}: missing {r}");
        }
        let (s, d) = m.describe();
        assert_eq!(s, STATUS_OK);
        let d: Descriptor = serde_json::from_slice(&d).unwrap();
        assert_eq!(d.id, id);
        // ABI misuse is rejected with a status, never a trap.
        assert_eq!(
            m.invoke(OP_HISTORY, b"{}").0,
            STATUS_INVALID_ARGUMENT,
            "invoke before create"
        );
        assert_eq!(m.create(b"{}").0, STATUS_OK);
        assert_eq!(m.create(b"{}").0, STATUS_INVALID_ARGUMENT, "second create");
        assert_eq!(m.invoke(9, b"{}").0, STATUS_INVALID_ARGUMENT, "unknown op");
        m.destroy();
        m.destroy();
    }
}

#[test]
fn cvd_projects_established_cumulative_values() {
    let mut r = P::new("cvd_indicator_cvd.wasm", "{}").unwrap();
    assert_eq!(r.d.id, "cvd");
    assert_eq!(r.d.outputs.len(), 1);
    assert_eq!(r.d.outputs[0].kind, OutputKind::Line);
    assert_eq!(r.d.outputs[0].pane, Pane::Separate);
    conformance("cvd_indicator_cvd.wasm", "{}");
    let input = bars(50);
    let src = r.history(Mode::Source, 0, &input).unwrap();
    assert_eq!(
        line(&src, "cvd"),
        input.iter().map(|b| Some(b.cvd_usdt)).collect::<Vec<_>>()
    );
    let agg = r.history(Mode::Aggregate, 0, &input).unwrap();
    assert_eq!(
        line(&agg, "cvd"),
        input.iter().map(|b| Some(b.ac)).collect::<Vec<_>>()
    );
    let summed: f64 = input.iter().map(|b| b.delta_usdt).sum();
    assert_ne!(
        line(&src, "cvd").last().unwrap().unwrap(),
        summed,
        "projection, not recomputation"
    );
    let tail = r.history(Mode::Source, 30, &input).unwrap();
    assert_eq!(
        line(&tail, "cvd"),
        line(&src, "cvd")[30..].to_vec(),
        "windowing never changes a value"
    );
    // binary64 transport: edge values survive JSON bit-exactly.
    let e = edge_bars(20);
    let o = r.history(Mode::Source, 0, &e).unwrap();
    for (v, b) in line(&o, "cvd").iter().zip(&e) {
        assert_eq!(v.unwrap().to_bits(), b.cvd_usdt.to_bits());
    }
    let o = r.history(Mode::Aggregate, 0, &e).unwrap();
    for (v, b) in line(&o, "cvd").iter().zip(&e) {
        assert_eq!(v.unwrap().to_bits(), b.ac.to_bits());
    }
}

#[test]
fn cvd_live_replace_never_double_counts() {
    let mut r = P::new("cvd_indicator_cvd.wasm", "{}").unwrap();
    let input = bars(5);
    let init = r.live_init(3, Mode::Source, &input).unwrap();
    assert_eq!(line(&init, "cvd"), vec![Some(input[4].cvd_usdt)]);
    let mut next = bar(5);
    let appended = r
        .live_update(3, 1, vec![LiveOp::Append { bar: next.clone() }])
        .unwrap();
    assert_eq!(line(&appended, "cvd"), vec![Some(next.cvd_usdt)]);
    for (seq, v) in [(2, 123.0), (3, 456.0)] {
        next.cvd_usdt = v;
        let out = r
            .live_update(3, seq, vec![LiveOp::Replace { bar: next.clone() }])
            .unwrap();
        assert_eq!(
            line(&out, "cvd"),
            vec![Some(v)],
            "replaced value, not a sum"
        );
    }
    assert_eq!(
        r.live_update(2, 4, vec![LiveOp::Append { bar: bar(6) }])
            .unwrap_err()
            .code,
        codes::STALE_GENERATION
    );
    assert_eq!(
        r.live_update(3, 9, vec![LiveOp::Append { bar: bar(6) }])
            .unwrap_err()
            .code,
        codes::SEQUENCE_GAP
    );
}

#[test]
fn flow_conformance_scopes_reconciliation_and_zero() {
    let mut r = P::new("cvd_indicator_flow.wasm", "").unwrap();
    assert_eq!(r.d.id, "flow");
    let kinds: Vec<_> =
        r.d.outputs
            .iter()
            .map(|o| (o.key.as_str(), o.kind.clone(), o.pane_group.clone()))
            .collect();
    assert_eq!(
        kinds,
        vec![
            ("delta", OutputKind::Histogram, Some("flow".into())),
            ("buy_sell", OutputKind::DualRange, Some("flow".into())),
            ("total", OutputKind::Line, Some("flow".into())),
        ]
    );
    assert!(r.d.outputs[2].readout_only && !r.d.outputs[0].readout_only);
    conformance("cvd_indicator_flow.wasm", "");
    let mut input = bars(6);
    input[1].buy_volume_usdt = 0.0;
    input[1].sell_volume_usdt = 0.0;
    input[1].delta_usdt = 0.0;
    input[2].delta_usdt += 5.0;
    input[3].ad += 1000.0;
    input[4].buy_volume_usdt = 1_000_000.0;
    input[4].sell_volume_usdt = 400_000.0;
    input[4].delta_usdt = 600_000.000_5;
    input[5].buy_volume_usdt = 1_000_000.0;
    input[5].sell_volume_usdt = 400_000.0;
    input[5].delta_usdt = 600_001.0;
    let src = r.history(Mode::Source, 0, &input).unwrap();
    assert_eq!(
        line(&src, "delta"),
        input.iter().map(|b| Some(b.delta_usdt)).collect::<Vec<_>>(),
        "delta is a projection"
    );
    let SeriesValues::DualRange { upper, lower } = &src.series["buy_sell"] else {
        panic!()
    };
    let b0 = &input[0];
    assert_eq!(
        (upper[0], lower[0]),
        (Some(b0.buy_volume_usdt), Some(-b0.sell_volume_usdt))
    );
    assert_eq!(
        (upper[1], lower[1]),
        (Some(0.0), Some(0.0)),
        "true zero stays valid"
    );
    assert!(
        lower[1].unwrap().is_sign_positive(),
        "no negative zero on the wire"
    );
    assert_eq!(
        (upper[2], lower[2]),
        (None, None),
        "unreconciled -> gap, never zero"
    );
    assert!(upper[3].is_some(), "source mode ignores aggregate fields");
    assert_eq!((upper[4], lower[4]), (Some(1_000_000.0), Some(-400_000.0)));
    assert_eq!((upper[5], lower[5]), (None, None));
    let total = line(&src, "total");
    assert_eq!(
        total[0],
        Some(b0.buy_volume_usdt + b0.sell_volume_usdt),
        "Total = canonical buy + sell"
    );
    assert_eq!(total[1], Some(0.0), "true zero total");
    assert_eq!(
        (total[2], total[5]),
        (None, None),
        "Total shares Buy/Sell availability"
    );
    let agg = r.history(Mode::Aggregate, 0, &input).unwrap();
    assert_eq!(
        line(&agg, "delta"),
        input.iter().map(|b| Some(b.ad)).collect::<Vec<_>>()
    );
    let SeriesValues::DualRange { upper, lower } = &agg.series["buy_sell"] else {
        panic!()
    };
    assert_eq!(
        (upper[0], lower[0]),
        (Some(input[0].agb), Some(-input[0].ags)),
        "aggregate uses only agb/ags/ad"
    );
    assert_eq!(
        upper[3], None,
        "aggregate mismatch is a gap, no fallback to source values"
    );
    // The JSON wire keeps null gaps (nullable contract), not zeros.
    let req = HistoryRequest {
        scope: scope(Mode::Source),
        revision: 1,
        output_from: 0,
        bars: input.clone(),
    };
    let (_, raw) = r.m.invoke(OP_HISTORY, &serde_json::to_vec(&req).unwrap());
    let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(v["series"]["buy_sell"]["upper"][2].is_null(), "{v}");
}

#[test]
fn example_extension_has_params_context_and_gaps() {
    let f = "cvd_indicator_example_range.wasm";
    let mut r = P::new(f, r#"{"length":5}"#).unwrap();
    assert_eq!(r.d.id, "example.range");
    assert_eq!(r.d.context.bars, 500);
    conformance(f, r#"{"length":5}"#);
    let input = bars(12);
    let out = r.history(Mode::Source, 0, &input).unwrap();
    let mid = line(&out, "mid");
    let width = line(&out, "width");
    assert!(
        mid[..4].iter().all(Option::is_none) && width[..4].iter().all(Option::is_none),
        "no full window -> gaps, not zero"
    );
    let w = &input[3..8];
    let h = w.iter().map(|b| b.ph).fold(f64::MIN, f64::max);
    let l = w.iter().map(|b| b.pl).fold(f64::MAX, f64::min);
    assert_eq!(mid[7], Some((h + l) / 2.0));
    assert_eq!(width[7], Some(h - l));
    let mut c = P::new(f, r#"{"length":5,"source":"close"}"#).unwrap();
    let close = c.history(Mode::Aggregate, 0, &input).unwrap();
    let hc = input[3..8].iter().map(|b| b.apc).fold(f64::MIN, f64::max);
    let lc = input[3..8].iter().map(|b| b.apc).fold(f64::MAX, f64::min);
    assert_eq!(line(&close, "width")[7], Some(hc - lc));
    let warm = r.history(Mode::Source, 7, &input).unwrap();
    assert_eq!(line(&warm, "mid"), mid[7..].to_vec(), "warm-up honoured");
    assert_eq!(
        P::new(f, r#"{"length":1}"#).err().unwrap().code,
        codes::INVALID_PARAMS
    );
    assert_eq!(
        P::new(f, r#"{"source":"open"}"#).err().unwrap().code,
        codes::INVALID_PARAMS
    );
    let init = r.live_init(1, Mode::Source, &input).unwrap();
    assert_eq!(line(&init, "mid"), vec![mid[11]]);
    let mut b = input[11].clone();
    b.ph += 1000.0;
    let replaced = r
        .live_update(1, 1, vec![LiveOp::Replace { bar: b.clone() }])
        .unwrap();
    let again = r
        .live_update(1, 2, vec![LiveOp::Replace { bar: b }])
        .unwrap();
    assert_eq!(replaced, again, "idempotent replace");
}

#[test]
fn history_is_stateless_and_never_touches_the_live_instance() {
    for (_, file, id) in PUBLIC {
        let params = if id == "example.range" {
            r#"{"length":5}"#
        } else {
            "{}"
        };
        let mut a = P::new(file, params).unwrap();
        let mut b = P::new(file, params).unwrap();
        let input = edge_bars(40);
        let h = a.history(Mode::Source, 0, &input).unwrap();
        a.live_init(1, Mode::Source, &input[..20]).unwrap();
        b.live_init(1, Mode::Source, &input[..20]).unwrap();
        a.history(Mode::Aggregate, 0, &input[..30]).unwrap();
        let ua = a
            .live_update(
                1,
                1,
                vec![LiveOp::Append {
                    bar: input[20].clone(),
                }],
            )
            .unwrap();
        let ub = b
            .live_update(
                1,
                1,
                vec![LiveOp::Append {
                    bar: input[20].clone(),
                }],
            )
            .unwrap();
        assert_eq!(ua, ub, "{id}: live unaffected by interleaved history");
        assert_eq!(
            a.history(Mode::Source, 0, &input).unwrap(),
            h,
            "{id}: A/B/A history identical"
        );
    }
}

#[test]
fn libraries_export_distinct_ids_and_only_public_inputs() {
    let mut ids = Vec::new();
    for (_, file, _) in PUBLIC {
        let r = P::new(file, "{}").unwrap();
        let json = String::from_utf8(r.desc_json.clone()).unwrap();
        for private in ["\"lt\"", "\"lb\"", "\"alt\"", "\"alb\""] {
            assert!(
                !json.contains(private),
                "{file} descriptor references {private}"
            );
        }
        ids.push(r.d.id.clone());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

#[test]
fn corpus_replay_is_deterministic_and_covers_ok_and_errors() {
    let (mut ok, mut err) = (0, 0);
    for c in corpus() {
        let a = replay(&c);
        assert_eq!(a, replay(&c), "{}", c.name);
        for o in &a.1 {
            if o.status == STATUS_OK {
                ok += 1
            } else {
                err += 1
            }
        }
    }
    assert!(ok > 100 && err > 10, "ok={ok} err={err}");
}
