//! A patch as a list of numbered rows, and the rows back into a patch.
//!
//! # Why this exists
//!
//! The editor lives in `ivory-ui`, which cannot name a [`Voice`] and has no
//! business knowing what keyboard level scaling is. It draws rows with names
//! and numbers in them and reports which number changed. This file is the
//! translation, on the synth's side of the firewall — the same shape as the
//! effect defaults and the cartridge names.
//!
//! # The order is the interface
//!
//! A row is addressed by its GROUP and its INDEX, so the order below is what
//! the editor reports against. Reordering a group silently rewires every edit
//! made against it. The tests hold the order and the count.
//!
//! # What is exposed
//!
//! All of it: 155 parameters, which is every number a DX7 stores. There is no
//! "advanced" tier and nothing hidden — the point of a patch editor is that the
//! patch is yours, and a control somebody cannot reach is a control they have
//! to leave the app to change.

use super::voice::{Op, Voice};
use ivory_ui::ports::{PatchEdit, PatchGroup, PatchParam};

/// The seven groups: one per operator, then the patch itself.
pub const GROUPS: usize = 7;

/// The group index of the patch-wide parameters.
pub const GLOBAL: usize = 6;

/// Parameters in an operator's group. Every number the format keeps per
/// operator; see the module docs.
pub const OP_PARAMS: usize = 21;

/// Parameters in the global group.
///
/// Nineteen: the algorithm, feedback and key sync; eight pitch-envelope
/// numbers; six for the LFO; pitch mod sensitivity; and the transpose. With
/// six operators of [`OP_PARAMS`] that is 145 rows, and the patch's ten-byte
/// name makes up the format's 155.
pub const GLOBAL_PARAMS: usize = 19;

/// A plain numeric row.
fn num(name: &str, value: u8, max: u8) -> PatchParam {
    PatchParam {
        name: name.to_owned(),
        value: i32::from(value),
        max: i32::from(max),
        choices: Vec::new(),
        unit: String::new(),
    }
}

/// A row whose values have names.
fn choice(name: &str, value: u8, choices: &[&str]) -> PatchParam {
    PatchParam {
        name: name.to_owned(),
        value: i32::from(value),
        max: (choices.len() as i32 - 1).max(0),
        choices: choices.iter().map(|s| (*s).to_owned()).collect(),
        unit: String::new(),
    }
}

/// The four curve shapes, as the instrument's own panel names them.
const CURVES: [&str; 4] = ["-LIN", "-EXP", "+EXP", "+LIN"];

/// The LFO's six waveforms.
const WAVES: [&str; 6] = [
    "triangle",
    "saw down",
    "saw up",
    "square",
    "sine",
    "sample & hold",
];

/// One operator's rows, in the order the editor addresses them.
fn op_rows(op: &Op) -> Vec<PatchParam> {
    let mut v = Vec::with_capacity(OP_PARAMS);
    // The envelope first, because it is what a patch is mostly made of: four
    // rates and the four levels they run to.
    for k in 0..4 {
        v.push(num(&format!("Rate {}", k + 1), op.rate[k], 99));
    }
    for k in 0..4 {
        v.push(num(&format!("Level {}", k + 1), op.level[k], 99));
    }
    // How loud it is, and how much of that the player controls.
    v.push(num("Output level", op.output_level, 99));
    v.push(num("Velocity sens", op.vel_sens, 7));
    // Frequency: a ratio of the note, or a fixed pitch.
    v.push(choice(
        "Mode",
        u8::from(op.fixed),
        &["ratio", "fixed Hz"],
    ));
    v.push(num("Coarse", op.coarse, 31));
    v.push(num("Fine", op.fine, 99));
    // 7 is no detune at all, which is why this is not simply 0..=14.
    v.push(num("Detune", op.detune, 14));
    // Keyboard scaling: where the curves meet and what each side does.
    v.push(num("Break point", op.break_point, 99));
    v.push(num("Left depth", op.left_depth, 99));
    v.push(num("Right depth", op.right_depth, 99));
    v.push(choice("Left curve", op.left_curve, &CURVES));
    v.push(choice("Right curve", op.right_curve, &CURVES));
    v.push(num("Rate scaling", op.rate_scaling, 7));
    v.push(num("Amp mod sens", op.amp_mod_sens, 3));
    debug_assert_eq!(v.len(), OP_PARAMS);
    v
}

/// The patch-wide rows.
fn global_rows(v: &Voice) -> Vec<PatchParam> {
    let mut out = Vec::with_capacity(GLOBAL_PARAMS);
    // Shown as 1..=32, the way every chart prints it. Stored 0-based, so the
    // editor's value is one less than what it displays — handled where it is
    // displayed rather than here, because the ROUTING is indexed by the stored
    // value and an off-by-one in this direction is silent.
    out.push(num("Algorithm", v.algorithm, 31));
    out.push(num("Feedback", v.feedback, 7));
    out.push(choice("Osc key sync", u8::from(v.osc_sync), &["off", "on"]));
    // The pitch envelope: the one envelope that is not an attenuation. Level
    // 50 is the note as written.
    for k in 0..4 {
        out.push(num(&format!("Pitch rate {}", k + 1), v.pitch_rate[k], 99));
    }
    for k in 0..4 {
        out.push(num(&format!("Pitch level {}", k + 1), v.pitch_level[k], 99));
    }
    out.push(num("LFO speed", v.lfo_speed, 99));
    out.push(num("LFO delay", v.lfo_delay, 99));
    out.push(num("LFO pitch depth", v.lfo_pitch_depth, 99));
    out.push(num("LFO amp depth", v.lfo_amp_depth, 99));
    out.push(choice("LFO wave", v.lfo_wave.min(5), &WAVES));
    out.push(choice("LFO key sync", u8::from(v.lfo_sync), &["off", "on"]));
    out.push(num("Pitch mod sens", v.pitch_mod_sens, 7));
    // 24 is no transposition, which is why this is not centred on zero.
    out.push(num("Transpose", v.transpose, 48));
    debug_assert_eq!(out.len(), GLOBAL_PARAMS);
    out
}

/// Everything the editor shows, from a patch.
pub fn to_edit(v: &Voice, bank_path: &str) -> PatchEdit {
    let algorithm = usize::from(v.algorithm).min(31);
    let mut groups = Vec::with_capacity(GROUPS);
    for (i, op) in v.ops.iter().enumerate() {
        groups.push(PatchGroup {
            title: format!("OP{}", i + 1),
            params: op_rows(op),
        });
    }
    groups.push(PatchGroup {
        title: "PATCH".to_owned(),
        params: global_rows(v),
    });
    PatchEdit {
        name: v.display_name(),
        algorithm,
        routing: super::algorithms::DEST[algorithm],
        feedback_op: super::algorithms::FEEDBACK_OP[algorithm],
        groups,
        bank_path: bank_path.to_owned(),
    }
}

/// Apply one edited row back to the patch.
///
/// **Out-of-range addresses are ignored rather than clamped into a neighbour.**
/// A group or index the host does not recognise is a UI built against a
/// different build, and writing it to whatever parameter happens to be at that
/// offset would corrupt a patch silently.
pub fn apply(v: &mut Voice, group: usize, index: usize, value: i32) {
    let set = |dst: &mut u8, max: u8| *dst = value.clamp(0, i32::from(max)) as u8;
    if group < 6 {
        let op = &mut v.ops[group];
        match index {
            0..=3 => set(&mut op.rate[index], 99),
            4..=7 => set(&mut op.level[index - 4], 99),
            8 => set(&mut op.output_level, 99),
            9 => set(&mut op.vel_sens, 7),
            10 => op.fixed = value != 0,
            11 => set(&mut op.coarse, 31),
            12 => set(&mut op.fine, 99),
            13 => set(&mut op.detune, 14),
            14 => set(&mut op.break_point, 99),
            15 => set(&mut op.left_depth, 99),
            16 => set(&mut op.right_depth, 99),
            17 => set(&mut op.left_curve, 3),
            18 => set(&mut op.right_curve, 3),
            19 => set(&mut op.rate_scaling, 7),
            20 => set(&mut op.amp_mod_sens, 3),
            _ => {}
        }
        return;
    }
    if group != GLOBAL {
        return;
    }
    match index {
        0 => set(&mut v.algorithm, 31),
        1 => set(&mut v.feedback, 7),
        2 => v.osc_sync = value != 0,
        3..=6 => set(&mut v.pitch_rate[index - 3], 99),
        7..=10 => set(&mut v.pitch_level[index - 7], 99),
        11 => set(&mut v.lfo_speed, 99),
        12 => set(&mut v.lfo_delay, 99),
        13 => set(&mut v.lfo_pitch_depth, 99),
        14 => set(&mut v.lfo_amp_depth, 99),
        15 => set(&mut v.lfo_wave, 5),
        16 => v.lfo_sync = value != 0,
        17 => set(&mut v.pitch_mod_sens, 7),
        18 => set(&mut v.transpose, 48),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every parameter the format has is a row.** 155 numbers is the whole
    /// patch; anything left out is a control somebody has to leave the app to
    /// change, which is the opposite of what an editor is for.
    #[test]
    fn every_parameter_in_the_format_is_editable() {
        let e = to_edit(&Voice::default(), "");
        assert_eq!(e.groups.len(), GROUPS);
        let rows: usize = e.groups.iter().map(|g| g.params.len()).sum();
        // 6 operators of 21, plus the patch's own 19 — 145 rows for 155
        // parameters, because the name is ten of them and is a text field
        // rather than ten numbered rows.
        assert_eq!(rows, 6 * OP_PARAMS + e.groups[GLOBAL].params.len());
        assert_eq!(rows + 10, 155, "the whole format is not covered");
    }

    /// **Every row round-trips.** A row that reads a value it cannot write is
    /// a control that appears to work and does nothing, and the way to find
    /// them is to move every one of them and read the patch back.
    #[test]
    fn moving_any_row_changes_that_parameter_and_no_other() {
        let base = Voice::default();
        let shown = to_edit(&base, "");
        for (g, group) in shown.groups.iter().enumerate() {
            for (i, param) in group.params.iter().enumerate() {
                // A value the parameter is not already at.
                let want = if param.value == param.max {
                    param.max - 1
                } else {
                    param.max
                };
                if want < 0 {
                    continue;
                }
                let mut v = base;
                apply(&mut v, g, i, want);
                let after = to_edit(&v, "");
                assert_eq!(
                    after.groups[g].params[i].value, want,
                    "{}/{} did not take the value it was given",
                    group.title, param.name
                );
                // And nothing else moved.
                let mut changed = 0;
                for (g2, group2) in after.groups.iter().enumerate() {
                    for (i2, p2) in group2.params.iter().enumerate() {
                        if p2.value != shown.groups[g2].params[i2].value {
                            changed += 1;
                        }
                    }
                }
                assert_eq!(
                    changed, 1,
                    "{}/{} moved {changed} parameters",
                    group.title, param.name
                );
            }
        }
    }

    /// An address this build does not know is ignored, not written to whatever
    /// happens to be at that offset.
    #[test]
    fn an_unknown_row_changes_nothing() {
        let mut v = Voice::default();
        let before = v;
        apply(&mut v, 99, 0, 50);
        apply(&mut v, 0, 999, 50);
        apply(&mut v, GLOBAL, 999, 50);
        assert_eq!(v, before);
    }

    /// A value out of range is clamped rather than masked into a neighbour.
    #[test]
    fn a_value_out_of_range_is_clamped() {
        let mut v = Voice::default();
        apply(&mut v, 0, 0, 10_000);
        assert_eq!(v.ops[0].rate[0], 99);
        apply(&mut v, 0, 0, -50);
        assert_eq!(v.ops[0].rate[0], 0);
        // And the neighbour that shares its packed byte is untouched.
        apply(&mut v, 0, 19, 10_000);
        assert_eq!(v.ops[0].rate_scaling, 7);
        assert_eq!(v.ops[0].detune, Voice::default().ops[0].detune);
    }

    /// The routing the editor draws is the routing the synth plays.
    #[test]
    fn the_diagram_follows_the_algorithm() {
        let mut v = Voice::default();
        v.algorithm = 31;
        let e = to_edit(&v, "");
        assert_eq!(e.algorithm, 31);
        assert_eq!(e.routing, super::super::algorithms::DEST[31]);
        // Algorithm 32 is six carriers, which is the one anybody can check.
        assert!(e.routing.iter().all(|d| *d == 0));
    }
}
