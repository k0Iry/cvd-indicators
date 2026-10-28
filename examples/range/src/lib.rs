//! `example.range` — a deliberately small, third-party style indicator that
//! proves the extension path without any host internals.
//!
//! Different from the projections in `indicators/`: it has a parameter, a
//! declared warm-up context, real windowed computation, a price-pane overlay
//! and its own separate pane.
//!
//! * `mid` (line, price pane): `(highest high + lowest low) / 2` over `length`
//! * `width` (histogram, separate pane): `highest high - lowest low`
//!
//! Bars before a full window are `null` (gap), never zero.

use cvd_indicator_sdk::*;
use std::collections::BTreeMap;

pub const ID: &str = "example.range";
pub const MAX_LENGTH: i64 = 500;

pub struct Range {
    length: usize,
    source: String,
}

impl Indicator for Range {
    fn descriptor() -> Descriptor {
        Descriptor {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            name: "Range (example)".into(),
            description: "Example plugin: rolling high/low midline and range width".into(),
            abi: abi::ABI_VERSION,
            // Warm-up must cover the longest allowed window.
            context: ContextSpec {
                bars: MAX_LENGTH as u32,
                per_tf: Default::default(),
            },
            params: vec![
                ParamSpec {
                    key: "length".into(),
                    label: "Length".into(),
                    kind: ParamKind::Int {
                        default: 20,
                        min: 2,
                        max: MAX_LENGTH,
                    },
                },
                ParamSpec {
                    key: "source".into(),
                    label: "Price source".into(),
                    kind: ParamKind::Choice {
                        default: "high_low".into(),
                        options: vec!["high_low".into(), "close".into()],
                    },
                },
            ],
            inputs: vec![InputField::Price, InputField::AggregatePrice],
            outputs: vec![
                OutputSpec {
                    key: "mid".into(),
                    label: "Mid".into(),
                    pane: Pane::Price,
                    pane_group: None,
                    colors: vec!["#bb6bd9".into()],
                    widths: vec![],
                    readout_only: false,
                    kind: OutputKind::Line,
                },
                OutputSpec {
                    key: "width".into(),
                    label: "Range".into(),
                    pane: Pane::Separate,
                    pane_group: Some("range".into()),
                    colors: vec!["#bb6bd9".into(), "#bb6bd9".into()],
                    widths: vec![],
                    readout_only: false,
                    kind: OutputKind::Histogram,
                },
            ],
            live: true,
        }
    }

    fn new(params: &ParamValues) -> Result<Self, PluginError> {
        let length = params
            .int("length")
            .ok_or_else(|| PluginError::invalid_params("length missing"))?
            as usize;
        let source = params.choice("source").unwrap_or("high_low").to_string();
        Ok(Self { length, source })
    }

    fn compute(
        &mut self,
        ctx: &ComputeContext<'_>,
        bars: &[InputBar],
        from: usize,
    ) -> Result<SeriesSet, PluginError> {
        let agg = ctx.scope.mode == Mode::Aggregate;
        let close_only = self.source == "close";
        let hi = |b: &InputBar| match (agg, close_only) {
            (true, true) => b.apc,
            (true, false) => b.aph,
            (false, true) => b.price_close,
            (false, false) => b.ph,
        };
        let lo = |b: &InputBar| match (agg, close_only) {
            (true, true) => b.apc,
            (true, false) => b.apl,
            (false, true) => b.price_close,
            (false, false) => b.pl,
        };
        let n = self.length;
        let (mut mid, mut width) = (Vec::new(), Vec::new());
        for i in from..bars.len() {
            if i + 1 < n {
                mid.push(None);
                width.push(None);
                continue;
            }
            let w = &bars[i + 1 - n..=i];
            let h = w.iter().map(hi).fold(f64::NEG_INFINITY, f64::max);
            let l = w.iter().map(lo).fold(f64::INFINITY, f64::min);
            mid.push(Some((h + l) / 2.0));
            width.push(Some(h - l));
        }
        Ok(BTreeMap::from([
            ("mid".to_string(), SeriesValues::Line { values: mid }),
            (
                "width".to_string(),
                SeriesValues::Histogram { values: width },
            ),
        ]))
    }
}

export_indicator!(Range);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_valid_and_params_resolve() {
        let d = Range::descriptor();
        d.validate().unwrap();
        let p = resolve_params(&d, b"{}").unwrap();
        assert_eq!(p.int("length"), Some(20));
        assert!(resolve_params(&d, br#"{"length":1}"#).is_err());
        assert!(resolve_params(&d, br#"{"source":"open"}"#).is_err());
        assert!(resolve_params(&d, br#"{"nope":1}"#).is_err());
    }
}
