//! Why did the fretboard choose THAT shape?
//!
//! The voicing solver's answer to `examples/probe.rs`: it dumps the winner and
//! the runners-up with their costs, so a shape that looks wrong can be argued
//! with in points rather than in adjectives. Edit `CASES` to whatever you are
//! staring at.
//!
//!     cargo run -p ivory-core --example voiceprobe --release
//!     cargo run -p ivory-core --example voiceprobe --release -- 60 64 67
//!     cargo run -p ivory-core --example voiceprobe --release -- --capo 3 43 50 55
//!     cargo run -p ivory-core --example voiceprobe --release -- --tuning DADGAD 57 58

use ivory_core::fretboard::{FretboardSpec, Tuning};
use ivory_core::voicing::{solve_debug, History, StringState, Voicing, Weights};

const CASES: &[(&[u8], &str)] = &[
    (&[40, 47, 52, 56, 59, 64], "open E"),
    (&[41, 48, 53, 57, 60, 65], "F barre"),
    (&[57, 60, 64, 67], "Am7 close — 95 points from the 12th-fret alternative"),
    (&[64, 65], "E4+F4 — 11 points apart, a knife edge by construction"),
    (&[62, 65, 69, 72, 76], "rootless Dm9 — the five-finger call"),
    (&[36, 60, 64, 67], "low bass under a triad — the ghost call"),
    (&[36, 43, 48, 52, 55, 58, 60, 64, 67, 72], "ten notes, six strings"),
];

fn render(v: &Voicing) -> String {
    v.strings
        .iter()
        .map(|s| match s {
            StringState::Sounding { fret, folded: true, .. } => format!("({fret})"),
            StringState::Sounding { fret, .. } => format!("{fret}"),
            StringState::Skipped => "x".to_owned(),
            StringState::Unused => "-".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn report(spec: &FretboardSpec, held: &[u8], label: &str, w: &Weights) {
    let (best, stats, runners) = solve_debug(spec, held, &History::NONE, w);
    println!("\n{held:?}  {label}");
    println!(
        "  {:>8}  {:<24}  anchor {:?} span {} fingers {} {:?}{}",
        best.cost,
        render(&best),
        best.shape.anchor,
        best.shape.span,
        best.shape.fingers,
        best.shape.playability,
        best.shape.barre.map_or(String::new(), |b| format!(
            "  barre {}@{}..={}",
            b.fret, b.lo_string, b.hi_string
        )),
    );
    if let Some(c) = best.caption() {
        println!("  {:>8}  {c}", "");
    }
    for n in &best.notes {
        if !matches!(n.outcome, ivory_core::voicing::Outcome::Placed { .. }) {
            println!("  {:>8}  {} -> {:?}", "", n.pitch, n.outcome);
        }
    }
    for (cost, v) in &runners {
        println!("  {cost:>8}  {:<24}  (+{})", render(v), cost - best.cost);
    }
    println!(
        "  {:>8}  {} leaves, {} pruned{}",
        "",
        stats.leaves,
        stats.pruned,
        if stats.truncated { ", TRUNCATED" } else { "" }
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut capo = 0u8;
    let mut tuning = "Standard".to_owned();
    let mut held: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--capo" => {
                i += 1;
                capo = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--tuning" => {
                i += 1;
                tuning = args.get(i).cloned().unwrap_or_default();
            }
            other => {
                if let Ok(n) = other.parse::<u8>() {
                    held.push(n);
                }
            }
        }
        i += 1;
    }
    let Some(t) = Tuning::by_name(&tuning) else {
        eprintln!("no tuning named {tuning}");
        std::process::exit(2);
    };
    let spec = FretboardSpec { tuning: t.clone(), frets: 22, capo };
    let w = Weights::for_tuning(t);
    println!("{} frets 22 capo {capo}   (n) = folded, x = skipped, - = unused", t.name);

    if held.is_empty() {
        for (notes, label) in CASES {
            report(&spec, notes, label, &w);
        }
    } else {
        held.sort_unstable();
        held.dedup();
        report(&spec, &held, "", &w);
    }
}
