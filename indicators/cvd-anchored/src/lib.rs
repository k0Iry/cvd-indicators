//! `cvd_anchored` — CVD since the start of the current UTC day or week.
//!
//! Opt-in companion of the plain `cvd` line (which stays a zero-context
//! projection). Value at bar `b` of period `[S, S+len)` is
//! `cum(b) - base(S)`, where `cum` is the host's **established** cumulative of
//! the scope (`cvd_usdt` in source mode, `ac` in aggregate mode — never mixed)
//! and `base(S)` is the host-supplied boundary reference (`context.periods`):
//! the same scope's canonical cumulative of the EXACT bar ending at `S - 1`.
//!
//! - `base == null` (boundary bar absent, invalid or unknown) -> `null` for
//!   that whole period (trustworthy missing: never inferred from an earlier
//!   bar, never 0).
//! - `cum(b) == base` -> exact `0.0`.
//! - No warm-up: a value depends only on its bar and its period reference, so
//!   every timeframe/anchor (including 1m `utc_week`) needs one boundary
//!   lookup per touched period, never a period of bars.
//!
//! The descriptor declares both kinds (the anchor is a runtime param); the
//! host supplies both and the plugin reads the selected one.

use cvd_indicator_sdk::*;
use std::collections::BTreeMap;

pub const ID: &str = "cvd_anchored";
pub const OUTPUT: &str = "cvd";
pub const ANCHORS: [&str; 2] = ["utc_day", "utc_week"];

pub const DAY: i64 = 86_400;
pub const WEEK: i64 = 7 * DAY;

/// Zero warm-up; both UTC period references.
pub fn context() -> ContextSpec {
    ContextSpec { bars: 0, per_tf: BTreeMap::new(), periods: vec![PeriodKind::UtcDay, PeriodKind::UtcWeek], ..ContextSpec::default() }
}

/// Anchored values for `bars[from..]`; `cum(b)` = the scope's cumulative.
pub fn anchored(
    kind: PeriodKind,
    tf_secs: i64,
    bars: &[InputBar],
    periods: &[PeriodBaseline],
    cum: impl Fn(&InputBar) -> f64,
    from: usize,
) -> Result<Vec<Option<f64>>, PluginError> {
    bars[from.min(bars.len())..]
        .iter()
        .map(|b| {
            let s = bar_period_start(kind, tf_secs, b.time);
            let p = periods
                .iter()
                .find(|p| p.kind == kind && p.start == s)
                .ok_or_else(|| PluginError::invalid_input("missing period reference"))?;
            Ok(p.value.map(|base| cum(b) - base).filter(|v| v.is_finite()))
        })
        .collect()
}

pub struct CvdAnchored {
    kind: PeriodKind,
}

impl Indicator for CvdAnchored {
    type Domain = Bars;

    fn descriptor() -> Descriptor {
        Descriptor {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            name: "CVD (UTC reset)".into(),
            description: "Established cumulative taker quote delta since the UTC day/week start (Monday 00:00 UTC), from the host's exact boundary reference; empty when that reference is unknown".into(),
            abi: abi::ABI_VERSION,
            context: context(),
            params: vec![ParamSpec {
                key: "anchor".into(),
                label: "Reset (UTC)".into(),
                kind: ParamKind::Choice {
                    default: "utc_day".into(),
                    options: ANCHORS.iter().map(|s| s.to_string()).collect(),
                },
            }],
            inputs: vec![InputField::Cvd, InputField::AggregateCvd],
            outputs: vec![OutputSpec {
                key: OUTPUT.into(),
                label: "CVD (UTC reset)".into(),
                pane: Pane::Separate,
                pane_group: None,
                colors: vec!["#f2994a".into()],
                widths: vec![],
                readout_only: false,
                kind: OutputKind::Line,
            }],
            live: true,
        }
    }

    fn new(params: &ParamValues) -> Result<Self, PluginError> {
        let kind = match params.choice("anchor") {
            Some("utc_day") => PeriodKind::UtcDay,
            Some("utc_week") => PeriodKind::UtcWeek,
            _ => return Err(PluginError::invalid_params("anchor must be utc_day or utc_week")),
        };
        Ok(Self { kind })
    }

    fn compute(&mut self, BarsInput { ctx, bars, output_from: from }: BarsInput<'_>) -> Result<SeriesSet, PluginError> {
        let tfs = tf_seconds(&ctx.scope.tf).ok_or_else(|| PluginError::invalid_input("unsupported timeframe"))?;
        let aggregate = ctx.scope.mode == Mode::Aggregate;
        let cum = |b: &InputBar| if aggregate { b.ac } else { b.cvd_usdt };
        let values = anchored(self.kind, tfs, bars, ctx.periods, cum, from)?;
        Ok(BTreeMap::from([(OUTPUT.to_string(), SeriesValues::Line { values })]))
    }
}

export_indicator!(Bars: CvdAnchored);

#[cfg(test)]
mod tests {
    use super::*;

    const MON: i64 = 1_794_787_200; // 2026-11-16 00:00 UTC, Monday

    fn scope(tf: &str, mode: Mode) -> Scope {
        Scope { exchange: "x".into(), market: "spot".into(), symbol: "BTC".into(), tf: tf.into(), mode }
    }
    fn bar(t: i64, c: f64, a: f64) -> InputBar {
        InputBar { time: t, cvd_usdt: c, ac: a, ..Default::default() }
    }
    fn pb(kind: PeriodKind, start: i64, value: Option<f64>) -> PeriodBaseline {
        PeriodBaseline { kind, start, value }
    }
    fn run(anchor: &str, tf: &str, mode: Mode, bars: &[InputBar], periods: &[PeriodBaseline], from: usize) -> Result<Vec<Option<f64>>, PluginError> {
        let params = resolve_params(&CvdAnchored::descriptor(), format!(r#"{{"anchor":"{anchor}"}}"#).as_bytes())?;
        let mut c = CvdAnchored::new(&params)?;
        let sc = scope(tf, mode);
        let ctx = ComputeContext { scope: &sc, revision: 1, params: &params, context_bars: 0, phase: ComputePhase::History, periods };
        match c.compute(BarsInput { ctx: &ctx, bars, output_from: from })?.remove(OUTPUT).unwrap() {
            SeriesValues::Line { values } => Ok(values),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn descriptor_declares_both_periods_and_no_warmup() {
        let d = CvdAnchored::descriptor();
        d.validate().unwrap();
        assert_eq!(d.context.periods, vec![PeriodKind::UtcDay, PeriodKind::UtcWeek]);
        for tf in ["1m", "5m", "15m", "30m", "1h"] {
            assert_eq!(d.context.bars_for(tf), 0, "{tf}");
        }
        assert_eq!(resolve_params(&d, b"{}").unwrap().choice("anchor"), Some("utc_day"));
        assert_eq!(resolve_params(&d, br#"{"anchor":"none"}"#).unwrap_err().code, codes::INVALID_PARAMS);
    }

    #[test]
    fn late_sunday_1m_week_nonzero_zero_and_missing_both_modes() {
        let next = MON + WEEK; // following Monday 00:00
        let bars = [bar(next - 61, 150.0, -40.0), bar(next - 1, 100.0, -50.0), bar(next + 59, 80.0, -70.0)];
        let periods = [
            pb(PeriodKind::UtcDay, next - DAY, Some(100.0)),
            pb(PeriodKind::UtcDay, next, None),
            pb(PeriodKind::UtcWeek, MON, Some(25.5)),
            pb(PeriodKind::UtcWeek, next, Some(100.0)),
        ];
        let w = run("utc_week", "1m", Mode::Source, &bars, &periods, 0).unwrap();
        assert_eq!(w, vec![Some(124.5), Some(74.5), Some(-20.0)]);
        let d = run("utc_day", "1m", Mode::Source, &bars, &periods, 0).unwrap();
        assert_eq!(d, vec![Some(50.0), Some(0.0), None], "exact zero and trustworthy null");
        assert_eq!(d[1].unwrap().to_bits(), 0, "+0.0");
        let a = run("utc_week", "1m", Mode::Aggregate, &bars, &periods, 1).unwrap();
        assert_eq!(a, vec![Some(-75.5), Some(-170.0)], "aggregate uses ac");
    }

    #[test]
    fn missing_reference_is_an_input_error_not_a_value() {
        let bars = [bar(MON + 59, 1.0, 1.0)];
        let e = run("utc_day", "1m", Mode::Source, &bars, &[pb(PeriodKind::UtcWeek, MON, Some(0.0))], 0).unwrap_err();
        assert_eq!(e.code, codes::INVALID_INPUT);
        assert!(run("utc_day", "4h", Mode::Source, &bars, &[], 0).is_err());
    }

    #[test]
    fn output_from_never_changes_values() {
        let bars: Vec<InputBar> = (0..30).map(|i| bar(MON - 2 * 3600 + i * 3600 + 3599, i as f64 * 3.0, 0.0)).collect();
        let periods = [
            pb(PeriodKind::UtcDay, MON - DAY, Some(-1.0)),
            pb(PeriodKind::UtcDay, MON, Some(3.0)),
            pb(PeriodKind::UtcDay, MON + DAY, None),
        ];
        let full = run("utc_day", "1h", Mode::Source, &bars, &periods, 0).unwrap();
        assert_eq!(full[2], Some(3.0));
        assert_eq!(full[1], Some(4.0));
        assert!(full[26..].iter().all(Option::is_none));
        for from in [0, 1, 2, 25, 29] {
            assert_eq!(run("utc_day", "1h", Mode::Source, &bars, &periods, from).unwrap(), full[from..].to_vec());
        }
    }
}
