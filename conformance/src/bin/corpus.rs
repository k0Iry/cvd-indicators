//! Writes the shared conformance corpus with expected outcomes computed by the
//! REAL wasm artifacts (`CVD_WASM_DIR`). Usage: corpus <out.json>
fn main() {
    let out = std::env::args().nth(1).expect("usage: corpus <out.json>");
    let v = cvd_indicators_conformance::corpus_json();
    std::fs::write(&out, serde_json::to_vec(&v).unwrap()).expect("write corpus");
    eprintln!("wrote {out}");
}
