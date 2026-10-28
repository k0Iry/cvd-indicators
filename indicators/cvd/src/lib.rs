//! `cvd` — the public CVD line.
//!
//! Projects the host's **established** cumulative volume delta
//! (`cvd_usdt` in source mode, `ac` in aggregate mode). The host owns the
//! canonical cumulative series, its seed and its source/market boundaries;
//! this library never re-accumulates per-bar deltas, so it cannot drift from
//! persistence and needs no warm-up context.

use cvd_indicator_sdk::*;
use std::collections::BTreeMap;

pub const ID: &str = "cvd";
pub const OUTPUT: &str = "cvd";

pub struct Cvd;

impl Indicator for Cvd {
    fn descriptor() -> Descriptor {
        Descriptor {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            name: "Cumulative Volume Delta (CVD)".into(),
            description: "Established cumulative taker quote delta from the host (projection, not recomputed)".into(),
            abi: abi::ABI_VERSION,
            context: ContextSpec { bars: 0, per_tf: Default::default() },
            params: vec![],
            inputs: vec![InputField::Cvd, InputField::AggregateCvd],
            outputs: vec![OutputSpec {
                key: OUTPUT.into(),
                label: "CVD".into(),
                pane: Pane::Separate,
                pane_group: None,
                colors: vec!["#56ccf2".into()],
                widths: vec![],
                readout_only: false,
                kind: OutputKind::Line,
            }],
            live: true,
        }
    }

    fn new(_params: &ParamValues) -> Result<Self, PluginError> {
        Ok(Self)
    }

    fn compute(
        &mut self,
        ctx: &ComputeContext<'_>,
        bars: &[InputBar],
        from: usize,
    ) -> Result<SeriesSet, PluginError> {
        let aggregate = ctx.scope.mode == Mode::Aggregate;
        let values = bars[from..]
            .iter()
            .map(|b| Some(if aggregate { b.ac } else { b.cvd_usdt }))
            .collect();
        Ok(BTreeMap::from([(
            OUTPUT.to_string(),
            SeriesValues::Line { values },
        )]))
    }
}

export_indicator!(Cvd);

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(scope: &Scope, params: &ParamValues) -> ComputeContext<'static> {
        // Leak is fine in tests; keeps lifetimes simple.
        ComputeContext {
            scope: Box::leak(Box::new(scope.clone())),
            revision: 1,
            params: Box::leak(Box::new(params.clone())),
            context_bars: 0,
            phase: ComputePhase::History,
        }
    }

    #[test]
    fn descriptor_is_valid_and_deterministic() {
        let d = Cvd::descriptor();
        d.validate().unwrap();
        assert_eq!(d, Cvd::descriptor());
        assert_eq!(d.context.max_bars(), 0, "projection needs no warm-up");
    }

    #[test]
    fn projects_mode_field_without_accumulating() {
        let bars: Vec<InputBar> = [(60, 10.0, 7.0, 500.0), (120, -3.0, 9.0, -800.0)]
            .iter()
            .map(|&(time, cvd, delta, ac)| InputBar {
                time,
                cvd_usdt: cvd,
                delta_usdt: delta,
                ac,
                ..Default::default()
            })
            .collect();
        let params = ParamValues::default();
        for (mode, want) in [
            (Mode::Source, vec![Some(10.0), Some(-3.0)]),
            (Mode::Aggregate, vec![Some(500.0), Some(-800.0)]),
        ] {
            let scope = Scope {
                exchange: "x".into(),
                market: "spot".into(),
                symbol: "BTC".into(),
                tf: "1m".into(),
                mode,
            };
            let out = Cvd.compute(&ctx(&scope, &params), &bars, 0).unwrap();
            assert_eq!(out[OUTPUT], SeriesValues::Line { values: want });
        }
    }
}
