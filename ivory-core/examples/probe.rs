use ivory_core::ChordDetector;
use std::collections::HashSet;

fn main() {
    let cases: &[&[u8]] = &[
        &[64, 70, 74, 78],   // v060 E Bb D F# -> want C7(#11)
        &[57, 63, 67],       // x-D9 A Eb G -> want F9
        &[52, 55, 58, 62, 72],// x-D10 E G Bb D C -> want C9/E
        &[60, 62, 64, 67],   // v108 C D E G -> want C(add9)
        &[64, 69, 70, 74],   // v120 E A Bb D -> want Em7b5(11)
        &[60, 65, 66, 70],   // v121 C F Gb Bb -> want Cm7b5(11)
        &[60, 62, 64, 66, 68],// v136 C D E F# G# -> want D7(#11)
    ];
    for c in cases {
        let mut d = ChordDetector::new();
        let (best, cands) = d.detect_chord_debug(&c.iter().copied().collect::<HashSet<u8>>(), 6);
        println!("{:?} -> {:?}", c, best);
        for (n, s) in cands { println!("      {:>7.1}  {}", s, n); }
    }
}
