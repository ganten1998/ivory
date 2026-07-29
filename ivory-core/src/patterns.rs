/// All static data tables ported from chord_detector.py

pub static NOTE_NAMES: &[&str] = &["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];
pub static NOTE_NAMES_FLAT: &[&str] = &["C","Db","D","Eb","E","F","Gb","G","Ab","A","Bb","B"];

/// Interval names indexed by semitones (0-21).
pub static INTERVAL_NAMES: &[&str] = &[
    "P1","m2","M2","m3","M3","P4","d5","P5","m6","M6","m7","M7",
    "P8","m9","M9","m10","M10","P11","A11","P12","m13","M13",
];

/// Chord patterns: (internal_name, &[intervals from root]).
/// ORDER MATTERS — iteration order controls detection priority.
pub static CHORD_PATTERNS: &[(&str, &[u8])] = &[
    // ── Triads ──────────────────────────────────────────────────────────────
    ("major",      &[0, 4, 7]),
    ("minor",      &[0, 3, 7]),
    ("diminished", &[0, 3, 6]),
    ("augmented",  &[0, 4, 8]),
    ("sus2",       &[0, 2, 7]),
    ("sus4",       &[0, 5, 7]),

    // ── Sus extended ────────────────────────────────────────────────────────
    ("7sus4",      &[0, 5, 7, 10]),
    ("7sus2",      &[0, 2, 7, 10]),
    ("9sus",       &[0, 2, 5, 10]),
    ("9sus_with5", &[0, 2, 5, 7, 10]),
    ("13sus",      &[0, 2, 5, 9, 10]),
    ("13sus_with5",&[0, 2, 5, 7, 9, 10]),
    ("7sus13",     &[0, 2, 5, 9, 10]),
    ("sus13",      &[0, 2, 5, 9]),

    // ── 7th chords (half-dim before other m7) ───────────────────────────────
    ("half_diminished7",      &[0, 3, 6, 10]),
    ("half_diminished11",     &[0, 3, 5, 6, 10]),
    ("half_diminished11_no3", &[0, 5, 6, 10]),
    ("major7",        &[0, 4, 7, 11]),
    ("major7#5",      &[0, 4, 8, 11]),
    ("minor7",        &[0, 3, 7, 10]),
    ("dominant7",     &[0, 4, 7, 10]),
    ("diminished7",   &[0, 3, 6, 9]),
    ("diminished_major7", &[0, 3, 6, 11]),
    ("7b13_no5",      &[0, 4, 8, 10]),   // BEFORE aug7 for priority
    ("augmented7",    &[0, 4, 8, 10]),
    ("minor_major7",  &[0, 3, 7, 11]),
    ("minor_major9",  &[0, 2, 3, 7, 11]),

    // ── Extended ────────────────────────────────────────────────────────────
    ("major9",        &[0, 2, 4, 7, 11]),
    ("minor9",        &[0, 2, 3, 7, 10]),
    ("dominant9",     &[0, 2, 4, 7, 10]),
    ("major11",       &[0, 2, 4, 5, 7, 11]),
    ("major9#11",     &[0, 2, 4, 6, 7, 11]),
    ("major7#11",     &[0, 4, 6, 7, 11]),
    ("major7#11_no5", &[0, 4, 6, 11]),
    ("major7#11_shell",&[0, 6, 11]),
    ("minor11",       &[0, 2, 3, 5, 7, 10]),
    ("minor11_no5",   &[0, 2, 3, 5, 10]),
    ("minor11_no9",   &[0, 3, 5, 7, 10]),
    ("minor11_shell", &[0, 3, 5, 10]),
    ("major13",       &[0, 2, 4, 5, 7, 9, 11]),
    ("major13#11",    &[0, 2, 4, 6, 7, 9, 11]),
    ("minor13",       &[0, 2, 3, 5, 7, 9, 10]),
    ("dominant11",    &[0, 2, 4, 5, 7, 10]),
    ("dominant13",    &[0, 2, 4, 5, 7, 9, 10]),
    // 13th shells (BEFORE 6th chords)
    ("13_shell",      &[0, 4, 9, 10]),
    ("13_no5_no11",   &[0, 2, 4, 9, 10]),
    ("13_no5",        &[0, 2, 4, 5, 9, 10]),

    // ── Dom 7#11 voicings ────────────────────────────────────────────────────
    ("7#11_no5",      &[0, 4, 6, 10]),
    ("7#11_no3_no5",  &[0, 2, 6, 10]),
    ("13#11_no3_no5", &[0, 2, 6, 9, 10]),
    ("13#11_no9_no5", &[0, 4, 6, 9, 10]),
    ("13#11_no5",     &[0, 2, 4, 6, 9, 10]),

    // ── Altered dominants – with 5th ─────────────────────────────────────────
    ("7b9",       &[0, 1, 4, 7, 10]),
    ("7#9",       &[0, 3, 4, 7, 10]),
    ("7#11",      &[0, 4, 6, 7, 10]),
    ("7b13",      &[0, 4, 7, 8, 10]),
    ("7b9#11",    &[0, 1, 4, 6, 7, 10]),
    ("7#9#11",    &[0, 3, 4, 6, 7, 10]),
    ("7b9b13",    &[0, 1, 4, 7, 8, 10]),
    ("7#9b13",    &[0, 3, 4, 7, 8, 10]),
    ("7#11b13",   &[0, 4, 6, 7, 8, 10]),
    ("7b9#11b13", &[0, 1, 4, 6, 7, 8, 10]),
    ("7#9#11b13", &[0, 3, 4, 6, 7, 8, 10]),
    ("7b9#9",     &[0, 1, 3, 4, 7, 10]),
    ("7b9#9#11",  &[0, 1, 3, 4, 6, 7, 10]),
    ("7b9#9b13",  &[0, 1, 3, 4, 7, 8, 10]),
    ("9b13",      &[0, 2, 4, 7, 8, 10]),
    // Without 5th – shells (AFTER full voicings)
    ("9b13_no5",      &[0, 2, 4, 8, 10]),
    ("7#11_shell",    &[0, 2, 6, 9, 10]),
    ("7#11_no3",      &[0, 6, 7, 10]),
    ("7#9#11_shell",  &[0, 3, 6, 9, 10]),
    ("7b9#11_shell",  &[0, 1, 6, 9, 10]),
    ("7b9#11_no3",    &[0, 1, 6, 7, 10]),
    ("7b9#11_no5",    &[0, 1, 4, 6, 10]),
    ("7b9#11_13_no5", &[0, 1, 4, 6, 9, 10]),
    ("7b9b13_no5",    &[0, 1, 4, 8, 10]),
    ("7#9b13_no5",    &[0, 3, 4, 8, 10]),
    ("7b9#9_no5",     &[0, 1, 3, 4, 10]),
    ("altered",       &[0, 1, 3, 4, 6, 8, 10]),

    // ── Add / 6th chords ─────────────────────────────────────────────────────
    ("add9",       &[0, 2, 4, 7]),
    ("minor_add9", &[0, 2, 3, 7]),
    ("6",          &[0, 4, 7, 9]),
    ("6_no5",      &[0, 4, 9]),
    ("6add4",      &[0, 4, 5, 7, 9]),
    ("6add4_no5",  &[0, 4, 5, 9]),
    ("6_9",        &[0, 2, 4, 7, 9]),
    ("6_9_no5",    &[0, 2, 4, 9]),
    ("6_9_no3",    &[0, 2, 7, 9]),
    ("major7_6_9", &[0, 2, 4, 7, 9, 11]),
    ("minor6",     &[0, 3, 7, 9]),
    ("minor6_no5", &[0, 3, 9]),
    ("minor6_9",   &[0, 2, 3, 7, 9]),
    ("minor6_9_no5",&[0, 2, 3, 9]),
    ("add11",      &[0, 4, 5, 7]),

    // ── Power chord ──────────────────────────────────────────────────────────
    ("5", &[0, 7]),
];

/// Essential intervals for each chord type (must be present for a positive ID).
pub fn essential_for(chord_type: &str) -> &'static [u8] {
    match chord_type {
        "major"      => &[4],
        "minor"      => &[3],
        "diminished" => &[3, 6],
        "augmented"  => &[4, 8],
        "sus2"       => &[2],
        "sus4"       => &[5],
        "5"          => &[7],

        "major7"          => &[4, 11],
        "major7#5"        => &[4, 11],
        "minor7"          => &[3, 10],
        "dominant7"       => &[4, 10],
        "diminished7"     => &[3, 9],
        "diminished_major7" => &[3, 6, 11],
        "half_diminished7"  => &[3, 10],
        "half_diminished11" => &[6, 10],
        "half_diminished11_no3" => &[6, 10],
        "augmented7"      => &[4, 10],
        "minor_major7"    => &[3, 11],
        "minor_major9"    => &[3, 11],

        "major9"   => &[4, 11],
        "minor9"   => &[3, 10],
        "dominant9"=> &[4, 10],
        "major11"  => &[4, 11],
        "major9#11"=> &[4, 6, 11],
        "major7#11"=> &[4, 6, 11],
        "major7#11_no5"  => &[4, 6, 11],
        "major7#11_shell"=> &[6, 11],
        "minor11"        => &[3, 10],
        "minor11_no5"    => &[3, 10],
        "minor11_no9"    => &[3, 10],
        "minor11_shell"  => &[3, 10],
        "major13"        => &[4, 11],
        "major13#11"     => &[4, 11],
        "minor13"        => &[3, 10],
        "dominant11"     => &[4, 10],
        "dominant13"     => &[4, 10],
        "13_shell"       => &[4, 10],
        "13_no5_no11"    => &[4, 10],
        "13_no5"         => &[4, 10],

        "7#11_no5"     => &[4, 10],
        "7#11_no3_no5" => &[6, 10],
        "13#11_no3_no5"=> &[6, 10],
        "13#11_no9_no5"=> &[4, 10],
        "13#11_no5"    => &[4, 10],

        "7sus4"      => &[5, 10],
        "7sus2"      => &[2, 10],
        "9sus"       => &[2, 5, 10],
        "9sus_with5" => &[2, 5, 10],
        "13sus"      => &[2, 10],
        "13sus_with5"=> &[2, 10],
        "7sus13"     => &[2, 10],
        "sus13"      => &[2, 9],

        "7b9"  => &[4, 10],
        "7#9"  => &[4, 10],
        "7#11" => &[4, 10],
        "7b13" => &[4, 10],
        "7b9#11"    => &[4, 6, 10],
        "7#9#11"    => &[3, 4, 6, 10],
        "7b9b13"    => &[4, 10],
        "7#9b13"    => &[4, 10],
        "7#11b13"   => &[4, 10],
        "7b9#11b13" => &[4, 10],
        "7#9#11b13" => &[4, 10],
        "7b9#9"     => &[4, 10],
        "7b9#9#11"  => &[4, 10],
        "7b9#9b13"  => &[4, 10],
        "9b13"      => &[4, 10],
        "9b13_no5"  => &[4, 10],
        "7#11_shell"     => &[6, 10],
        "7#11_no3"       => &[6, 10],
        "7#9#11_shell"   => &[3, 6, 10],
        "7b9#11_shell"   => &[1, 6, 10],
        "7b9#11_no3"     => &[1, 6, 10],
        "7b9#11_no5"     => &[1, 4, 6, 10],
        "7b9#11_13_no5"  => &[1, 4, 6, 10],
        "7b13_no5"       => &[4, 10],
        "7b9b13_no5"     => &[4, 10],
        "7#9b13_no5"     => &[4, 10],
        "7b9#9_no5"      => &[4, 10],
        "altered"        => &[4, 10],

        "6"          => &[4, 9],
        "6_no5"      => &[4, 9],
        "6add4"      => &[4, 9],
        "6add4_no5"  => &[4, 9],
        "6_9"        => &[4, 9],
        "6_9_no5"    => &[4, 9],
        "6_9_no3"    => &[2, 9],
        "major7_6_9" => &[4, 9, 11],
        "minor6"     => &[3, 9],
        "minor6_no5" => &[3, 9],
        "minor6_9"   => &[3, 9],
        "minor6_9_no5" => &[3, 9],

        "add9"       => &[4],
        "minor_add9" => &[3],
        "add11"      => &[4],

        _ => &[],
    }
}

/// Optional intervals for each chord type (omitting them is acceptable in jazz).
pub fn optional_for(chord_type: &str) -> &'static [u8] {
    match chord_type {
        "major"      => &[0, 7],
        "minor"      => &[0, 7],
        "diminished" => &[0],
        "augmented"  => &[0],
        "sus2"       => &[0, 7],
        "sus4"       => &[0, 7],
        "5"          => &[0],

        "major7"          => &[0, 7],
        "major7#5"        => &[0],
        "minor7"          => &[0, 7],
        "dominant7"       => &[0, 7],
        "diminished7"     => &[0],
        "diminished_major7" => &[0],
        "half_diminished7"  => &[0],
        "augmented7"        => &[0],
        "minor_major7"      => &[0, 7],
        "minor_major9"      => &[0, 2, 7],

        "major9"      => &[0, 7],
        "minor9"      => &[0, 7],
        "dominant9"   => &[0, 7],
        "major11"     => &[0, 7],
        "major7#11"   => &[0, 2, 7],
        "major7#11_no5"  => &[0, 2],
        "major7#11_shell"=> &[0, 2, 4, 7],
        "major9#11"   => &[0, 7],
        "minor11"     => &[0, 7],
        "minor11_no5" => &[0],
        "minor11_shell"   => &[0, 2],
        "major13"     => &[0, 5, 7],
        "major13#11"  => &[0, 7],
        "minor13"     => &[0, 7],
        "dominant11"  => &[0, 7],
        "dominant13"  => &[0, 5, 7],
        "13_shell"    => &[0, 7],
        "13_no5_no11" => &[0, 7],
        "13_no5"      => &[0, 7],

        "7sus4"      => &[0, 7],
        "7sus2"      => &[0, 7],
        "7sus13"     => &[0, 5, 7],
        "sus13"      => &[0, 5, 7],

        "7b9"  => &[0, 7],
        "7#9"  => &[0, 7],
        "7#11" => &[0, 7],
        "7b13" => &[0, 7],
        "7b9#11"    => &[0, 7],
        "7#9#11"    => &[0, 7],
        "7b9b13"    => &[0, 7],
        "7#9b13"    => &[0, 7],
        "7#11b13"   => &[0, 7],
        "7b9#11b13" => &[0, 7],
        "7#9#11b13" => &[0, 7],
        "7b9#9"     => &[0, 7],
        "7b9#9#11"  => &[0, 7],
        "7b9#9b13"  => &[0, 7],
        "9b13"      => &[0, 7],
        "9b13_no5"  => &[0],
        "7#11_shell"     => &[0, 2, 9],
        "7#11_no3"       => &[0, 2, 9],
        "7#9#11_shell"   => &[0, 6, 9],
        "7b9#11_shell"   => &[0, 6, 9],
        "7b9#11_no3"     => &[0, 6, 9],
        "7b9#11_no5"     => &[0],
        "7b9b13_no5"     => &[0],
        "7#9b13_no5"     => &[0],
        "7b9#9_no5"      => &[0],
        "altered"        => &[0, 7],

        "7#11_no5"     => &[0],
        "7#11_no3_no5" => &[0],
        "13#11_no3_no5"=> &[0],
        "13#11_no9_no5"=> &[0],
        "13#11_no5"    => &[0],

        "6"          => &[0, 7],
        "6_no5"      => &[0],
        "6add4"      => &[0, 7],
        "6add4_no5"  => &[0],
        "6_9"        => &[0, 7],
        "6_9_no5"    => &[0],
        "6_9_no3"    => &[0, 7],
        "minor6"     => &[0, 7],
        "minor6_no5" => &[0],
        "minor6_9"   => &[0, 7],
        "minor6_9_no5" => &[0],

        "add9"       => &[0, 7],
        "minor_add9" => &[0, 7],
        "add11"      => &[0, 7],

        _ => &[],
    }
}

/// Scale patterns: (scale_name, &[intervals from root]).
pub static SCALE_PATTERNS: &[(&str, &[u8])] = &[
    // Major modes
    ("Ionian",      &[0, 2, 4, 5, 7, 9, 11]),
    ("Dorian",      &[0, 2, 3, 5, 7, 9, 10]),
    ("Phrygian",    &[0, 1, 3, 5, 7, 8, 10]),
    ("Lydian",      &[0, 2, 4, 6, 7, 9, 11]),
    ("Mixolydian",  &[0, 2, 4, 5, 7, 9, 10]),
    ("Aeolian",     &[0, 2, 3, 5, 7, 8, 10]),
    ("Locrian",     &[0, 1, 3, 5, 6, 8, 10]),
    // Melodic minor modes
    ("Melodic Minor",     &[0, 2, 3, 5, 7, 9, 11]),
    ("Dorian b2",         &[0, 1, 3, 5, 7, 9, 10]),
    ("Lydian Augmented",  &[0, 2, 4, 6, 8, 9, 11]),
    ("Lydian Dominant",   &[0, 2, 4, 6, 7, 9, 10]),
    ("Mixolydian b6",     &[0, 2, 4, 5, 7, 8, 10]),
    ("Locrian #2",        &[0, 2, 3, 5, 6, 8, 10]),
    ("Altered",           &[0, 1, 3, 4, 6, 8, 10]),
    // Harmonic minor modes
    ("Harmonic Minor",    &[0, 2, 3, 5, 7, 8, 11]),
    ("Locrian #6",        &[0, 1, 3, 5, 6, 9, 10]),
    ("Ionian #5",         &[0, 2, 4, 5, 8, 9, 11]),
    ("Dorian #4",         &[0, 2, 3, 6, 7, 9, 10]),
    ("Phrygian Dominant", &[0, 1, 4, 5, 7, 8, 10]),
    ("Lydian #2",         &[0, 3, 4, 6, 7, 9, 11]),
    ("Altered Diminished",&[0, 1, 3, 4, 6, 8, 9]),
    // Pentatonic
    ("Major Pentatonic",  &[0, 2, 4, 7, 9]),
    ("Minor Pentatonic",  &[0, 3, 5, 7, 10]),
    // Blues
    ("Major Blues",  &[0, 2, 3, 4, 7, 9]),
    ("Minor Blues",  &[0, 3, 5, 6, 7, 10]),
    // Symmetrical
    ("Whole Tone",          &[0, 2, 4, 6, 8, 10]),
    ("Whole-Half Diminished",&[0, 2, 3, 5, 6, 8, 9, 11]),
    ("Half-Whole Diminished",&[0, 1, 3, 4, 6, 7, 9, 10]),
];

pub static MAJOR_MODES: &[&str] = &[
    "Ionian","Dorian","Phrygian","Lydian","Mixolydian","Aeolian","Locrian",
];
pub static MELODIC_MINOR_MODES: &[&str] = &[
    "Melodic Minor","Dorian b2","Lydian Augmented","Lydian Dominant",
    "Mixolydian b6","Locrian #2","Altered",
];
pub static HARMONIC_MINOR_MODES: &[&str] = &[
    "Harmonic Minor","Locrian #6","Ionian #5","Dorian #4",
    "Phrygian Dominant","Lydian #2","Altered Diminished",
];
pub static CLUSTERED_ONLY_SCALES: &[&str] = &[
    "Major Pentatonic","Minor Pentatonic","Major Blues","Minor Blues","Whole Tone",
];
