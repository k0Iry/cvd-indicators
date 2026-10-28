//! Generates the browser fixture consumed by cvd-core-ui tests:
//! REAL public cdylib output computed by the REAL runner, wrapped in the
//! host web envelope documented in cvd-core docs/host-indicator-integration.md
//! (`IndicatorHistory`/`ModeResult` for /history, `indicator` frames for /ws,
//! /indicators discovery). The envelope wrapping is written here with
//! serde_json because the server types live in a binary crate; field names
//! follow those types exactly. Data is synthetic and public.
//!
//! Usage: CVD_INDICATOR_RUNNER=... cargo run -p cvd-indicators-conformance --bin web-fixture -- <out.json>

use cvd_indicator_sdk::{InputBar, LiveOp, Mode};
use cvd_indicators_conformance::{bar, library, runner_binary, scope, Reply, Runner};
use serde_json::{json, Value};

const PACKAGES: [&str; 3] = [
    "cvd-indicator-cvd",
    "cvd-indicator-flow",
    "cvd-indicator-example-range",
];
const BARS: i64 = 240;

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::Source => "source",
        Mode::Aggregate => "aggregate",
    }
}

fn mode_result(reply: Reply) -> Value {
    match reply {
        Reply::Ok(output) => json!({"status": "ok", "output": output}),
        Reply::Err(error) => json!({"status": "unavailable", "error": error}),
    }
}

fn main() {
    let out = std::env::args().nth(1).expect("output path");
    // Canonical CVD time = last second of the bucket (never shifted).
    let bars: Vec<InputBar> = (0..BARS)
        .map(|i| {
            let mut b = bar(i);
            b.time = 1_700_000_039 + 60 * i;
            // A few genuine-zero and unreconciled flow bars for gap/zero cases.
            if i % 37 == 5 {
                b.buy_volume_usdt = 0.0;
                b.sell_volume_usdt = 0.0;
                b.delta_usdt = 0.0;
            }
            if i % 41 == 7 {
                b.delta_usdt += 17.0;
            }
            b
        })
        .collect();
    let next = {
        let mut b = bar(BARS);
        b.time = 1_700_000_039 + 60 * BARS;
        b
    };
    let mut corrected = next.clone();
    corrected.price_close += 3.0;
    corrected.buy_volume_usdt += 100.0;
    corrected.delta_usdt += 100.0;
    corrected.cvd_usdt += 100.0;

    let mut discovery = Vec::new();
    let mut history = serde_json::Map::new();
    let mut frames = Vec::new();
    let generation = 1_700_000_000_000u64;
    for (n, package) in PACKAGES.iter().enumerate() {
        let mut r = Runner::spawn(&runner_binary(), &library(package)).expect("runner");
        let d = r.descriptor.clone();
        let desc_json: Value = serde_json::from_slice(&r.descriptor_json).unwrap();
        discovery.push(
            json!({"id": d.id, "status": "ready", "modes": ["source", "aggregate"],
            "version": d.version, "descriptor": desc_json, "error": null, "restarts": 0}),
        );
        let ctx = d.context.bars_for("1m") as usize;
        // The synthetic page is the start of history: no older warm-up exists,
        // so every page bar is an output bar (output.times == page bars).
        let keep = 0;
        let mut modes = serde_json::Map::new();
        for mode in [Mode::Source, Mode::Aggregate] {
            // Warm-up rows are used then trimmed: output.times == page bars.
            modes.insert(
                mode_str(mode).into(),
                mode_result(r.history("{}", &scope(mode), keep, &bars)),
            );
        }
        history.insert(d.id.clone(), json!({"version": d.version, "modes": modes}));
        // Live: open on the cached window, append the next bar, then replace it.
        let session = 10 + n as u64;
        let window = &bars[bars.len() - (ctx + 1).min(bars.len())..];
        for mode in [Mode::Source] {
            // as_of_rev = base-frame rev included: open = history snapshot
            // (rev 0), append = rev 1, replace = rev 2 (matches live.*_bar.rev).
            let frame = |seq: u64, reply: Reply| match reply {
                Reply::Ok(output) => {
                    json!({"type": "indicator", "api_version": 1, "id": d.id, "version": d.version,
                    "mode": mode_str(mode), "generation": generation, "session": session, "seq": seq,
                    "as_of_rev": seq, "output": output})
                }
                Reply::Err(e) => panic!("live {}: {e}", d.id),
            };
            frames.push(frame(
                0,
                r.live_open(session, generation, "{}", &scope(mode), window),
            ));
            frames.push(frame(
                1,
                r.live_update(
                    session,
                    generation,
                    1,
                    vec![LiveOp::Append { bar: next.clone() }],
                ),
            ));
            frames.push(frame(
                2,
                r.live_update(
                    session,
                    generation,
                    2,
                    vec![LiveOp::Replace {
                        bar: corrected.clone(),
                    }],
                ),
            ));
        }
        assert!(r.shutdown().success());
    }
    let keep_from = 0usize;
    let page_bars: Vec<Value> = bars[keep_from..]
        .iter()
        .map(|b| {
            let mut v = serde_json::to_value(b).unwrap();
            v["quality"] = Value::Null;
            v
        })
        .collect();
    let to_bar = |b: &InputBar| {
        let mut v = serde_json::to_value(b).unwrap();
        v["quality"] = Value::Null;
        v
    };
    let fixture = json!({
        "generator": "cvd-indicators/conformance/src/bin/web-fixture.rs",
        "discovery": {"api_version": 1, "indicators": discovery},
        "history": {"series": "spot-v1", "bars": page_bars, "has_more_before": false, "has_more_after": false,
            "oldest_time": bars[0].time, "newest_time": bars.last().unwrap().time, "realtime_enabled": true,
            "indicator_api_version": 1, "indicators": history},
        "live": {
            "append_bar": {"series": "spot-v1", "action": "append", "rev": 1, "bar": to_bar(&next)},
            "replace_bar": {"series": "spot-v1", "action": "update", "rev": 2, "bar": to_bar(&corrected)},
            "indicator_frames": frames,
        },
    });
    std::fs::write(&out, serde_json::to_vec_pretty(&fixture).unwrap()).expect("write fixture");
    eprintln!("wrote {out}");
}
