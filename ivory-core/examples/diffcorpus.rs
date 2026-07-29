//! Differential runner: engine vs the Python golden corpus.
//!
//!   cargo run -p ivory-core --example diffcorpus --release            # summary
//!   cargo run -p ivory-core --example diffcorpus --release -- --dump  # every mismatch
//!   cargo run -p ivory-core --example diffcorpus --release -- --json OUT.json  # rust output
//!
//! Corpus rows: {"notes":[..],"src":"..","flat":<str|null>,"sharp":<str|null>}.
use ivory_core::ChordDetector;
use std::collections::HashSet;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dump = args.iter().any(|a| a == "--dump");
    let json_out = args.iter().position(|a| a == "--json").map(|i| args[i + 1].clone());

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/golden/corpus.json");
    let raw = std::fs::read_to_string(path).expect("read corpus.json");
    let rows: serde_json::Value = serde_json::from_str(&raw).expect("parse corpus.json");
    let rows = rows.as_array().expect("corpus is array");

    let mut mism_flat = 0usize;
    let mut mism_sharp = 0usize;
    let mut both = 0usize;
    let mut dumped = 0usize;
    let mut out = Vec::new();

    for row in rows {
        let notes: Vec<u8> = row["notes"].as_array().unwrap().iter()
            .map(|n| n.as_u64().unwrap() as u8).collect();
        let set: HashSet<u8> = notes.iter().copied().collect();
        let py_flat = row["flat"].as_str();
        let py_sharp = row["sharp"].as_str();

        let mut df = ChordDetector::new(); df.set_note_preference(true);
        let rust_flat = df.detect_chord(&set);
        let mut ds = ChordDetector::new(); ds.set_note_preference(false);
        let rust_sharp = ds.detect_chord(&set);

        let f_mismatch = rust_flat.as_deref() != py_flat;
        let s_mismatch = rust_sharp.as_deref() != py_sharp;
        if f_mismatch { mism_flat += 1; }
        if s_mismatch { mism_sharp += 1; }
        if f_mismatch || s_mismatch { both += 1; }

        if json_out.is_some() {
            out.push(serde_json::json!({
                "notes": notes, "flat": rust_flat, "sharp": rust_sharp,
            }));
        }
        if dump && f_mismatch && dumped < 4000 {
            println!("{:?} [{}]  py={:?}  rust={:?}",
                notes, row["src"].as_str().unwrap_or(""), py_flat, rust_flat);
            dumped += 1;
        }
    }

    if let Some(p) = json_out {
        std::fs::write(&p, serde_json::to_string(&out).unwrap()).unwrap();
        eprintln!("wrote {p}");
    }
    eprintln!("rows={}  flat_mismatch={}  sharp_mismatch={}  any_mismatch={}",
        rows.len(), mism_flat, mism_sharp, both);
}
