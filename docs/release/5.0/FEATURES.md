# Tangent 5.0 feature list

Written to be lifted from. The short list is for a store page or a post; the
long list is the reference, grouped the way somebody deciding whether to
download it would want to read it.

Everything here is in the free build. There are no paid features.

---

## The short list

- Names what you play in real time, from plain triads to altered dominants,
  rootless voicings, slash chords and scales
- Engraves it on a staff: six clefs, all fifteen key signatures, correct
  accidental spelling, `8va`, letter names in the noteheads
- Names the runners-up too, so "C6, or Am7" instead of one answer to a question
  that has two
- Shows the same notes on a guitar neck, a circle of fifths and a Tonnetz
- Records a take as `.wav`, `.mid` and a composited `.mp4` of the window itself
- Runs in your DAW as a VST3 monitor
- Works offline, with no account and no telemetry
- macOS, Windows and Linux
- Free, with nothing locked

---

## The long list

### Reading what you play

- **All 88 keys**, drawn in real time from any MIDI keyboard.
- **Chord detection** by weighted scoring rather than lookup: triads, sevenths,
  extensions, alterations, suspensions, rootless jazz voicings, slash chords
  and scales.
- **The runners-up, named.** The readout carries the winner and the next two
  best matches.
- **Teach it your own names.** Hold a voicing, name it, and it is yours from
  then on. Corrections are learnable too.
- **Transpose** the readout without moving your hands.
- **Sharps or flats**, your choice, everywhere at once.

### Sheet music

- **Six clefs**: treble, bass, alto, tenor, and the octave-down treble and bass
  that a guitarist and a double bassist actually read.
- **All fifteen key signatures**, from the staff's own right-click menu.
- **Correct spelling for the key you are in**, so an E flat is an E flat and
  not a D sharp.
- **`8va` and `8vb`** when the notes climb or fall further than ledger lines
  can be counted.
- **Letter names inside the noteheads**, on by default, off with `U`.
- **Several clefs at once.** Stack as many as you like and every staff shows
  every note: a violist and a cellist read the same chord in their own clefs,
  side by side, from one keyboard.
- **Noteheads that never overlap**, including seconds and augmented unisons.

### The theory band

- **Four panels**: circle of fifths, Tonnetz, harmonic triangles, sheet music.
- **`1` to `4` arrange them.** A number toggles its panel; pressing it for a
  panel that is already showing moves that panel to the end. Turn all four off
  and the band collapses and gives the height back to the keyboard.
- **Circle of fifths** with each key shaded by how much of what you are playing
  fits it.
- **Tonnetz**, the lattice where left to right is fifths and the diagonals are
  thirds, with your triads lit as triangles.
- **Harmonic triangles**, I, IV and V pointing up with i, iv and v inverted.
- **Follows MIDI** by default, so the diagrams track what you play.

### The guitar neck

- **Real fingerings**, where a player would actually put their fingers, not
  wherever the notes happen to fall.
- **Playable as an input.** Click the neck to place notes and read the chord
  off the piano above.
- **Barres**, laid by holding and dragging along a fret.
- **Every shipped tuning**, plus your own, plus a capo.

### Recording

- **A take is the window.** The `.mp4` is the same panels in the same places at
  the same sizes, with your camera where the window already puts it. There is
  no separate video layout to keep in your head, because there is not one.
- **16:9**, so a take needs neither letterbox nor crop.
- **`.wav`, `.mid` and `.mp4`** from one press.
- **Five VST3 instrument slots**, each with its own level and its own window.
- **A transport**: record, stop, a pair of VU meters, and metronome and input
  faders you can reach while your hands are busy.
- **Count-in** in any time signature including 6/8, in or out of the file.
- **A settings panel** behind the cog for where takes go, the camera, the audio
  input, the count-in, the export and what happens when a take finishes.
- Video export is macOS only for now. On Windows and Linux a take still writes
  its audio and MIDI, and says so rather than failing quietly.

### In your DAW

- **VST3 plugin** with the same display, on a MIDI or instrument track.
- **Makes no sound.** It is a monitor, not an instrument.

### The app itself

- **Offline.** No account, no telemetry, nothing phoned home. It works on a
  plane.
- **Dark mode**, and every colour in it is yours to set.
- **Three bundled typefaces**, cycled with `F`.
- **Panels you can tear off** into their own windows.
- **Hold `H`** for every keyboard shortcut.
- **macOS 11+** (Apple Silicon and Intel, signed and notarized), **Windows
  10+**, **Linux** (glibc 2.32+, ALSA, Wayland or X11).
- **MIT licensed.** Rebuild it, modify it, redistribute it.
