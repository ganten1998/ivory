#!/usr/bin/env python3
"""Generate a golden corpus from the reference Python ChordDetector.

For every generated MIDI note set, records detect_chord() output under both
flat and sharp naming preferences. The Rust rewrite differential-tests
against this file (minus a documented known-bug exception list).
"""
import json
import random
import sys

IVORY_DIR = "/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory"
sys.path.insert(0, IVORY_DIR)

import chord_detector as cd  # noqa: E402
from chord_detector import ChordDetector  # noqa: E402

det_flat = ChordDetector(prefer_flats=True)
det_sharp = ChordDetector(prefer_flats=False)

cases = {}  # key: tuple(sorted notes) -> source tag (first origin wins)


def add(notes, tag):
    notes = sorted(set(n for n in notes if 21 <= n <= 108))
    if not notes:
        return
    key = tuple(notes)
    if key not in cases:
        cases[key] = tag


# --- 1. Every chord pattern x 12 roots x voicing transforms -----------------
for name, intervals in cd.CHORD_PATTERNS.items():
    for pc in range(12):
        root = 48 + pc  # C3-based
        base = [root + i for i in intervals]
        add(base, f"pattern:{name}:closed")
        # inversions (rotate lowest note up an octave), up to 3
        inv = list(base)
        for k in range(min(3, len(base) - 1)):
            inv = sorted(inv[1:] + [inv[0] + 12])
            add(inv, f"pattern:{name}:inv{k+1}")
        # root doubled an octave down
        add([root - 12] + base, f"pattern:{name}:rootdouble")
        # drop-2 (second-highest voice down an octave)
        if len(base) >= 4:
            d2 = sorted(base[:-2] + [base[-2] - 12] + [base[-1]])
            add(d2, f"pattern:{name}:drop2")
        # rootless voicing
        if len(intervals) >= 4 and 0 in intervals:
            add([root + i for i in intervals if i != 0], f"pattern:{name}:rootless")
        # wide spread: alternate voices up an octave
        wide = [n + 12 * (i % 2) for i, n in enumerate(base)]
        add(wide, f"pattern:{name}:spread")

# --- 2. All two-note intervals ---------------------------------------------
for pc in range(12):
    for semis in range(1, 25):
        add([48 + pc, 48 + pc + semis], f"interval:{semis}")

# --- 3. Single notes at several octaves ------------------------------------
for n in range(21, 109, 7):
    add([n], "single")

# --- 4. Scales (one-octave clusters) x 12 roots ----------------------------
for name, intervals in cd.SCALE_PATTERNS.items():
    for pc in range(12):
        root = 55 + (pc if pc < 6 else pc - 12)
        add([root + i for i in intervals], f"scale:{name}")

# --- 5. Edge cases ----------------------------------------------------------
add(list(range(60, 72)), "edge:chromatic12")
add(list(range(21, 109)), "edge:all88")
add([21, 108], "edge:extremes")
add([60, 72, 84, 96], "edge:octaves-only")
add([60, 61], "edge:semitone")
for k in range(2, 12):
    add(list(range(60, 60 + k)), f"edge:cluster{k}")

# --- 6. Random sets (seeded) ------------------------------------------------
rng = random.Random(0xC0FFEE)
for i in range(6000):
    size = rng.randint(2, 10)
    if rng.random() < 0.5:
        lo = rng.randint(36, 72)
        pool = range(lo, min(lo + 16, 108))  # clustered
    else:
        pool = range(30, 96)  # spread
    pool = list(pool)
    if size > len(pool):
        size = len(pool)
    add(rng.sample(pool, size), "random")

# --- Run the reference detector --------------------------------------------
out = []
errors = 0
for key in sorted(cases):
    notes = list(key)
    rec = {"notes": notes, "src": cases[key]}
    try:
        rec["flat"] = det_flat.detect_chord(set(notes))
    except Exception as e:  # reference crashes are findings, not corpus rows
        rec["flat_error"] = repr(e)
        errors += 1
    try:
        rec["sharp"] = det_sharp.detect_chord(set(notes))
    except Exception as e:
        rec["sharp_error"] = repr(e)
        errors += 1
    out.append(rec)

path = sys.argv[1] if len(sys.argv) > 1 else "corpus.json"
with open(path, "w") as f:
    json.dump(out, f, indent=0)

n_named = sum(1 for r in out if r.get("flat"))
print(f"cases: {len(out)}  named(flat): {n_named}  none: {len(out)-n_named}  errors: {errors}")
