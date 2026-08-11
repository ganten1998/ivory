//! Candidate-score probe driven by stdin, so debugging a case needs no rebuild:
//!
//!   echo 'v060 E Bb D F#|64,70,74,78' | cargo run -p ivory-core --example probe2 --release -- -v
//!
//! One `label|midi,midi,...` line per case; `#` comments and blank lines skipped.
//! `-v` also dumps the top-6 candidate scores (`probe.rs` is the same idea with
//! the cases hard-coded in the source).
use ivory_core::detector::ChordDetector;
use std::collections::HashSet;
use std::io::BufRead;

fn main() {
    let verbose = std::env::args().any(|a| a == "-v");
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (label, rest) = match line.split_once('|') {
            Some(x) => x,
            None => continue,
        };
        let midi: Vec<u8> = rest.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let set: HashSet<u8> = midi.iter().copied().collect();
        let mut d = ChordDetector::new();
        if verbose {
            let (result, cands) = d.detect_chord_debug(&set, 6);
            println!("{:32} rs-> {:?}", label, result);
            for (name, score) in cands {
                println!("      {:<18} {:.1}", name, score);
            }
        } else {
            let result = d.detect_chord(&set);
            println!("{:32} rs-> {:?}", label.trim(), result);
        }
    }
}
