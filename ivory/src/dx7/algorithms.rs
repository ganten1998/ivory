//! The thirty-two algorithms: which operator feeds which.
//!
//! # How to read this table
//!
//! `DEST[a][i]` is where operator `i + 1` sends its output under algorithm
//! `a + 1`. `OUT` means it is a carrier and goes to the speaker; anything else
//! is the operator number it modulates.
//!
//! Operators are always evaluated from 6 down to 1, which is the order the
//! chart draws them in and the order that guarantees a modulator is computed
//! before whatever it modulates. Every algorithm in the DX7 is a forest with
//! edges pointing downward in operator number, with one exception per
//! algorithm: the feedback operator, which reads its own previous output.
//!
//! # This table is the one thing here that is a transcription
//!
//! Everything else in `dx7/` is derived from the format or from the arithmetic.
//! This is copied from the chart on the front of the instrument, and a wrong
//! entry does not fail: it plays, and the patch sounds like a different patch.
//! `verify_by_ear` in the tests is what it is called for a reason.
//!
//! The invariants that CAN be checked are, in `tests` below: every algorithm
//! has at least one carrier, no operator modulates itself except through the
//! feedback path, and every destination is a lower-numbered operator.

/// A carrier: this operator's output is heard.
pub const OUT: u8 = 0;

/// Where each operator sends its output, per algorithm.
pub const DEST: [[u8; 6]; 32] = [
    // 1: 6→5→4→3 and 2→1. Two carriers.
    [OUT, 1, OUT, 3, 4, 5],
    // 2: as 1, feedback moves to operator 2.
    [OUT, 1, OUT, 3, 4, 5],
    // 3: 6→5→4 and 3→2→1.
    [OUT, 1, 2, OUT, 4, 5],
    // 4: as 3, with the feedback loop around 4-5-6.
    [OUT, 1, 2, OUT, 4, 5],
    // 5: three pairs, 6→5, 4→3, 2→1.
    [OUT, 1, OUT, 3, OUT, 5],
    // 6: as 5, with the feedback loop around 5-6.
    [OUT, 1, OUT, 3, OUT, 5],
    // 7: 2→1, and 3 taken by 4 and by 6→5.
    [OUT, 1, OUT, 3, 3, 5],
    // 8: as 7, feedback on 4.
    [OUT, 1, OUT, 3, 3, 5],
    // 9: as 7, feedback on 2.
    [OUT, 1, OUT, 3, 3, 5],
    // 10: 3→2→1, and 4 taken by 5 and 6.
    [OUT, 1, 2, OUT, 4, 4],
    // 11: as 10, feedback on 6.
    [OUT, 1, 2, OUT, 4, 4],
    // 12: 2→1, and 3 taken by 4, 5 and 6.
    [OUT, 1, OUT, 3, 3, 3],
    // 13: as 12, feedback on 6.
    [OUT, 1, OUT, 3, 3, 3],
    // 14: 2→1, and 3 taken by 4, with 4 taken by 5 and 6.
    [OUT, 1, OUT, 3, 4, 4],
    // 15: as 14, feedback on 2.
    [OUT, 1, OUT, 3, 4, 4],
    // 16: one carrier. 1 taken by 2, by 3→4, and by 5→6.
    [OUT, 1, 1, 3, 1, 5],
    // 17: as 16, feedback on 2.
    [OUT, 1, 1, 3, 1, 5],
    // 18: one carrier. 1 taken by 2, by 3, and by 4→5→6.
    [OUT, 1, 1, 1, 4, 5],
    // 19: 6→5 and 6→4 into two carriers, plus 3→2→1.
    [OUT, 1, 2, OUT, OUT, 4],
    // 20: 3→1 and 3→2, with 4 and 5 into 3.
    [OUT, OUT, 1, 3, 3, OUT],
    // 21: two stacks feeding four carriers.
    [OUT, OUT, 1, 3, 3, OUT],
    // 22: 6 into three carriers, 2→1.
    [OUT, 1, OUT, OUT, OUT, 3],
    // 23: 6→5, 4→3, and 2→1 with 1 and 2 both carriers.
    [OUT, OUT, OUT, 3, OUT, 5],
    // 24: 6 into three carriers.
    [OUT, OUT, OUT, OUT, 3, 3],
    // 25: 6 into two carriers, three carriers standing alone.
    [OUT, OUT, OUT, OUT, OUT, 3],
    // 26: 6→5 into 4, and 3→2, 1 alone.
    [OUT, OUT, 2, OUT, 4, 5],
    // 27: as 26, feedback on 3.
    [OUT, OUT, 2, OUT, 4, 5],
    // 28: 5 feeds 4, 3 feeds nothing, 6 feeds 5.
    [OUT, OUT, OUT, 3, 4, OUT],
    // 29: four carriers, 6→5 and 4→3.
    [OUT, OUT, OUT, OUT, OUT, 5],
    // 30: five carriers with one three-deep stack.
    [OUT, OUT, OUT, OUT, 4, OUT],
    // 31: five carriers, 6→5.
    [OUT, OUT, OUT, OUT, OUT, 5],
    // 32: all six are carriers.
    [OUT, OUT, OUT, OUT, OUT, OUT],
];

/// Which operator the feedback loop is taken from, per algorithm, 1-based.
///
/// Always exactly one. In algorithms 4 and 6 the printed loop encloses three
/// and two operators respectively; both are approximated here as a self-loop on
/// the same operator, which is the same amount of feedback entering the same
/// place and differs only in the colour of the result.
pub const FEEDBACK_OP: [u8; 32] = [
    6, 2, 6, 6, 6, 6, 6, 4, 2, 6, // 1-10
    6, 6, 6, 6, 2, 6, 2, 6, 6, 6, // 11-20
    6, 6, 6, 6, 6, 6, 3, 6, 6, 6, // 21-30
    6, 6, // 31-32
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every algorithm has to produce sound, and every edge has to point at an
    /// operator that exists.
    #[test]
    fn every_algorithm_is_wired_to_something_audible() {
        for (a, dest) in DEST.iter().enumerate() {
            let carriers = dest.iter().filter(|d| **d == OUT).count();
            assert!(carriers >= 1, "algorithm {} has no carrier", a + 1);
            assert!(carriers <= 6, "algorithm {} has {carriers} carriers", a + 1);
            for (i, d) in dest.iter().enumerate() {
                assert!(*d <= 6, "algorithm {} operator {} targets {d}", a + 1, i + 1);
                assert_ne!(
                    *d as usize,
                    i + 1,
                    "algorithm {} has operator {} modulating itself outside the \
                     feedback path",
                    a + 1,
                    i + 1
                );
            }
        }
    }

    /// **Modulators come after what they modulate.** Operators are evaluated
    /// from 6 down to 1, so an edge pointing UP the numbering would read a
    /// value that had not been computed yet this sample.
    #[test]
    fn every_edge_points_down_the_operator_numbering() {
        for (a, dest) in DEST.iter().enumerate() {
            for (i, d) in dest.iter().enumerate() {
                if *d != OUT {
                    assert!(
                        (*d as usize) < i + 1,
                        "algorithm {} has operator {} modulating {d}, which is \
                         evaluated later",
                        a + 1,
                        i + 1
                    );
                }
            }
        }
    }

    /// The feedback operator is always one real operator.
    #[test]
    fn the_feedback_operator_exists() {
        assert_eq!(FEEDBACK_OP.len(), 32);
        for (a, op) in FEEDBACK_OP.iter().enumerate() {
            assert!(
                (1..=6).contains(op),
                "algorithm {} takes feedback from operator {op}",
                a + 1
            );
        }
    }

    /// Algorithm 32 is the one everybody can check from memory: six carriers,
    /// no modulation, an additive organ.
    #[test]
    fn algorithm_thirty_two_is_six_carriers() {
        assert!(DEST[31].iter().all(|d| *d == OUT));
    }
}
