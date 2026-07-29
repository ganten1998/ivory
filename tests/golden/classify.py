#!/usr/bin/env python3
"""Differential classifier: map every Ivory engine / Python golden-corpus
divergence to a DIVERGENCES.md rule id (or flag it for hand audit).

Inputs (same directory):
  corpus.json       - Python golden corpus {notes,src,flat,sharp}
  rust-golden.json  - current engine output {notes,flat,sharp}

Outputs (same directory):
  classified-divergences.json - one record per mismatching row:
      {notes, src, python_flat, rust_flat, python_sharp, rust_sharp, rule}
  (also prints per-rule counts and the residual UNCLASSIFIED list)

Every rule is pitch-class-relational, so a single classification covers both
the flat and sharp renderings of a row. See docs/DIVERGENCES.md for rule text.
"""
import json
import os
import re
import collections

HERE = os.path.dirname(os.path.abspath(__file__))

PC = {'C': 0, 'C#': 1, 'Db': 1, 'D': 2, 'D#': 3, 'Eb': 3, 'E': 4, 'F': 5,
      'F#': 6, 'Gb': 6, 'G': 7, 'G#': 8, 'Ab': 8, 'A': 9, 'A#': 10, 'Bb': 10, 'B': 11}
NOTE = re.compile(r'^([A-G][b#]?)(.*)$')

# quality string -> intervals relative to the named root
Q = {
    '': [0, 4, 7], 'm': [0, 3, 7], 'dim': [0, 3, 6], 'aug': [0, 4, 8],
    '2': [0, 2, 7], '4': [0, 5, 7], '5': [0, 7],
    '6': [0, 4, 7, 9], 'm6': [0, 3, 7, 9], '7': [0, 4, 7, 10], 'm7': [0, 3, 7, 10],
    'm7b5': [0, 3, 6, 10], 'dim7': [0, 3, 6, 9], 'dimΔ7': [0, 3, 6, 11],
    'Δ7': [0, 4, 7, 11], 'Δ7#5': [0, 4, 8, 11], 'mΔ7': [0, 3, 7, 11],
    'mΔ7(9)': [0, 2, 3, 7, 11], '9': [0, 4, 7, 10, 2], 'Δ9': [0, 4, 7, 11, 2],
    'm9': [0, 3, 7, 10, 2], '11': [0, 4, 7, 10, 2, 5], 'Δ11': [0, 4, 7, 11, 2, 5],
    'm11': [0, 3, 7, 10, 2, 5], '13': [0, 4, 7, 10, 2, 5, 9], 'Δ13': [0, 4, 7, 11, 2, 5, 9],
    'm13': [0, 3, 7, 10, 2, 5, 9], 'Δ13#11': [0, 4, 7, 11, 2, 6, 9],
    'Δ9(#11)': [0, 4, 7, 11, 2, 6], 'Δ7(#11)': [0, 4, 7, 11, 6],
    '7(#11)': [0, 4, 7, 10, 6], '7(b9)': [0, 4, 7, 10, 1], '7b9': [0, 4, 7, 10, 1],
    '7(#9)': [0, 4, 7, 10, 3], '7(b13)': [0, 4, 7, 10, 8],
    '7(#9,#11)': [0, 4, 7, 10, 3, 6], '7(b9,#11)': [0, 4, 7, 10, 1, 6],
    '7(#11,b13)': [0, 4, 7, 10, 6, 8], '7(b9,b13)': [0, 4, 7, 10, 1, 8],
    '7(#9,b13)': [0, 4, 7, 10, 3, 8], '7(b9,#9)': [0, 4, 7, 10, 1, 3],
    '9(b13)': [0, 4, 7, 10, 2, 8], '13(#11)': [0, 4, 7, 10, 2, 6, 9],
    '13(sus)': [0, 2, 5, 9, 10], '9(sus)': [0, 2, 5, 7, 10], 'sus13': [0, 2, 5, 9],
    '7sus4': [0, 5, 7, 10], '7sus2': [0, 2, 7, 10], '(add9)': [0, 4, 7, 2],
    'm(add9)': [0, 3, 7, 2], 'add11': [0, 4, 7, 5], '6add4': [0, 4, 5, 7, 9],
    'm7b5(11)': [0, 3, 6, 10, 5], '6/9': [0, 4, 7, 9, 2], 'm6/9': [0, 3, 7, 9, 2],
    'maj7(6/9)': [0, 4, 7, 9, 11, 2],
}

# exact interval-label tokens the 2-note formatter can emit  ("C (M3)" etc.)
_INTERVAL_TOK = re.compile(
    r'^[A-G][b#]? \((?:[Mmd]?\d{1,2}|P\d{1,2}|d5|A\d{1,2})\)$')
_SCALE_WORDS = ('Scale', 'Pentatonic', 'Blues', 'Dorian', 'Phrygian', 'Lydian',
                'Mixolydian', 'Aeolian', 'Locrian', 'Ionian', 'Whole', 'Melodic',
                'Harmonic', 'Altered', 'Diminished', 'Chromatic')


def is_interval(name):
    return name is not None and ('semitones' in name or bool(_INTERVAL_TOK.match(name)))


def is_scale(name):
    if name is None:
        return False
    return any(w in name for w in _SCALE_WORDS)


def is_nonchord(name):
    return name is None or is_interval(name) or is_scale(name)


def parse(name):
    """(rootpc, bass_pc_or_None, quality, pcset) or None if not a nameable chord."""
    if is_nonchord(name):
        return None
    s = name
    bass = None
    tmp = s.replace('6/9', '\x016\x019').replace('(6/9)', '(\x016\x019)')
    if '/' in tmp:
        base, b = tmp.rsplit('/', 1)
        base = base.replace('\x016\x019', '6/9')
        b = b.replace('\x016\x019', '6/9')
        m = NOTE.match(b)
        if m and m.group(2) == '':
            bass = PC.get(m.group(1))
        s = base
    else:
        s = tmp.replace('\x016\x019', '6/9')
    m = NOTE.match(s)
    if not m:
        return None
    root = PC.get(m.group(1))
    q = m.group(2)
    if root is None or q not in Q:
        return None
    pcs = set((root + i) % 12 for i in Q[q])
    if bass is not None:
        pcs.add(bass)
    return (root, bass, q, pcs)


def fit(name, notes):
    p = parse(name)
    if p is None:
        return None
    npcs = set(n % 12 for n in notes)
    ch = p[3]
    # error = chord tones not sounded (missing) + sounded tones not named (extra)
    return len(ch - npcs) + len(npcs - ch)


def root_of(name):
    if name is None:
        return None
    m = NOTE.match(name)
    return PC.get(m.group(1)) if m else None


def qual_of(name):
    p = parse(name)
    return p[2] if p else None


DOM_ALTS = {'7(#11)', '7(b9)', '7(#9)', '7(b13)', '7(#9,#11)', '7(b9,#11)',
            '7(#11,b13)', '7(b9,b13)', '7(#9,b13)', '7(b9,#9)', '9(b13)', '13(#11)'}
TERTIAN = {'13', 'Δ13', 'm13', 'Δ13#11', 'Δ9(#11)', 'Δ7(#11)',
           'Δ9', 'Δ11', '11', 'm11', 'm9', '9', 'Δ7', 'm7', 'mΔ7(9)'}
SUS69 = {'7sus4', '7sus2', '9(sus)', '13(sus)', 'sus13', '6/9', 'm6/9'}


def classify(c, r):
    """Return a DIVERGENCES rule id for the mismatch row (c=python, r=rust)."""
    n = c['notes']
    pcs = set(x % 12 for x in n)
    upc = len(pcs)
    bass = min(n) % 12
    pf, rf = c['flat'], r['flat']
    pr, rr = root_of(pf), root_of(rf)
    pq, rq = qual_of(pf), qual_of(rf)
    perr, rerr = fit(pf, n), fit(rf, n)

    # ---- 1. >=8 unique PCs never name a chord ----------------------------
    if upc >= 8:
        if rf is None:
            return 'D17-none'
        if is_scale(rf):
            return 'D17-scale'
        return 'D17-other'

    # ---- 2. scale-vs-chord span (K6/K8/D15) ------------------------------
    if is_scale(rf) and not is_scale(pf):
        return 'K6-scale-span'
    if is_scale(pf) and not is_scale(rf):
        return 'K6-scale-span'
    if is_interval(rf) or is_interval(pf):
        return 'K1-interval'

    # both are nameable chords beyond here (unless one is None)
    if rf is None or pf is None:
        return 'edge-none'

    # ---- 3. D6: bare 7b9 -> 7(b9) parens ---------------------------------
    if pq == '7b9' and rq == '7(b9)' and pr == rr:
        return 'D6-b9-parens'

    # ---- 4. D5: dim/dim7 slash reading -> root-position 7(b9) ------------
    if pf and 'dim' in pf and '/' in pf and rq == '7(b9)':
        return 'D5-dim-slash-7b9'

    # ---- 5. D12: diminished triad -> minor6(no5) (bare or slashed) -------
    if pq == 'dim' and rq == 'm6' and parse(pf)[3] == parse(rf)[3]:
        return 'D12-dim-m6'
    if pf and rf and 'dim' in pf and rq in ('m6', 'm6/9') and \
            root_of(pf.split('/')[0]) == (rr + 9) % 12:
        return 'D12-dim-m6'

    # ---- 6. D9: rootless dominant -> implied real root -------------------
    #   rust names a dominant whose root is NOT sounded but the tritone is.
    if rr is not None and rr not in pcs and rq in (DOM_ALTS | {'9', '13'}):
        if (rr + 4) % 12 in pcs and (rr + 10) % 12 in pcs:
            return 'D9-rootless-dom'

    # ---- 7. D1: closed complete m7 named from bass, not relative 6 -------
    if pq == '6' and rq in ('m7', 'm6', 'm9', 'm13', 'm11') and rr == bass \
            and pr != bass:
        return 'D1-m7-not-rel6'

    # ---- 8. D20: m11 reading loses to bass-coherent sus/6-9/7sus ----------
    if pf and 'm11' in pf and rq in SUS69:
        return 'D20-m11-vs-sus69'
    if pq == 'm11' and rq == 'm11' and rr == bass and pr != bass:
        return 'D20-m11-bass'

    # ---- 9. D19: dead inversion names / dominant9 root-position ----------
    #   python drops the 9/13 with an X7/bass slash; rust keeps the full
    #   9-chord (root position or bass slash).
    if pq in ('7', 'Δ7', '6') and rq in ('9', 'Δ9', '13', 'Δ13',
                                              '6/9', 'm9') and rr == pr:
        return 'D19-inversion-9th'
    if pq in ('7', 'Δ7') and rq in ('9', 'Δ9') and \
            parse(pf)[3] <= pcs and rr == pr:
        return 'D19-inversion-9th'

    # ---- 10. D10 / dominant9 inversion: X7(#11)-style -> Y9/bass ---------
    if rq in ('9', 'Δ9') and rerr is not None and rerr == 0 and \
            (perr is None or perr > 0):
        return 'D10-dom9-inversion'

    # ---- 11. D8: 13sus with b7 present -----------------------------------
    if rq in ('13(sus)', '9(sus)', 'sus13') and rr == bass:
        return 'D8-13sus'

    # ---- 12. D3: 6/7-PC tertian stack named from the bass root -----------
    if rr == bass and pr != bass and rq in TERTIAN and upc >= 5:
        return 'D3-tertian-bass'

    # ---- 13. D4: altered dominant named from the bass root ---------------
    if rr == bass and pr != bass and rq in DOM_ALTS:
        return 'D4-altdom-bass'

    # ---- 14. general bass-coherent rename (D3/D4 family) ------------------
    if rr == bass and pr != bass:
        return 'D3D4-bass-coherent'

    # ---- 15. pure-scoring improvement (D2/D18): rust fits >= python -------
    if perr is not None and rerr is not None:
        if rerr < perr:
            return 'D2-scoring-better'
        if rerr == perr:
            return 'D2-scoring-equal'

    # ======================================================================
    # Beyond here rust fits *worse* than python by raw pc-count.  These are
    # the rows the audit (UNEXPLAINED.md) hand-reviews.  We still tag each
    # with the family it belongs to; a raw-fit penalty is NOT proof of a
    # regression (jazz shorthand like Cm13 for C-Eb-A-Bb legitimately omits
    # unstated extensions, which this metric counts as "missing").
    # ======================================================================

    # D4-family: python's spurious Δ7(#11) lydian reading demoted to a
    # dominant (the whole point of the rewrite; Δ7#11 appears 5570x in the
    # Python corpus). Root/quality differ but it is the same altered-dominant
    # ambiguity, resolved toward the dominant.
    if pf and ('Δ7(#11)' in pf or 'Δ9(#11)' in pf or 'Δ13#11' in pf) and \
            rq and (rq.startswith('7(') or rq in ('13(#11)', '9(b13)',
                                                   '7(#11)', '9', '13', '11')):
        return 'D4-lydian-demote'

    # augmented-symmetry: an aug triad + 1 note; rust prefers a plain major
    # triad + slash (E/Ab) and leaves one note unexplained where python's
    # Δ7#5 names all four. Rare, symmetric, ~2-point scoring margin.
    if pf and 'Δ7#5' in pf:
        return 'AUDIT-aug-symmetry'

    src = c['src']
    if 'rootless' in src or 'shell' in src:
        return 'AUDIT-rootless-shell-voicing'

    if upc >= 6:
        return 'AUDIT-ambiguous-dense'

    # small chromatic-ish cluster (<=4 pcs packed within 4 semitones)
    spcs = sorted(pcs)
    if upc <= 4:
        wrap = min((spcs[(i + upc - 1) % upc] - spcs[i]) % 12
                   for i in range(upc)) if upc > 1 else 0
        if wrap <= 4:
            return 'AUDIT-chromatic-cluster'

    return 'AUDIT-ambiguous-voicing'


def main():
    corp = json.load(open(os.path.join(HERE, 'corpus.json')))
    rust = json.load(open(os.path.join(HERE, 'rust-golden.json')))
    assert len(corp) == len(rust)
    out = []
    counts = collections.Counter()
    residual = []
    for c, r in zip(corp, rust):
        assert c['notes'] == r['notes']
        if c['flat'] == r['flat'] and c['sharp'] == r['sharp']:
            continue
        rule = classify(c, r)
        counts[rule] += 1
        rec = {
            'notes': c['notes'], 'src': c['src'],
            'python_flat': c['flat'], 'rust_flat': r['flat'],
            'python_sharp': c['sharp'], 'rust_sharp': r['sharp'],
            'rule': rule,
        }
        out.append(rec)
        if rule.startswith('AUDIT-'):
            residual.append(rec)
    json.dump(out, open(os.path.join(HERE, 'classified-divergences.json'), 'w'),
              indent=0)
    total = len(out)
    print(f'total mismatches: {total}')
    for k, v in counts.most_common():
        print(f'  {k:28} {v}')
    audit = sum(v for k, v in counts.items() if k.startswith('AUDIT-'))
    print(f'\nAUDIT rows (hand-reviewed in UNEXPLAINED.md): {audit}')
    return out, counts


def audit_rows():
    """The AUDIT-* residual, for UNEXPLAINED.md generation."""
    corp = json.load(open(os.path.join(HERE, 'corpus.json')))
    rust = json.load(open(os.path.join(HERE, 'rust-golden.json')))
    rows = []
    for c, r in zip(corp, rust):
        if c['flat'] == r['flat'] and c['sharp'] == r['sharp']:
            continue
        rule = classify(c, r)
        if rule.startswith('AUDIT-'):
            rows.append((rule, c, r))
    return rows


if __name__ == '__main__':
    main()
