//! Real runner integration + output conformance for the public libraries.
//!
//! Each test builds the cdylib, spawns the actual public `cvd-indicator-runner`
//! (path from `CVD_INDICATOR_RUNNER`) with an empty environment and talks the
//! v1 frame protocol. All data is synthetic loopback input.

use cvd_indicator_sdk::{
    codes, Descriptor, InputBar, LiveOp, Mode, OutputKind, Pane, SeriesValues,
};
use cvd_indicators_conformance::{bar, library, runner_binary, scope, Runner};

fn spawn(package: &str) -> Runner {
    Runner::spawn(&runner_binary(), &library(package)).expect("runner handshake")
}

fn bars(n: i64) -> Vec<InputBar> {
    (0..n).map(bar).collect()
}

fn line(out: &cvd_indicator_sdk::Output, key: &str) -> Vec<Option<f64>> {
    match &out.series[key] {
        SeriesValues::Line { values } | SeriesValues::Histogram { values } => values.clone(),
        other => panic!("{key}: unexpected {other:?}"),
    }
}

/// Generic conformance every public library must satisfy through the runner.
fn conformance(r: &mut Runner, params: &str) {
    let d: &Descriptor = &r.descriptor;
    d.validate().unwrap();
    assert!(!d.version.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&r.descriptor_json).unwrap();
    assert_eq!(
        json["abi"], 1,
        "descriptor JSON is the SDK document verbatim"
    );
    let desc = d.clone();
    let ctx = desc.context.bars_for("1m") as usize;
    let input = bars(ctx as i64 + 40);
    for mode in [Mode::Source, Mode::Aggregate] {
        let out = r.history(params, &scope(mode), ctx, &input).unwrap();
        out.validate(
            &desc,
            Some(&input[ctx..].iter().map(|b| b.time).collect::<Vec<_>>()),
        )
        .unwrap();
        // Determinism: identical request -> identical output.
        assert_eq!(r.history(params, &scope(mode), ctx, &input).unwrap(), out);
    }
    // Non-finite / non-monotonic input is rejected by the SDK, not computed.
    let mut bad = input.clone();
    bad.swap(1, 2);
    assert_eq!(
        r.history(params, &scope(Mode::Source), 0, &bad)
            .unwrap_err()
            .code,
        codes::INVALID_INPUT
    );
    assert_eq!(
        r.history(r#"{"unknown":1}"#, &scope(Mode::Source), 0, &input)
            .unwrap_err()
            .code,
        codes::INVALID_PARAMS
    );
}

#[test]
fn cvd_projects_established_cumulative_values_through_runner() {
    let mut r = spawn("cvd-indicator-cvd");
    assert_eq!(r.descriptor.id, "cvd");
    assert_eq!(r.descriptor.outputs.len(), 1);
    assert_eq!(r.descriptor.outputs[0].kind, OutputKind::Line);
    assert_eq!(r.descriptor.outputs[0].pane, Pane::Separate);
    conformance(&mut r, "{}");
    let input = bars(50);
    let src = r.history("{}", &scope(Mode::Source), 0, &input).unwrap();
    assert_eq!(
        line(&src, "cvd"),
        input.iter().map(|b| Some(b.cvd_usdt)).collect::<Vec<_>>()
    );
    let agg = r.history("{}", &scope(Mode::Aggregate), 0, &input).unwrap();
    assert_eq!(
        line(&agg, "cvd"),
        input.iter().map(|b| Some(b.ac)).collect::<Vec<_>>()
    );
    // Not a recomputation: the running sum of deltas differs from the output.
    let summed: f64 = input.iter().map(|b| b.delta_usdt).sum();
    assert_ne!(line(&src, "cvd").last().unwrap().unwrap(), summed);
    // Windowing never changes a projected value (no hidden accumulator).
    let tail = r.history("{}", &scope(Mode::Source), 30, &input).unwrap();
    assert_eq!(line(&tail, "cvd"), line(&src, "cvd")[30..].to_vec());
    assert!(r.shutdown().success());
}

#[test]
fn cvd_live_replace_never_double_counts() {
    let mut r = spawn("cvd-indicator-cvd");
    let input = bars(5);
    let s = scope(Mode::Source);
    let init = r.live_open(7, 3, "{}", &s, &input).unwrap();
    assert_eq!(line(&init, "cvd"), vec![Some(input[4].cvd_usdt)]);
    let mut next = bar(5);
    let appended = r
        .live_update(7, 3, 1, vec![LiveOp::Append { bar: next.clone() }])
        .unwrap();
    assert_eq!(line(&appended, "cvd"), vec![Some(next.cvd_usdt)]);
    // Same bucket corrected twice: output is the replaced value, not a sum.
    for v in [123.0, 456.0] {
        next.cvd_usdt = v;
        let seq = if v == 123.0 { 2 } else { 3 };
        let out = r
            .live_update(7, 3, seq, vec![LiveOp::Replace { bar: next.clone() }])
            .unwrap();
        assert_eq!(line(&out, "cvd"), vec![Some(v)]);
    }
    // Stale generation and sequence gaps are rejected by the SDK/runner.
    assert_eq!(
        r.live_update(7, 2, 4, vec![LiveOp::Append { bar: bar(6) }])
            .unwrap_err()
            .code,
        codes::STALE_GENERATION
    );
    assert_eq!(
        r.live_update(7, 3, 9, vec![LiveOp::Append { bar: bar(6) }])
            .unwrap_err()
            .code,
        codes::SEQUENCE_GAP
    );
    assert!(r.shutdown().success());
}

#[test]
fn flow_conformance_scopes_reconciliation_and_zero() {
    let mut r = spawn("cvd-indicator-flow");
    assert_eq!(r.descriptor.id, "flow");
    let kinds: Vec<_> = r
        .descriptor
        .outputs
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
    assert!(r.descriptor.outputs[2].readout_only && !r.descriptor.outputs[0].readout_only);
    conformance(&mut r, "");
    let mut input = bars(6);
    // 0: regular; 1: true zero; 2: unreconciled; 3: aggregate-only mismatch;
    // 4: fp-noise; 5: material mismatch.
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
    let src = r.history("", &scope(Mode::Source), 0, &input).unwrap();
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
    let agg = r.history("", &scope(Mode::Aggregate), 0, &input).unwrap();
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
    assert!(r.shutdown().success());
}

#[test]
fn example_extension_has_params_context_and_gaps() {
    let mut r = spawn("cvd-indicator-example-range");
    assert_eq!(r.descriptor.id, "example.range");
    assert_eq!(r.descriptor.context.bars, 500);
    conformance(&mut r, r#"{"length":5}"#);
    let input = bars(12);
    let out = r
        .history(r#"{"length":5}"#, &scope(Mode::Source), 0, &input)
        .unwrap();
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
    let close = r
        .history(
            r#"{"length":5,"source":"close"}"#,
            &scope(Mode::Aggregate),
            0,
            &input,
        )
        .unwrap();
    let hc = input[3..8].iter().map(|b| b.apc).fold(f64::MIN, f64::max);
    let lc = input[3..8].iter().map(|b| b.apc).fold(f64::MAX, f64::min);
    assert_eq!(line(&close, "width")[7], Some(hc - lc));
    // Warm-up bars are honoured: output only for bars[output_from..].
    let warm = r
        .history(r#"{"length":5}"#, &scope(Mode::Source), 7, &input)
        .unwrap();
    assert_eq!(line(&warm, "mid"), mid[7..].to_vec());
    // Parameter validation is the SDK's, against the descriptor.
    assert_eq!(
        r.history(r#"{"length":1}"#, &scope(Mode::Source), 0, &input)
            .unwrap_err()
            .code,
        codes::INVALID_PARAMS
    );
    assert_eq!(
        r.history(r#"{"source":"open"}"#, &scope(Mode::Source), 0, &input)
            .unwrap_err()
            .code,
        codes::INVALID_PARAMS
    );
    // Live: replace of the forming bar recomputes the window, not accumulates.
    let init = r
        .live_open(1, 1, r#"{"length":5}"#, &scope(Mode::Source), &input)
        .unwrap();
    assert_eq!(line(&init, "mid"), vec![mid[11]]);
    let mut b = input[11].clone();
    b.ph += 1000.0;
    let replaced = r
        .live_update(1, 1, 1, vec![LiveOp::Replace { bar: b.clone() }])
        .unwrap();
    let again = r
        .live_update(1, 1, 2, vec![LiveOp::Replace { bar: b }])
        .unwrap();
    assert_eq!(replaced, again, "idempotent replace");
    assert!(r.shutdown().success());
}

#[test]
fn libraries_export_distinct_ids_and_only_public_inputs() {
    let mut ids = Vec::new();
    for package in [
        "cvd-indicator-cvd",
        "cvd-indicator-flow",
        "cvd-indicator-example-range",
    ] {
        let r = spawn(package);
        let json = String::from_utf8(r.descriptor_json.clone()).unwrap();
        for private in ["\"lt\"", "\"lb\"", "\"alt\"", "\"alb\""] {
            assert!(
                !json.contains(private),
                "{package} descriptor references {private}"
            );
        }
        ids.push(r.descriptor.id.clone());
        assert!(r.shutdown().success());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}
