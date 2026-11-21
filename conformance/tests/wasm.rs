//! Public conformance through the REAL wasm32 artifacts (cvd-wasm v1 ABI):
//! module profile, descriptor identity, algorithm expectations (ported
//! unchanged from the retired runner suite), live deltas, stateless history.
//! Build artifacts first (README: Testing). Data is synthetic loopback input.

use cvd_indicator_sdk::{
    codes, Descriptor, HistoryRequest, PeriodBaseline, PeriodKind, InputBar, LiveInit, LiveOp, LiveUpdate, Mode, Output,
    OutputKind, Pane, PluginError, SeriesValues,
};
use cvd_indicators_conformance::*;

struct P {
    m: WasmPlugin,
    d: Descriptor,
    desc_json: Vec<u8>,
    /// Live session mode + canonical bars seen (host-equivalent period refs).
    live_mode: Mode,
    seen: Vec<InputBar>,
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
        Ok(Self { m, d, desc_json, live_mode: Mode::Source, seen: Vec::new() })
    }
    /// Period references as the host supplies them: only for a
    /// periods-declaring descriptor, from the exact canonical bars known.
    fn refs(&mut self, mode: Mode, out: &[InputBar], known: &[InputBar]) -> Option<Vec<PeriodBaseline>> {
        for b in known {
            match self.seen.iter_mut().find(|x| x.time == b.time) {
                Some(x) => *x = b.clone(),
                None => self.seen.push(b.clone()),
            }
        }
        if self.d.context.periods.is_empty() {
            return None;
        }
        let seen = self.seen.clone();
        Some(period_refs(&scope(mode).tf, mode, out, canon(&seen)))
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
        let periods = self.refs(mode, &bars[from.min(bars.len())..], bars);
        let b = serde_json::to_vec(&HistoryRequest {
            scope: scope(mode),
            revision: 1,
            output_from: from,
            bars: bars.to_vec(),
            periods,
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
        self.live_mode = mode;
        let periods = self.refs(mode, &bars[bars.len().saturating_sub(1)..], bars);
        let b = serde_json::to_vec(&LiveInit {
            scope: scope(mode),
            revision: rev,
            bars: bars.to_vec(),
            periods,
        })
        .unwrap();
        self.call(OP_LIVE_INIT, b)
    }
    fn live_update(&mut self, rev: u64, seq: u64, ops: Vec<LiveOp>) -> Result<Output, PluginError> {
        let obs: Vec<InputBar> = ops.iter().map(|o| match o { LiveOp::Append { bar } | LiveOp::Replace { bar } => bar.clone() }).collect();
        let periods = self.refs(self.live_mode, &obs, &obs);
        let b = serde_json::to_vec(&LiveUpdate {
            revision: rev,
            seq,
            ops,
            periods,
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
        periods: None,
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
        if id == "orderbook" {
            continue; // book plugin: no bar history/live operations (see book test)
        }
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
    assert_eq!(ids.len(), 5);
}

fn book_body(bids: &[[f64; 2]], asks: &[[f64; 2]]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "source": {"exchange": "binance", "market": "usdm", "symbol": "BTC"},
        "base": "BTC", "quote": "USDT", "generation": 3, "seq": 99, "exchange_time_ms": 1_700_000_000_000i64,
        "bids": bids, "asks": asks,
        "coverage": {"bid_floor": bids.last().map(|l| l[0]).unwrap_or(0.0), "ask_ceiling": asks.last().map(|l| l[0]).unwrap_or(0.0)}
    }))
    .unwrap()
}

#[test]
fn orderbook_plugin_book_only_contract() {
    let mut m = WasmPlugin::file("cvd_indicator_orderbook.wasm");
    let (s, d) = m.describe();
    assert_eq!(s, STATUS_OK);
    let d: Descriptor = serde_json::from_slice(&d).unwrap();
    d.validate().unwrap();
    assert!(d.is_book() && !d.live);
    assert_eq!(d.outputs[0].kind, OutputKind::PriceLevels);
    assert_eq!(
        m.create(br#"{"rows":5,"group":1.0,"unit":"quote"}"#).0,
        STATUS_OK
    );
    // Bar operations are not accepted by a book plugin.
    assert_eq!(m.invoke(OP_HISTORY, b"{}").0, STATUS_INVALID_ARGUMENT);
    let body = book_body(
        &[[100.4, 1.0], [100.1, 2.0], [99.9, 3.0]],
        &[[100.6, 5.0], [101.2, 1.0]],
    );
    let (s, out) = m.invoke(OP_BOOK, &body);
    assert_eq!(s, STATUS_OK, "{}", String::from_utf8_lossy(&out));
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["unit"], "USDT");
    assert_eq!(v["best_bid"], 100.4);
    assert_eq!(v["rows"][0]["price"], 102.0);
    assert_eq!(v["rows"].as_array().unwrap().len(), 4);
    // Crossed / malformed books are rejected with a plugin error, not a trap.
    let crossed = book_body(&[[101.0, 1.0]], &[[100.0, 1.0]]);
    let (s, e) = m.invoke(OP_BOOK, &crossed);
    assert_eq!(s, STATUS_ERROR);
    let e: PluginError = serde_json::from_slice(&e).unwrap();
    assert_eq!(e.code, codes::INVALID_INPUT);
    assert_eq!(m.invoke(OP_BOOK, b"{").0, STATUS_ERROR);
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

/// Dedicated actual-WASM numeric acceptance for the public order-book plugin
/// (exact/grouped × base/quote, invalid group + recovery, source switch).
/// Expectations are hand-computed, not produced by the plugin's own code.
#[test]
fn orderbook_wasm_numeric_cases() {
    let mut m = WasmPlugin::file("cvd_indicator_orderbook.wasm");
    let eth = |sym: &str| {
        let mut v: serde_json::Value = serde_json::from_slice(&book_body(
            &[[2000.5, 1.0], [2000.25, 2.0], [1999.75, 4.0]],
            &[[2001.0, 0.5], [2001.5, 3.0]],
        ))
        .unwrap();
        v["source"]["symbol"] = sym.into();
        v["base"] = sym.into();
        serde_json::to_vec(&v).unwrap()
    };
    let rows = |m: &mut WasmPlugin, params: &str, body: &[u8]| -> (String, Rows) {
        m.destroy(); // params edit = fresh instance (host reload semantics)
        assert_eq!(m.create(params.as_bytes()).0, STATUS_OK, "{params}");
        let (s, out) = m.invoke(OP_BOOK, body);
        assert_eq!(s, STATUS_OK, "{params}: {}", String::from_utf8_lossy(&out));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let r = v["rows"].as_array().unwrap().iter()
            .map(|r| (r["price"].as_f64().unwrap(), r["bid"].as_f64().unwrap(), r["ask"].as_f64().unwrap()))
            .collect();
        (v["unit"].as_str().unwrap().to_string(), r)
    };
    let body = eth("ETH");
    type Rows = Vec<(f64, f64, f64)>;
    let cases: [(&str, &str, Rows); 4] = [
        (r#"{"rows":5,"group":0.0,"unit":"base"}"#, "ETH",
            vec![(2001.5, 0.0, 3.0), (2001.0, 0.0, 0.5), (2000.5, 1.0, 0.0), (2000.25, 2.0, 0.0), (1999.75, 4.0, 0.0)]),
        (r#"{"rows":5,"group":0.0,"unit":"quote"}"#, "USDT",
            vec![(2001.5, 0.0, 6004.5), (2001.0, 0.0, 1000.5), (2000.5, 2000.5, 0.0), (2000.25, 4000.5, 0.0), (1999.75, 7999.0, 0.0)]),
        (r#"{"rows":5,"group":1.0,"unit":"base"}"#, "ETH",
            vec![(2002.0, 0.0, 3.0), (2001.0, 0.0, 0.5), (2000.0, 3.0, 0.0), (1999.0, 4.0, 0.0)]),
        (r#"{"rows":5,"group":1.0,"unit":"quote"}"#, "USDT",
            vec![(2002.0, 0.0, 6004.5), (2001.0, 0.0, 1000.5), (2000.0, 6001.0, 0.0), (1999.0, 7999.0, 0.0)]),
    ];
    for (params, unit, want) in &cases {
        let (u, got) = rows(&mut m, params, &body);
        assert_eq!((&u[..], &got), (*unit, want), "{params}");
    }
    // Statically permitted but too-wide group: recoverable invalid_params.
    m.destroy();
    assert_eq!(m.create(br#"{"rows":5,"group":10000.0,"unit":"base"}"#).0, STATUS_OK);
    let (s, e) = m.invoke(OP_BOOK, &body);
    assert_eq!(s, STATUS_ERROR);
    let e: PluginError = serde_json::from_slice(&e).unwrap();
    assert_eq!(e.code, codes::INVALID_PARAMS);
    // Recovery by editing params, then source switch (BTC identity).
    let (_, got) = rows(&mut m, r#"{"rows":5,"group":1.0,"unit":"base"}"#, &body);
    assert_eq!(got, cases[2].2);
    let (u, got) = rows(&mut m, r#"{"rows":5,"group":1.0,"unit":"base"}"#, &eth("BTC"));
    assert_eq!((u.as_str(), got), ("BTC", cases[2].2.clone()));
}

/// Optional official-tick grouping on the actual .wasm: legacy params/output
/// identical with or without a host `tick_size`; tick modes without one are a
/// recoverable `invalid_input` (no fallback width); 1/5/10 give distinct exact
/// widths; tick+manual together is `invalid_params`; exact/manual repairs it.
#[test]
fn orderbook_wasm_tick_group_stage() {
    let mut m = WasmPlugin::file("cvd_indicator_orderbook.wasm");
    let plain = book_body(&[[2000.5, 1.0], [1999.75, 4.0]], &[[2001.0, 0.5]]);
    let mut v: serde_json::Value = serde_json::from_slice(&plain).unwrap();
    v["tick_size"] = "0.25".into();
    let ticked = serde_json::to_vec(&v).unwrap();
    let run = |m: &mut WasmPlugin, params: &[u8], body: &[u8]| {
        m.destroy();
        assert_eq!(
            m.create(params).0,
            STATUS_OK,
            "{}",
            String::from_utf8_lossy(params)
        );
        m.invoke(OP_BOOK, body)
    };
    for p in [
        &br#"{"rows":5,"group":1.0,"unit":"base"}"#[..],
        br#"{"rows":5,"group":0.0}"#,
    ] {
        let a = run(&mut m, p, &plain);
        assert_eq!(a.0, STATUS_OK);
        assert_eq!(
            a,
            run(&mut m, p, &ticked),
            "unknown tick_size must not change legacy output"
        );
    }
    let (s, e) = run(&mut m, br#"{"rows":5,"ticks":"5"}"#, &plain);
    assert_eq!(s, STATUS_ERROR);
    let e: PluginError = serde_json::from_slice(&e).unwrap();
    assert_eq!(e.code, codes::INVALID_INPUT, "{}", e.message);
    // Selection switch on the same tick-carrying book: distinct widths
    // (tick 0.25 -> 0.25 / 1.25 / 2.5), hand-computed bid floor / ask ceil.
    let prices = |m: &mut WasmPlugin, p: &[u8]| -> Vec<f64> {
        let (s, out) = run(m, p, &ticked);
        assert_eq!(s, STATUS_OK, "{}", String::from_utf8_lossy(&out));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            (v["best_bid"].as_f64(), v["best_ask"].as_f64()),
            (Some(2000.5), Some(2001.0))
        );
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["price"].as_f64().unwrap())
            .collect()
    };
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"1"}"#),
        vec![2001.0, 2000.5, 1999.75]
    );
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"5"}"#),
        vec![2001.25, 2000.0, 1998.75]
    );
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"10"}"#),
        vec![2002.5, 2000.0, 1997.5]
    );
    // absence after presence (source without metadata): unavailable, then repair
    assert_eq!(
        run(&mut m, br#"{"rows":5,"ticks":"10"}"#, &plain).0,
        STATUS_ERROR
    );
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"10"}"#),
        vec![2002.5, 2000.0, 1997.5]
    );
    // B-1 regression: ticks take precedence over a stored manual group
    // (group=1 + ticks=5 -> 5-tick ladder, not a conflict error); a missing
    // tick never falls back to manual; off restores manual width 1.
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"5","group":1.0}"#),
        vec![2001.25, 2000.0, 1998.75]
    );
    let (s, e) = run(&mut m, br#"{"rows":5,"ticks":"10","group":1.0}"#, &plain);
    assert_eq!(s, STATUS_ERROR);
    assert_eq!(
        serde_json::from_slice::<PluginError>(&e).unwrap().code,
        codes::INVALID_INPUT
    );
    assert_eq!(
        prices(&mut m, br#"{"rows":5,"ticks":"off","group":1.0}"#),
        prices(&mut m, br#"{"rows":5,"group":1.0}"#)
    );
    assert_ne!(
        prices(&mut m, br#"{"rows":5,"ticks":"off","group":1.0}"#),
        prices(&mut m, br#"{"rows":5,"ticks":"5","group":1.0}"#)
    );
    assert_eq!(
        run(&mut m, br#"{"rows":5,"ticks":"off","group":1.0}"#, &plain).0,
        STATUS_OK
    );
    m.destroy();
    assert_ne!(m.create(br#"{"ticks":"2"}"#).0, STATUS_OK);
}

// ------------------------------------------------ UTC-anchored CVD (opt-in)

const MON: i64 = 1_794_787_200; // 2026-11-16 00:00 UTC, Monday
const WEEK: i64 = 7 * 86_400;
const ANCH: &str = "cvd_indicator_cvd_anchored.wasm";

fn hist_p(r: &mut P, tf: &str, mode: Mode, from: usize, bars: &[InputBar], periods: Option<Vec<PeriodBaseline>>) -> Result<Output, PluginError> {
    let b = serde_json::to_vec(&HistoryRequest { scope: scope_tf(mode, tf), revision: 1, output_from: from, bars: bars.to_vec(), periods }).unwrap();
    r.call(OP_HISTORY, b)
}
fn hist_c(r: &mut P, tf: &str, mode: Mode, from: usize, bars: &[InputBar], canon_bars: &[InputBar]) -> Result<Output, PluginError> {
    let p = period_refs(tf, mode, &bars[from..], canon(canon_bars));
    hist_p(r, tf, mode, from, bars, Some(p))
}
fn cum(b: &InputBar, agg: bool) -> f64 {
    if agg { b.ac } else { b.cvd_usdt }
}

#[test]
fn plain_cvd_descriptor_is_unchanged_zero_context() {
    let r = P::new("cvd_indicator_cvd.wasm", "{}").unwrap();
    assert_eq!(r.d.context.max_bars(), 0);
    assert!(r.d.context.periods.is_empty());
    assert!(!String::from_utf8_lossy(&r.desc_json).contains("periods"), "plain descriptor has no periods key");
    assert!(r.d.params.is_empty());
    assert_eq!(
        P::new("cvd_indicator_cvd.wasm", r#"{"anchor":"utc_day"}"#).err().unwrap().code,
        codes::INVALID_PARAMS,
        "plain cvd has no anchor param"
    );
}

#[test]
fn anchored_descriptor_declares_periods_zero_warmup() {
    let r = P::new(ANCH, r#"{"anchor":"utc_week"}"#).unwrap();
    assert_eq!(r.d.id, "cvd_anchored");
    assert_eq!(r.d.context.periods, vec![PeriodKind::UtcDay, PeriodKind::UtcWeek]);
    assert_eq!(r.d.context.max_bars(), 0, "no period-of-bars warm-up (1m week included)");
}

#[test]
fn anchored_day_history_both_modes_value_zero_missing() {
    let mut r = P::new(ANCH, r#"{"anchor":"utc_day"}"#).unwrap();
    let day = continuous_bars(MON - 30 * 60, 60, 60);
    for (mode, agg) in [(Mode::Source, false), (Mode::Aggregate, true)] {
        let v = line(&hist_c(&mut r, "1m", mode, 0, &day, &day).unwrap(), "cvd");
        assert!(v[..30].iter().all(Option::is_none), "Sunday boundary bar absent -> null");
        for (j, x) in v.iter().enumerate().skip(30) {
            assert_eq!(x.unwrap().to_bits(), (cum(&day[j], agg) - cum(&day[29], agg)).to_bits(), "j={j}");
        }
        assert!(v[30].unwrap() != 0.0);
        for from in [0, 29, 30, 31, 59] {
            assert_eq!(line(&hist_c(&mut r, "1m", mode, from, &day, &day).unwrap(), "cvd"), v[from..].to_vec(), "from={from}");
        }
        // Exact zero: cumulative exactly back at the boundary value.
        let mut z = day.clone();
        if agg { z[40].ac = day[29].ac } else { z[40].cvd_usdt = day[29].cvd_usdt }
        let v = line(&hist_c(&mut r, "1m", mode, 30, &z, &z).unwrap(), "cvd");
        assert_eq!(v[10].unwrap().to_bits(), 0, "exact +0.0 {mode:?}");
    }
    // Boundary bar missing in the canonical set -> whole Monday null (not 0,
    // never seeded from 23:58).
    let mut gap = day.clone();
    gap.remove(29);
    assert!(line(&hist_c(&mut r, "1m", Mode::Source, 0, &gap, &gap).unwrap(), "cvd").iter().all(Option::is_none));
    // No references supplied / missing one / non-finite -> rejected, never nulls.
    for p in [None, Some(vec![])] {
        assert_eq!(hist_p(&mut r, "1m", Mode::Source, 0, &day, p).unwrap_err().code, codes::INVALID_INPUT);
    }
}

#[test]
fn anchored_week_late_sunday_1m_without_week_of_bars() {
    let mut r = P::new(ANCH, r#"{"anchor":"utc_week"}"#).unwrap();
    let late = continuous_bars(MON + WEEK - 30 * 60, 60, 40); // Sun 23:30 .. Mon 00:09
    assert_eq!(late[29].time, MON + WEEK - 1);
    let base = InputBar { time: MON - 1, cvd_usdt: 777_000.5, ac: -1_234_567.25, ..Default::default() };
    let lk = |t: i64| if t == MON - 1 { Some(base.clone()) } else { canon(&late)(t) };
    for (mode, agg) in [(Mode::Source, false), (Mode::Aggregate, true)] {
        let p = period_refs("1m", mode, &late, lk);
        let v = line(&hist_p(&mut r, "1m", mode, 0, &late, Some(p)).unwrap(), "cvd");
        for j in 0..30 {
            assert_eq!(v[j].unwrap().to_bits(), (cum(&late[j], agg) - cum(&base, agg)).to_bits(), "late Sunday j={j}");
        }
        for j in 30..40 {
            assert_eq!(v[j].unwrap().to_bits(), (cum(&late[j], agg) - cum(&late[29], agg)).to_bits(), "next Monday reset j={j}");
        }
    }
    let v = line(&hist_c(&mut r, "1m", Mode::Source, 0, &late, &late).unwrap(), "cvd");
    assert!(v[..30].iter().all(Option::is_none) && v[30..].iter().all(Option::is_some), "unknown week ref -> null only for that week");
}

#[test]
fn anchored_live_append_replace_reset() {
    let mut r = P::new(ANCH, r#"{"anchor":"utc_day"}"#).unwrap();
    let day = continuous_bars(MON - 30 * 60, 60, 60);
    let refs = |b: &InputBar| Some(period_refs("1m", Mode::Source, std::slice::from_ref(b), canon(&day)));
    let b = serde_json::to_vec(&LiveInit { scope: scope_tf(Mode::Source, "1m"), revision: 4, bars: day[..29].to_vec(), periods: refs(&day[28]) }).unwrap();
    assert_eq!(line(&r.call(OP_LIVE_INIT, b).unwrap(), "cvd"), vec![None], "Sunday: ref absent");
    let upd = |r: &mut P, seq: u64, op: LiveOp, bar: &InputBar| {
        let b = serde_json::to_vec(&LiveUpdate { revision: 4, seq, ops: vec![op], periods: refs(bar) }).unwrap();
        line(&r.call(OP_LIVE_UPDATE, b).unwrap(), "cvd")
    };
    assert_eq!(upd(&mut r, 1, LiveOp::Append { bar: day[29].clone() }, &day[29]), vec![None]);
    assert_eq!(upd(&mut r, 2, LiveOp::Append { bar: day[30].clone() }, &day[30]), vec![Some(day[30].cvd_usdt - day[29].cvd_usdt)], "midnight reset");
    let mut fix = day[30].clone();
    fix.cvd_usdt += 5.0;
    assert_eq!(upd(&mut r, 3, LiveOp::Replace { bar: fix.clone() }, &fix), vec![Some(fix.cvd_usdt - day[29].cvd_usdt)], "replace, not sum");
    let b = serde_json::to_vec(&LiveUpdate { revision: 4, seq: 4, ops: vec![LiveOp::Append { bar: day[31].clone() }], periods: None }).unwrap();
    assert_eq!(r.call(OP_LIVE_UPDATE, b).unwrap_err().code, codes::INVALID_INPUT, "update lacking refs rejected");
}
