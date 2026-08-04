# Ivory 2.1.0 — Chord Learning test build

Thanks for trying this. Ivory watches your MIDI keyboard, draws the notes, and
names the chord you are playing. **Chord Learning** is the new, experimental
part: you can tell Ivory you would rather see a different name, and it adjusts.

No MIDI keyboard? You can still test everything: right-click → **Enable
Keytoggle**, then click keys on the drawn piano to hold notes.

## Running it

**Windows** — unzip anywhere and run `ivory.exe`. Windows will say *"Windows
protected your PC"*, because the app is not code-signed: click **More info** →
**Run anyway**. (It is a plain Rust program, no installer, no network access.)

**macOS** — open the `.dmg`, drag `Ivory.app` to Applications. The app is
ad-hoc signed but not notarized, so the first launch is blocked: double-click it,
accept the warning, then go to **System Settings → Privacy & Security**, scroll
to Security, and click **Open Anyway** next to Ivory. (The old right-click →
Open trick no longer works on macOS 15 and later.)

Everything is in the **right-click menu** — that is the entire interface.

## The two different "teach" features

They are easy to confuse, so:

| Menu item | What it does |
|---|---|
| **Teach Chord Name...** | Pins an exact name to that exact voicing, forever. A dictionary entry. Precise, boring, always works. |
| **Correct Chord Name...** | Trains a general *preference*. It does not memorise this one chord — it nudges how Ivory weighs readings, so similar chords shift too. This is the experiment. |

## What to try

1. Hold a chord Ivory names in a way you disagree with.
2. Right-click → **Correct Chord Name...**
3. Pick the name you would rather see from the list, then **Learn**.
   - The list only contains readings Ivory actually considered. If the name you
     want is not there, Ivory cannot be talked into it — use *Teach Chord
     Name...* to pin it instead.
   - The number on the right is that reading's score. Small gaps move easily;
     large gaps will not move at all.
4. Read the message. It will tell you one of:
   - **Learned** — it worked, and what the chord now reads as.
   - **could not be nudged that far** — the gap was too big; *nothing changed*.
   - **already wins** — that name was already Ivory's choice.
5. Play some other chords and see whether anything else changed. **This is the
   interesting part** — the feature is meant to generalise, so it is supposed to
   affect similar voicings. Whether it does so usefully or annoyingly is exactly
   what we do not know yet.

Good first case if you want one: hold **C–E–G–A with C as the lowest note**.
Ivory says `C6`; many players would call it `Am7`. Correct it and see what else
moves. (Voice it with E at the bottom instead and Ivory names it by a fixed
rule, so it will tell you there is nothing to re-rank — that is expected.)

**How far one correction reaches, measured:** against a 13,133-voicing test
corpus, that single `C6 → Am7` correction changes **1,182 readings (9%)**, many
of them in unrelated keys. So if chords you were happy with start reading oddly
after a correction, that is the feature working as designed, not a glitch —
and it is precisely what we want your opinion on. **Forget Learning** restored
all 13,133 exactly in testing.

## Undo

- Right-click → **Manage Taught Chords...** shows whether learning is on, how
  many corrections you have made, and what it has picked up. **Forget Learning**
  wipes all of it and restores stock behaviour.
- Right-click → **Disable Chord Learning** silences it without erasing anything.
- Nothing is sent anywhere. Everything lives in `~/.config/ivory/overrides.json`
  (on Windows: `C:\Users\<you>\.config\ivory\overrides.json`). Deleting that file
  is a full reset.

## What is worth reporting back

- Chord names you think are plain wrong (with the exact notes you held).
- Anything that crashed, froze, or looked broken/clipped.
- Whether "Correct Chord Name..." ever did something you did not expect —
  especially if a correction wrecked a chord you were happy with.
- Whether the messages made sense, or left you guessing.
- Does your MIDI keyboard get detected? (right-click → **Select MIDI Input...**)

Known and expected: the app icon looks soft at large sizes, and Windows shows the
unsigned-app warning above.
