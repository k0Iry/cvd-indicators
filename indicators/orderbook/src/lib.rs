//! `orderbook` — live single-source price ladder.
//!
//! Input is the host's validated live book of ONE exchange/market/symbol
//! (never an aggregate, never a stale book). Output is `rows` price levels per
//! side, optionally grouped into `group`-wide price buckets (bids round down,
//! asks round up, so a bucket never mixes sides), with quantities in base
//! units or quote notional (`price × quantity`, nominal; no FX conversion).
//! Resting displayed liquidity only: not hidden orders, not executions.

use cvd_indicator_sdk::book::{BookIndicator, BookSnapshot, PriceLevelRow, PriceLevels};
use cvd_indicator_sdk::*;
use std::collections::BTreeMap;

pub const ID: &str = "orderbook";
pub const LADDER: &str = "ladder";

pub struct OrderBook {
    rows: usize,
    group: f64,
    quote: bool,
}

fn round8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Grouped `(price -> qty)` for one side; bids floor, asks ceil.
fn side(
    levels: &[[f64; 2]],
    group: f64,
    ask: bool,
    quote: bool,
    rows: usize,
) -> Result<BTreeMap<u64, (f64, f64)>, PluginError> {
    let mut out: BTreeMap<u64, (f64, f64)> = BTreeMap::new();
    for l in levels {
        let p = if group > 0.0 {
            let k = l[0] / group;
            round8(if ask { k.ceil() } else { k.floor() } * group)
        } else {
            l[0]
        };
        if p.is_nan() || p <= 0.0 {
            // A bid bucket would start at price 0: the group is wider than the
            // supplied bid prices. Recoverable: change `group`.
            return Err(PluginError::new(
                codes::INVALID_PARAMS,
                "price group is wider than the bid price; choose a smaller group",
            ));
        }
        if out.len() >= rows && !out.contains_key(&p.to_bits()) {
            // levels are best-first, so later buckets are further away
            break;
        }
        let q = if quote { l[0] * l[1] } else { l[1] };
        out.entry(p.to_bits()).or_insert((p, 0.0)).1 += q;
    }
    Ok(out)
}

pub fn ladder(book: &BookSnapshot, rows: usize, group: f64, quote: bool) -> Result<PriceLevels, PluginError> {
    let mut merged: BTreeMap<u64, PriceLevelRow> = BTreeMap::new();
    for (ask, levels) in [(false, &book.bids), (true, &book.asks)] {
        for (_, (p, q)) in side(levels, group, ask, quote, rows)? {
            let r = merged.entry(p.to_bits()).or_insert(PriceLevelRow {
                price: p,
                bid: 0.0,
                ask: 0.0,
            });
            if ask {
                r.ask += q
            } else {
                r.bid += q
            }
        }
    }
    let mut rows: Vec<PriceLevelRow> = merged.into_values().collect();
    rows.sort_by(|a, b| b.price.total_cmp(&a.price));
    Ok(PriceLevels {
        unit: if quote {
            book.quote.clone()
        } else {
            book.base.clone()
        },
        best_bid: book.bids[0][0],
        best_ask: book.asks[0][0],
        rows,
    })
}

impl BookIndicator for OrderBook {
    fn descriptor() -> Descriptor {
        Descriptor {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            name: "Order Book".into(),
            description: "Live single-source price ladder (resting displayed liquidity)".into(),
            abi: abi::ABI_VERSION,
            context: ContextSpec::default(),
            params: vec![
                ParamSpec {
                    key: "rows".into(),
                    label: "Rows per side".into(),
                    kind: ParamKind::Int {
                        default: 20,
                        min: 5,
                        max: 100,
                    },
                },
                ParamSpec {
                    key: "group".into(),
                    label: "Price group (0 = exact)".into(),
                    kind: ParamKind::Float {
                        default: 0.0,
                        min: 0.0,
                        max: 10000.0,
                    },
                },
                ParamSpec {
                    key: "unit".into(),
                    label: "Quantity unit".into(),
                    kind: ParamKind::Choice {
                        default: "base".into(),
                        options: vec!["base".into(), "quote".into()],
                    },
                },
            ],
            inputs: vec![InputField::OrderBook],
            outputs: vec![OutputSpec {
                key: LADDER.into(),
                label: "Order Book".into(),
                pane: Pane::Separate,
                pane_group: None,
                colors: vec!["#27ae60".into(), "#eb5757".into()],
                widths: vec![],
                readout_only: false,
                kind: OutputKind::PriceLevels,
            }],
            live: false,
        }
    }

    fn new(p: &ParamValues) -> Result<Self, PluginError> {
        Ok(Self {
            rows: p.int("rows").unwrap_or(20) as usize,
            group: p.float("group").unwrap_or(0.0),
            quote: p.choice("unit") == Some("quote"),
        })
    }

    fn compute_book(&mut self, book: &BookSnapshot) -> Result<PriceLevels, PluginError> {
        ladder(book, self.rows, self.group, self.quote)
    }
}

cvd_indicator_sdk::export_book_indicator!(OrderBook);

#[cfg(test)]
mod tests {
    use super::*;
    use cvd_indicator_sdk::book::{BookCoverage, BookSource};

    fn book() -> BookSnapshot {
        BookSnapshot {
            source: BookSource {
                exchange: "binance".into(),
                market: "usdm".into(),
                symbol: "BTC".into(),
            },
            base: "BTC".into(),
            quote: "USDT".into(),
            generation: 1,
            seq: 10,
            exchange_time_ms: 1,
            bids: vec![[100.4, 1.0], [100.1, 2.0], [99.9, 3.0], [99.0, 4.0]],
            asks: vec![[100.6, 5.0], [100.9, 6.0], [101.2, 7.0]],
            coverage: BookCoverage {
                bid_floor: 99.0,
                ask_ceiling: 101.2,
            },
        }
    }

    #[test]
    fn exact_levels_descending_and_valid() {
        let b = book();
        let o = ladder(&b, 20, 0.0, false).unwrap();
        o.validate(&b).unwrap();
        assert_eq!(o.rows.len(), 7);
        assert_eq!(
            o.rows[0],
            PriceLevelRow {
                price: 101.2,
                bid: 0.0,
                ask: 7.0
            }
        );
        assert_eq!(
            o.rows[6],
            PriceLevelRow {
                price: 99.0,
                bid: 4.0,
                ask: 0.0
            }
        );
        assert_eq!(o.unit, "BTC");
    }

    #[test]
    fn grouping_never_mixes_sides_and_sums() {
        let b = book();
        let o = ladder(&b, 20, 1.0, false).unwrap();
        o.validate(&b).unwrap();
        // bids floor: 100 <- 100.4+100.1 = 3, 99 <- 99.9+99.0 = 7; asks ceil: 101 <- 5+6, 102 <- 7
        let got: Vec<_> = o.rows.iter().map(|r| (r.price, r.bid, r.ask)).collect();
        assert_eq!(
            got,
            vec![
                (102.0, 0.0, 7.0),
                (101.0, 0.0, 11.0),
                (100.0, 3.0, 0.0),
                (99.0, 7.0, 0.0)
            ]
        );
    }

    #[test]
    fn rows_bound_and_quote_notional() {
        let b = book();
        let o = ladder(&b, 1, 0.0, true).unwrap();
        o.validate(&b).unwrap();
        assert_eq!(o.unit, "USDT");
        assert_eq!(o.rows.len(), 2);
        assert_eq!(o.rows[1].bid, 100.4);
        assert_eq!(o.rows[0].ask, 100.6 * 5.0);
    }

    /// C-03: a statically permitted group wider than the bid price must not
    /// render price 0 or drop bids; it is a recoverable invalid-params state.
    #[test]
    fn oversized_group_is_recoverable_invalid_params() {
        let b = book(); // best bid 100.4
        let mut p = OrderBook { rows: 20, group: 10_000.0, quote: false };
        let e = p.compute_book(&b).unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
        // Recovery: editing the parameter on the same instance shape works.
        p.group = 1.0;
        let o = p.compute_book(&b).unwrap();
        o.validate(&b).unwrap();
        assert!(o.rows.iter().all(|r| r.price > 0.0));
        // Any transmitted bid that would floor to bucket 0 is rejected (no
        // silent drop): lowest bid 99.0 < group 100.
        p.group = 100.0;
        assert_eq!(p.compute_book(&b).unwrap_err().code, codes::INVALID_PARAMS);
        // Largest group with positive buckets for every bid: floors to 100/50.
        p.group = 50.0;
        let o = p.compute_book(&b).unwrap();
        o.validate(&b).unwrap();
        let bids: Vec<_> = o.rows.iter().filter(|r| r.bid > 0.0).map(|r| (r.price, r.bid)).collect();
        assert_eq!(bids, vec![(100.0, 3.0), (50.0, 7.0)]);
    }

    #[test]
    fn descriptor_is_a_valid_book_descriptor() {
        let d = OrderBook::descriptor();
        d.validate().unwrap();
        assert!(d.is_book());
    }
}
