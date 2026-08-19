# Tines and Boxes

**Tangent Bank 01 — sixteen electric pianos and sixteen jazz guitars for the DX7 engine.**
Written 19 August 2026. All thirty-two voices are original: no patch in this bank is
copied, and the last section shows the measurements that back that up.

---

## 1. What was studied

`DX7_AllTheWeb.zip` in this folder holds 13,192 `.syx` cartridges. Parsed as 32-voice
bulk dumps that is 356,096 voices, of which **42,291 are unique** once byte-identical
duplicates are removed. The corpus is mostly the same few hundred patches copied
between cartridges for forty years, which is itself the most useful fact in it: the
electric piano and the jazz guitar each have one dominant ancestor, and almost
everything filed under those names is a light edit of it.

Voices were classified by name. After excluding rock, bass, brass and synth-lead
names, that gives **1,387 electric pianos** and **791 clean guitars**, of which **86**
are explicitly jazz guitars.

Two other things were needed and were fetched rather than guessed:

- **The algorithm table.** Decoded from Dexed's `algorithms[32]` opcode table and
  verified against the corpus. It agrees with the chart on the front of the instrument
  and with `ivory/src/dx7/algorithms.rs`, which is a useful independent check on that
  transcription — the file's own doc comment says a wrong entry there does not fail,
  it just plays the wrong patch.
- **A renderer.** Tangent's own `ivory/src/dx7` engine, lifted into a standalone
  harness so every claim below is measured rather than asserted. Nothing in the
  repository was modified.

---

## 2. What makes a DX7 patch sound like an electric piano

Measured across the 1,387 electric pianos, against the whole-corpus baseline.

| | electric pianos | whole corpus |
|---|---|---|
| algorithm 5 | **51%** | 12% |
| feedback 6 | **42%** | 14% |
| carrier ratio 1.00 | **84%** | 48% |
| modulator ratio 14.00 | **11%** | under 1% |
| modulator velocity sensitivity 6–7 | **42%** | 23% |
| rate scaling (median) | **3** | 1 |
| carrier detune spread (median) | **±3** | ±1 |
| brightness decays faster than loudness | **79%** | 57% |

Read together those numbers describe one machine:

**1. Three independent two-operator stacks.** Algorithm 5 is `6→5`, `4→3`, `2→1` with
carriers 1, 3 and 5. Half of all electric pianos are on it. It is not a habit — it is
what a tine piano *is*: several struck bars sounding at once, each with its own
strike and its own decay, summed rather than stacked.

**2. The strike is a high-ratio modulator that dies immediately.** The famous one is
ratio 14.00 with output around 58–72, envelope falling to zero in a fifth of a second,
and **velocity sensitivity 7** — 63% of all high-ratio operators in the set are at
maximum velocity sensitivity. This is the single most important parameter in the sound.
It is why the instrument gets *brighter* when you hit it harder rather than just louder,
and it is what a sampled piano needs a dozen velocity layers to fake.

**3. The body is a 1:1 modulator at output 79–90 with velocity sensitivity 6.** A sine
modulated at the unison spreads into a harmonic series that thins as the modulator's
envelope falls. That decay from bright to pure, inside one note, is the whole trick.

**4. The carriers are detuned against each other by ±3 to ±7.** No LFO — median LFO
depth in the set is zero. The shimmer is beating between stacks, not vibrato.

**5. Rate scaling 3 everywhere,** so high notes decay faster, and no sustain segment at
all: L3 and L4 are zero, so a held note dies the way a struck bar dies.

---

## 3. What makes a DX7 patch sound like a jazz guitar

Measured across the 86 jazz guitars. The concentration here is extreme.

| | jazz guitars | clean guitars | electric pianos |
|---|---|---|---|
| algorithm 8 | **77%** | 24% | 2% |
| feedback 7 (maximum) | **97%** | 54% | 23% |
| modulator ratio 3.00 | **65%** | 27% | 3% |
| carrier detune spread | **0** | 0 | ±3 |
| transpose 12 (an octave down) | **64%** | 51% | 2% |
| carrier decay rate R3 (median) | **24** | 24 | 20 |

**1. Ratio 3.00, not 1.00.** Two thirds of jazz guitar modulators sit at the third
harmonic. That odd-ratio modulation is the hollow, woody, slightly nasal formant of an
archtop body. Ratio 1 sounds like an electric piano; ratio 2 sounds like a clarinet;
ratio 3 sounds like a box with a hole in it.

**2. Maximum feedback on a modulator.** 97% are at feedback 7, and in algorithm 8 the
feedback operator is OP4 — a modulator, not a carrier. An operator modulating itself at
full depth approaches a sawtooth, and feeding *that* into the carrier is the grit of a
wound string and a pick, which no amount of sine-on-sine will produce.

**3. A high-ratio burst that is over before the note is.** OP6 at ratio 14, output ~75,
envelope `99 57 99 75` falling to level 0 — full level instantly, gone in about
200 ms. It modulates a modulator rather than a carrier, so it reads as a *pick* rather
than as a bell struck alongside the string.

**4. No detune.** Detune spread is exactly zero. A guitar is one string; the chorusing
that makes an electric piano lush makes a guitar sound like a synthesizer.

**5. Level scaling that dulls the tone up the neck.** Right depth 15–65 on the
modulators with the `-LIN` curve. High notes lose brightness and sustain, as they do on
a real instrument.

**6. An octave down.** Transpose 12, because guitar music is written an octave above
where it sounds.

### The thing the corpus gets wrong

77% on one algorithm and 65% on one ratio is not a description of guitars — it is 86
copies of one 1983 factory patch. The principles above are real; the monoculture is
not. This bank keeps the principles and spreads them across ten algorithms.

---

## 4. How the bank was verified

Every voice was rendered through Tangent's DX7 engine and measured, then compared
against the same measurements taken from the factory references (`E.PIANO 1`,
`JAZZ GUIT1`, `RHODES`, `WURLITZER`, `FullTines`, `PICKGUITAR`, `SPANISHGTR`,
`NYLON G2TR` and others). Targets were set from the references rather than invented.

Two measurements did the work:

- **Attack time and decay-to-−20 dB**, which separate a struck bar from a plucked string.
- **HF ratio** — the share of signal power surviving a first-difference highpass, taken
  over the attack (0–150 ms) and the body (0.4–1.2 s). A pure sine at the fundamental
  reads about 0.0001; rich FM reads ten to a hundred times that. The *ratio* of the two
  is how much brighter the attack is than the sustain, which is the one number that
  captures "bright strike settling into a mellow tone".

A spectral centroid was tried first and thrown out: the fundamental dominates it so
heavily that removing an entire modulator moved it by 4%.

**Reference bands, and where this bank lands (C3, velocity 0.8):**

| | attack | decay to −20 dB | HF attack | HF attack/body |
|---|---|---|---|---|
| reference EPs | 5.9–24 ms | 0.35–0.99 s | 0.00013–0.00074 | 1.1–6.4 |
| **this bank's EPs** | 5.4–26.6 ms | 0.35–1.51 s | 0.00010–0.00057 | 1.0–1.6 |
| reference guitars | 20–104 ms | 0.42–1.72 s | 0.00009–0.00100 | 0.5–2.8 |
| **this bank's guitars** | 12–260 ms | 0.15–1.28 s | 0.00009–0.00084 | 1.1–10.0 |

Also checked at C1 and C6 and at velocity 0.30: every voice sounds across the whole
keyboard, rate scaling shortens the top octave without erasing it, and low velocity
mellows every patch rather than silencing it.

### Three things this process caught

- **Envelope levels are logarithmic.** An L2 of 74 is not "three quarters" — it is about
  21 dB down. A first pass at the guitars used L2 values in the 60s and 70s intending a
  two-slope decay and produced sixteen notes that were effectively over in 150 ms.
- **Modulator output level is steeply non-linear.** At output 60 a modulator is
  inaudible; at 80 it is faintly present; at 99 it is transformative. Brightness lives
  in the top fifteen units of the range.
- **The pick has to last.** Bursts set to R2 76 vanish in 40 ms and no listener hears a
  pick at all. The reference sits near R2 57, about 200 ms.

---

## 5. The bank

Sixteen tines, sixteen boxes, eleven algorithms. `Tangent-01-Tines-and-Boxes.syx` is a
standard 4,104-byte 32-voice bulk dump; `patch-source.py` is the readable definition of
every voice; `previews/` holds a rendered audition of each patch playing C2, C3, C4 and
C5 at rising velocity.

### Tines (1–16)

| # | name | alg | fb | the idea |
|---|---|---|---|---|
| 1 | TINE ONE | 5 | 6 | The centre of the bank. Strike at ratio 13, warm body, ±6 detune. |
| 2 | TINE TWO | 5 | 7 | Brighter and barkier: strike at 15, body up, full feedback. |
| 3 | TREMOLINE | 5 | 6 | TINE ONE with amplitude LFO on all three carriers. |
| 4 | NIGHT KEYS | 5 | 5 | Dark and long. Strike pulled back to 54, decay stretched. |
| 5 | BARKER | 5 | 7 | The aggressive one. Body modulator at 97, velocity sensitivity 7. |
| 6 | REED BOX | 5 | 7 | No tine at all — ratio 2 and 6 for a hollow reed bark. |
| 7 | BELL KEYS | 6 | 4 | Strike at ratio 20 that lingers instead of pinging. Feedback on a carrier. |
| 8 | BALLAD EP | 5 | 5 | Soft, slow, opens up only when you lean on it. |
| 9 | FUNK TINE | 5 | 6 | Short and percussive, strike at 17, rate scaling pulled back so the top octave survives. |
| 10 | WIDE TINE | 5 | 6 | Maximum detune spread across the three stacks. |
| 11 | GLASS KEYS | 3 | 5 | Two three-operator stacks instead of three pairs. Glassier, more complex. |
| 12 | VIBE KEYS | 5 | 3 | Ratio 7 and 4 modulators with deep slow tremolo. |
| 13 | HAMMER EP | 14 | 7 | Strikes at 9 and 19 driving the body modulator, not sitting beside it. A struck stack. |
| 14 | DEEP KEYS | 5 | 6 | An octave down, heavy body, strong scaling. |
| 15 | AIR KEYS | 22 | 5 | One modulator into three carriers at 1, 2 and 3, the upper two fading first. |
| 16 | VELVET EP | 5 | 0 | No feedback, gentlest strike, longest decay. The quiet end. |

### Boxes (17–32)

| # | name | alg | fb | the idea |
|---|---|---|---|---|
| 17 | ARCHTOP | 14 | 6 | The centre of the guitar half. Ratios 2, 3 and 7 stacked, pick at 16. |
| 18 | NECK WARM | 15 | 5 | Feedback moved to OP2, everything darker, heavy scaling up the neck. |
| 19 | BRIDGE CUT | 8 | 7 | Bright and forward: ratio 5 body, pick at 18. |
| 20 | NYLON AIR | 5 | 2 | Three parallel pairs, almost no grit. Nylon, not steel. |
| 21 | PALM MUTE | 18 | 6 | Single carrier, dense, dead inside a third of a second. |
| 22 | OCTAVES | 22 | 4 | Carriers at 1, 2 and 4 sounding together. |
| 23 | HOLLOW BOX | 14 | 7 | Two ratio-2 modulators for maximum boxiness, full feedback. |
| 24 | HARMONIC | 3 | 4 | Ratios 4, 6, 9 and a pick at 21. Chimes rather than wood. |
| 25 | CHORUS JZ | 5 | 5 | The one guitar that breaks the no-detune rule, on purpose. |
| 26 | FINGERTIP | 15 | 3 | Slow attack, no pick, round. |
| 27 | COMP SHORT | 8 | 7 | Staccato comping. |
| 28 | ROUND BOX | 16 | 5 | Single carrier, deep, the darkest scaling in the bank. |
| 29 | GYPSY PICK | 18 | 7 | Hardest attack here — pick at ratio 23. |
| 30 | SWELL GTR | 1 | 4 | Four-deep stack with a quarter-second swell. |
| 31 | TWELVE | 22 | 5 | Detuned carrier pairs an octave apart. |
| 32 | VELVET JZ | 2 | 3 | Darkest and smoothest. The quiet end of the guitars. |

---

## 6. Originality

The brief was variations on two themes, not copies, and the bank has to be
unambiguously yours to ship.

**Method.** Every one of the 32 voices was compared against all 42,291 unique corpus
voices on all 135 stored parameters. Two calibration pairs give the scale:

- `E.PIANO 1` vs `RHODES` — a known clone pair — agree on **93%** of non-trivial parameters.
- `E.PIANO 1` vs `JAZZ GUIT1` — unrelated instruments — agree on **18%**.

**Result.**

- **Zero of 32** voices are byte-identical to anything in the corpus.
- Agreement with the nearest corpus voice ranges **28% to 41%, mean 35%** — much nearer
  the unrelated benchmark than the clone benchmark.
- The nearest neighbours are scattered across many different cartridges rather than all
  pointing at one ancestor.

**What was rejected along the way.** The first version of the guitar half was written
directly from `JAZZ GUIT1`'s parameters and landed at 44–50% agreement, with every
operator within one or two units of the original and fourteen of sixteen patches on
algorithm 8. Measured side by side it was the factory patch with jitter on it. It was
thrown away and rewritten from the principles in section 3 instead, which is what
moved the guitars onto ten algorithms and down to 39–41%.

**What is deliberately shared, and why that is fine.** Algorithm numbers, ratio 1.00
carriers, ratio 3.00 modulators and transpose 12 recur here because they are how the
instrument works, in the way that a ii–V–I is how a turnaround works. What is not
shared is any actual parameter set.

**One caveat worth stating plainly.** These are measurements, not a legal opinion, and
"sounds like a Rhodes" is a goal here rather than a risk. No patch name in the bank
uses a trademark — no Rhodes, Wurlitzer, Fender, Gibson or model number — which is the
part that would actually cause trouble.

---

## 7. Caveats

- **The renderer is Tangent's engine, which is structurally exact but calibrated rather
  than cycle-accurate** — its own docs say so. Envelope rate and level curves are fitted
  functions, not the 1983 lookup tables. On real hardware or in Dexed these patches will
  sit a shade brighter or decay a shade differently. They will not be different sounds.
- **The previews are rendered from that same engine,** so they are a fair preview of
  Tangent and an approximation of hardware.
- **Nothing here has been heard.** Every judgement in this document is a measurement
  against a measured reference. The bands are drawn from the factory patches, so a voice
  landing inside them is behaving like the thing it is imitating — but the final call on
  whether TINE ONE is a *good* electric piano is yours, and the previews are there so
  that call is quick to make.
- **Level match is approximate.** The guitars cannot exceed output 99, so the tines were
  trimmed 6 units to meet them. Peaks across the bank span about 7 dB, which is normal
  for a DX7 cartridge but not mastered.
