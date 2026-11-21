//! `orderbook` — live single-source price ladder.
//!
//! Input is the host's validated live book of ONE exchange/market/symbol
//! (never an aggregate, never a stale book). Output is `rows` price levels per
//! side, optionally grouped into `group`-wide price buckets (bids round down,
//! asks round up, so a bucket never mixes sides), with quantities in base
//! units or quote notional (`price × quantity`, nominal; no FX conversion).
//! Resting displayed liquidity only: not hidden orders, not executions.

use cvd_indicator_sdk::book::{BookSnapshot, PriceLevelRow, PriceLevels};
use cvd_indicator_sdk::*;
use std::collections::BTreeMap;

pub const ID: &str = "orderbook";
pub const LADDER: &str = "ladder";

pub struct OrderBook {
    rows: usize,
    group: f64,
    /// Official-tick multiple (1/5/10); `None` = off (exact or manual group).
    ticks: Option<u8>,
    quote: bool,
}

/// Grouping mode, resolved per compute against the CURRENT book: switching
/// source (BTC/ETH, spot/perp) keeps the selected multiple and recomputes the
/// width from that book's own tick size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grouping<'a> {
    /// `group == 0`: exact levels; `group > 0`: manual decimal width.
    Manual(f64),
    /// `n × tick`, `tick` = host-supplied exact decimal tick size of the
    /// current book (`None` = host did not supply one).
    Ticks(u8, Option<&'a str>),
}

/// Strict positive exact decimal `digits[.digits]` -> `(mantissa, scale)`.
/// No sign, exponent, whitespace or f64 round trip. `None` if malformed,
/// zero, or beyond i128.
fn parse_decimal(s: &str) -> Option<(i128, u32)> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() || (s.contains('.') && frac.is_empty()) {
        return None;
    }
    let scale = u32::try_from(frac.len()).ok()?;
    let mut m: i128 = 0;
    for c in int.bytes().chain(frac.bytes()) {
        let d = c.checked_sub(b'0').filter(|d| *d <= 9)?;
        m = m.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    (m > 0).then_some((m, scale))
}

/// Exact `n × tick` as a decimal: integer mantissa multiply, same scale.
fn tick_width(tick: &str, n: u8) -> Result<(i128, u32), PluginError> {
    let (m, s) = parse_decimal(tick).ok_or_else(|| {
        PluginError::new(
            codes::INVALID_INPUT,
            "exchange tick size is not a valid positive decimal; tick grouping unavailable (use exact or manual group)",
        )
    })?;
    let m = m.checked_mul(i128::from(n)).ok_or_else(|| {
        PluginError::new(
            codes::INVALID_PARAMS,
            "tick group too wide; choose a smaller multiple",
        )
    })?;
    Ok((m, s))
}

/// Host-supplied exact decimal tick size of this book (`context.book:
/// ["tick_size"]`, cvd-core contract v1). Absent = unknown: tick modes then
/// return a recoverable `invalid_input` (`unavailable` is host-reserved), never a fabricated or spacing-derived
/// width.
fn book_tick(book: &BookSnapshot) -> Option<&str> {
    book.tick_size.as_deref()
}

/// Tick-group choice options; each needs the host's `tick_size`.
const TICK_OPTIONS: [&str; 3] = ["1", "5", "10"];

/// A finite positive f64 as the decimal it was written as: the shortest
/// round-trip representation (Rust `Display`), i.e. `mantissa × 10^-scale`.
/// Exchange prices and user groups are decimal strings parsed to the nearest
/// binary64, so this recovers the intended decimal exactly, with no epsilon.
/// `None` beyond i128 precision.
fn decimal(x: f64) -> Option<(i128, u32)> {
    let s = format!("{x}");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let scale = u32::try_from(frac.len()).ok()?;
    let mut m: i128 = 0;
    for c in int.bytes().chain(frac.bytes()) {
        let d = c.checked_sub(b'0').filter(|d| *d <= 9)?;
        m = m.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    Some((m, scale))
}

/// Exact integer grouping of decimal `price` into the decimal `group`
/// (`(mantissa, scale)`): bids floor, asks ceil; the bucket decimal is then
/// converted once to its nearest binary64. `None` beyond i128 precision.
fn bucket(price: f64, (gm, gs): (i128, u32), ask: bool) -> Option<f64> {
    let (pm, ps) = decimal(price)?;
    let s = ps.max(gs);
    let pm = pm.checked_mul(10i128.checked_pow(s - ps)?)?;
    let ga = gm.checked_mul(10i128.checked_pow(s - gs)?)?;
    let (q, r) = (pm / ga, pm % ga);
    let k = if ask && r != 0 { q + 1 } else { q };
    format!("{}e-{gs}", k.checked_mul(gm)?).parse().ok()
}

/// Grouped `(price -> qty)` for one side; bids floor, asks ceil.
fn side(
    levels: &[[f64; 2]],
    group: Option<(i128, u32)>,
    ask: bool,
    quote: bool,
    rows: usize,
) -> Result<BTreeMap<u64, (f64, f64)>, PluginError> {
    let mut out: BTreeMap<u64, (f64, f64)> = BTreeMap::new();
    for l in levels {
        let p = match group {
            Some(g) => bucket(l[0], g, ask).ok_or_else(|| {
                PluginError::new(
                    codes::INVALID_PARAMS,
                    "price group is too fine for these prices; choose a coarser group",
                )
            })?,
            None => l[0],
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

pub fn ladder(
    book: &BookSnapshot,
    rows: usize,
    group: f64,
    quote: bool,
) -> Result<PriceLevels, PluginError> {
    ladder_grouped(book, rows, Grouping::Manual(group), quote)
}

pub fn ladder_grouped(
    book: &BookSnapshot,
    rows: usize,
    grouping: Grouping<'_>,
    quote: bool,
) -> Result<PriceLevels, PluginError> {
    let group = match grouping {
        Grouping::Ticks(n, Some(t)) => Some(tick_width(t, n)?),
        Grouping::Ticks(_, None) => {
            return Err(PluginError::new(
                codes::INVALID_INPUT,
                "exchange tick size not provided for this market; tick grouping unavailable (use exact or manual group)",
            ))
        }
        Grouping::Manual(g) if g > 0.0 => Some(decimal(g).ok_or_else(|| {
            PluginError::new(codes::INVALID_PARAMS, "price group is too fine; choose a coarser group")
        })?),
        Grouping::Manual(_) => None,
    };
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

impl Indicator for OrderBook {
    type Domain = Book;

    fn descriptor() -> Descriptor {
        Descriptor {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            name: "Order Book".into(),
            description: "Live single-source price ladder (resting displayed liquidity)".into(),
            abi: abi::ABI_VERSION,
            context: ContextSpec {
                book: vec![BookMetaField::TickSize],
                book_requires: BTreeMap::from([(
                    "ticks".to_string(),
                    TICK_OPTIONS
                        .iter()
                        .map(|o| (o.to_string(), vec![BookMetaField::TickSize]))
                        .collect(),
                )]),
                ..ContextSpec::default()
            },
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
                    key: "ticks".into(),
                    label: "Tick group (1/5/10 exchange ticks; overrides price group; off = use price group)".into(),
                    kind: ParamKind::Choice {
                        default: "off".into(),
                        options: vec!["off".into(), "1".into(), "5".into(), "10".into()],
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
            ticks: match p.choice("ticks") {
                None | Some("off") => None,
                Some("1") => Some(1),
                Some("5") => Some(5),
                Some("10") => Some(10),
                Some(_) => {
                    return Err(PluginError::new(
                        codes::INVALID_PARAMS,
                        "unknown tick group",
                    ))
                }
            },
            quote: p.choice("unit") == Some("quote"),
        })
    }

    fn compute(&mut self, book: &BookSnapshot) -> Result<PriceLevels, PluginError> {
        // A tick selection takes precedence over any stored manual `group`
        // (which is kept untouched, so `off` resumes it). A missing tick is
        // an error, never a silent fallback to the manual width.
        let grouping = match self.ticks {
            Some(n) => Grouping::Ticks(n, book_tick(book)),
            None => Grouping::Manual(self.group),
        };
        ladder_grouped(book, self.rows, grouping, self.quote)
    }
}

cvd_indicator_sdk::export_indicator!(Book: OrderBook);

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
            tick_size: None,
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
        let mut p = OrderBook {
            rows: 20,
            group: 10_000.0,
            ticks: None,
            quote: false,
        };
        let e = p.compute(&b).unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
        // Recovery: editing the parameter on the same instance shape works.
        p.group = 1.0;
        let o = p.compute(&b).unwrap();
        o.validate(&b).unwrap();
        assert!(o.rows.iter().all(|r| r.price > 0.0));
        // Any transmitted bid that would floor to bucket 0 is rejected (no
        // silent drop): lowest bid 99.0 < group 100.
        p.group = 100.0;
        assert_eq!(p.compute(&b).unwrap_err().code, codes::INVALID_PARAMS);
        // Largest group with positive buckets for every bid: floors to 100/50.
        p.group = 50.0;
        let o = p.compute(&b).unwrap();
        o.validate(&b).unwrap();
        let bids: Vec<_> = o
            .rows
            .iter()
            .filter(|r| r.bid > 0.0)
            .map(|r| (r.price, r.bid))
            .collect();
        assert_eq!(bids, vec![(100.0, 3.0), (50.0, 7.0)]);
    }

    #[test]
    fn descriptor_is_a_valid_book_descriptor() {
        let d = OrderBook::descriptor();
        d.validate().unwrap();
        assert!(d.is_book());
    }

    fn one(bid: f64, ask: f64) -> BookSnapshot {
        BookSnapshot {
            bids: vec![[bid, 1.0]],
            asks: vec![[ask, 2.0]],
            coverage: BookCoverage {
                bid_floor: bid,
                ask_ceiling: ask,
            },
            ..book()
        }
    }

    /// (bid bucket, bid qty, ask bucket, ask qty) of a one-level-per-side book.
    fn sides(o: &PriceLevels) -> (f64, f64, f64, f64) {
        let b = o.rows.iter().find(|r| r.bid > 0.0).unwrap();
        let a = o.rows.iter().find(|r| r.ask > 0.0).unwrap();
        (b.price, b.bid, a.price, a.ask)
    }

    /// Decimal boundaries whose binary64 quotient misrounds (0.29/0.01 =
    /// 28.999999999999996 would floor to 0.28; 0.07/0.01 = 7.000000000000001
    /// would ceil to 0.08) stay in their own bucket. The adjacent binary64
    /// values are distinct prices and group strictly by their exact value.
    #[test]
    fn exact_decimal_boundaries_and_strict_neighbours() {
        let down = |x: f64| f64::from_bits(x.to_bits() - 1);
        let up = |x: f64| f64::from_bits(x.to_bits() + 1);
        assert_eq!(0.29f64 / 0.01, 28.999999999999996);
        assert_eq!(0.07f64 / 0.01, 7.000000000000001);
        let cases = [
            // (bid, ask) -> (bid bucket, ask bucket) at group 0.01
            ((0.29, 0.07), (0.29, 0.07)),
            ((0.01, 0.01), (0.01, 0.01)),
            ((up(0.29), down(0.07)), (0.29, 0.07)),
            ((down(0.29), up(0.07)), (0.28, 0.08)),
            ((up(0.01), down(0.01)), (0.01, 0.01)),
        ];
        for ((bid, ask), (wb, wa)) in cases {
            let b = one(bid, ask);
            let o = ladder(&b, 5, 0.01, false).unwrap();
            o.validate(&b).unwrap();
            assert_eq!(sides(&o), (wb, 1.0, wa, 2.0), "bid {bid:?} ask {ask:?}");
            assert_eq!(
                (o.best_bid.to_bits(), o.best_ask.to_bits()),
                (bid.to_bits(), ask.to_bits())
            );
        }
        // just below the 0.01 bid bucket floors to 0: recoverable, never dropped
        let e = ladder(&one(down(0.01), 0.07), 5, 0.01, false).unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
        // same boundaries in quote units: bucket unchanged, notional exact
        let o = ladder(&one(0.29, 0.07), 5, 0.01, true).unwrap();
        assert_eq!(sides(&o), (0.29, 0.29, 0.07, 0.14));
    }

    /// No eight-decimal merge: prices differing below 1e-8 stay distinct both
    /// exact and grouped at a finer group.
    #[test]
    fn sub_eight_decimal_prices_are_not_merged() {
        let b = one(0.123456781, 0.123456789);
        let o = ladder(&b, 5, 0.000000001, false).unwrap();
        o.validate(&b).unwrap();
        assert_eq!(sides(&o), (0.123456781, 1.0, 0.123456789, 2.0));
        let o = ladder(&b, 5, 0.0, false).unwrap();
        assert_eq!(sides(&o), (0.123456781, 1.0, 0.123456789, 2.0));
    }

    /// Precision beyond i128 is a recoverable params error, not a wrong bucket.
    #[test]
    fn unrepresentable_precision_is_recoverable() {
        assert_eq!(decimal(0.29), Some((29, 2)));
        assert_eq!(decimal(1e300), None);
        let e = ladder(&one(1e-30, 1.0), 5, 3e-30, false).unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
    }

    fn t(b: &BookSnapshot, n: u8, tick: &str) -> Result<PriceLevels, PluginError> {
        ladder_grouped(b, 20, Grouping::Ticks(n, Some(tick)), false)
    }

    #[test]
    fn tick_width_is_exact_decimal_multiply() {
        assert_eq!(tick_width("0.05", 1).unwrap(), (5, 2));
        assert_eq!(tick_width("0.05", 5).unwrap(), (25, 2));
        assert_eq!(tick_width("0.05", 10).unwrap(), (50, 2));
        assert_eq!(tick_width("0.1", 5).unwrap(), (5, 1));
        assert_eq!(tick_width("0.00001", 10).unwrap(), (10, 5));
        // f64 multiply would not be exact: 0.1 * 3 != 0.3
        assert_ne!(0.1f64 * 3.0, 0.3);
        assert_eq!(tick_width("0.1", 3).unwrap(), (3, 1));
        for bad in [
            "", "0", "0.000", "-0.1", "1e-2", " 0.1", "0.", ".5", "abc", "NaN",
        ] {
            assert_eq!(
                tick_width(bad, 1).unwrap_err().code,
                codes::INVALID_INPUT,
                "{bad:?}"
            );
        }
    }

    /// 1/5/10 ticks of 0.05 on true boundaries and ULP neighbours; best
    /// prices preserved raw, base qty preserved, sides never mix.
    #[test]
    fn tick_multiples_true_and_ulp_boundaries() {
        let down = |x: f64| f64::from_bits(x.to_bits() - 1);
        let up = |x: f64| f64::from_bits(x.to_bits() + 1);
        let cases = [
            (1, 100.35, 100.4, 100.35, 100.4),
            (1, up(100.35), down(100.4), 100.35, 100.4),
            (1, down(100.35), up(100.4), 100.3, 100.45),
            (5, 100.25, 100.5, 100.25, 100.5),
            (5, 100.45, 100.55, 100.25, 100.75),
            (5, down(100.25), up(100.5), 100.0, 100.75),
            (10, 100.5, 101.0, 100.5, 101.0),
            (10, 100.95, 101.05, 100.5, 101.5),
            (10, down(100.5), up(101.0), 100.0, 101.5),
        ];
        for (n, bid, ask, wb, wa) in cases {
            let b = one(bid, ask);
            let o = t(&b, n, "0.05").unwrap();
            o.validate(&b).unwrap();
            assert_eq!(
                sides(&o),
                (wb, 1.0, wa, 2.0),
                "n {n} bid {bid:?} ask {ask:?}"
            );
            assert_eq!(
                (o.best_bid.to_bits(), o.best_ask.to_bits()),
                (bid.to_bits(), ask.to_bits())
            );
        }
        // switching the selection gives distinct widths on the same book
        let b = book();
        let w: Vec<Vec<f64>> = [1u8, 5, 10]
            .iter()
            .map(|n| {
                t(&b, *n, "0.1")
                    .unwrap()
                    .rows
                    .iter()
                    .map(|r| r.price)
                    .collect()
            })
            .collect();
        assert_eq!(w[0], vec![101.2, 100.9, 100.6, 100.4, 100.1, 99.9, 99.0]);
        assert_eq!(w[1], vec![101.5, 101.0, 100.0, 99.5, 99.0]);
        assert_eq!(w[2], vec![102.0, 101.0, 100.0, 99.0]);
        // tick x1 equals exact for on-grid books
        assert_eq!(
            t(&b, 1, "0.1").unwrap(),
            ladder(&b, 20, 0.0, false).unwrap()
        );
        // quote notional uses the raw level price, not the bucket
        let o = ladder_grouped(
            &one(100.45, 100.55),
            5,
            Grouping::Ticks(5, Some("0.05")),
            true,
        )
        .unwrap();
        assert_eq!(sides(&o), (100.25, 100.45, 100.75, 100.55 * 2.0));
    }

    /// Mode survives source switches: same n recomputed per current book tick.
    #[test]
    fn same_mode_recomputes_per_book_tick_and_missing_is_recoverable() {
        let btc = one(95_000.37, 95_000.53);
        assert_eq!(
            sides(&t(&btc, 10, "0.1").unwrap()),
            (95_000.0, 1.0, 95_001.0, 2.0)
        );
        let eth = one(3_000.37, 3_000.53);
        assert_eq!(
            sides(&t(&eth, 10, "0.01").unwrap()),
            (3_000.3, 1.0, 3_000.6, 2.0)
        );
        let e = ladder_grouped(&eth, 20, Grouping::Ticks(5, None), false).unwrap_err();
        assert_eq!(e.code, codes::INVALID_INPUT);
        assert!(e.message.contains("tick size not provided"));
        assert!(t(&eth, 5, "0.01").is_ok());
    }

    #[test]
    fn params_default_off_and_legacy_settings_unchanged() {
        let d = OrderBook::descriptor();
        let pv = resolve_params(&d, br#"{"rows":20,"group":1.0,"unit":"base"}"#).unwrap();
        let mut p = OrderBook::new(&pv).unwrap();
        assert_eq!(p.ticks, None);
        assert_eq!(
            p.compute(&book()).unwrap(),
            ladder(&book(), 20, 1.0, false).unwrap()
        );
        let mut p = OrderBook::new(&resolve_params(&d, b"{}").unwrap()).unwrap();
        assert_eq!(
            p.compute(&book()).unwrap(),
            ladder(&book(), 20, 0.0, false).unwrap()
        );
        let mut p = OrderBook::new(&resolve_params(&d, br#"{"ticks":"5"}"#).unwrap()).unwrap();
        assert_eq!(p.compute(&book()).unwrap_err().code, codes::INVALID_INPUT);
        // B-1 regression: a stored manual group never blocks a tick
        // selection; ticks take precedence, missing tick never falls back
        // to manual, and `off` restores the untouched manual width.
        let mut ticked = book();
        ticked.tick_size = Some("0.1".into());
        let mut p =
            OrderBook::new(&resolve_params(&d, br#"{"ticks":"5","group":1.0}"#).unwrap()).unwrap();
        assert_eq!(p.group, 1.0, "manual preference not mutated");
        assert_eq!(
            p.compute(&ticked).unwrap(),
            ladder_grouped(&ticked, 20, Grouping::Ticks(5, Some("0.1")), false).unwrap()
        );
        assert_ne!(
            p.compute(&ticked).unwrap(),
            ladder(&ticked, 20, 1.0, false).unwrap()
        );
        assert_eq!(p.compute(&book()).unwrap_err().code, codes::INVALID_INPUT);
        let mut p = OrderBook::new(&resolve_params(&d, br#"{"ticks":"off","group":1.0}"#).unwrap())
            .unwrap();
        assert_eq!(
            p.compute(&ticked).unwrap(),
            ladder(&ticked, 20, 1.0, false).unwrap()
        );
        assert_eq!(
            p.compute(&book()).unwrap(),
            ladder(&book(), 20, 1.0, false).unwrap()
        );
        let mut p = OrderBook::new(&resolve_params(&d, br#"{"ticks":"off","group":0.0}"#).unwrap())
            .unwrap();
        assert_eq!(
            p.compute(&ticked).unwrap(),
            ladder(&ticked, 20, 0.0, false).unwrap()
        );
        assert!(resolve_params(&d, br#"{"ticks":"2"}"#).is_err());
    }

    /// Instance path: the same `ticks` selection recomputes from whatever
    /// tick the current book carries (BTC 0.1 -> ETH 0.01 -> absent -> back).
    #[test]
    fn compute_uses_current_book_tick_and_descriptor_requirements() {
        let d = OrderBook::descriptor();
        d.validate().unwrap();
        for (raw, want) in [
            (&br#"{}"#[..], vec![]),
            (br#"{"group":1.0}"#, vec![]),
            (br#"{"ticks":"off"}"#, vec![]),
            (br#"{"ticks":"1"}"#, vec![BookMetaField::TickSize]),
            (br#"{"ticks":"10"}"#, vec![BookMetaField::TickSize]),
        ] {
            assert_eq!(d.book_requirements(&resolve_params(&d, raw).unwrap()), want);
        }
        let mut p = OrderBook::new(&resolve_params(&d, br#"{"ticks":"10"}"#).unwrap()).unwrap();
        let with = |bid, ask, t: Option<&str>| BookSnapshot {
            tick_size: t.map(Into::into),
            ..one(bid, ask)
        };
        let b = with(95_000.37, 95_000.53, Some("0.1"));
        assert_eq!(
            sides(&p.compute(&b).unwrap()),
            (95_000.0, 1.0, 95_001.0, 2.0)
        );
        let e = with(3_000.37, 3_000.53, Some("0.01"));
        assert_eq!(sides(&p.compute(&e).unwrap()), (3_000.3, 1.0, 3_000.6, 2.0));
        let gone = with(3_000.37, 3_000.53, None);
        assert_eq!(p.compute(&gone).unwrap_err().code, codes::INVALID_INPUT);
        assert_eq!(
            sides(&p.compute(&b).unwrap()),
            (95_000.0, 1.0, 95_001.0, 2.0)
        );
    }
}
