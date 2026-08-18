# Tangent 5.0 feature list

Written to be lifted from. Grouped by what somebody would be trying to DO,
because that is how the app is now: a monitor is a thing you glance at, and
Tangent is a thing you teach a lesson on or record a take with.

Everything here is in the free build. There are no paid features.

---

## In one sentence

Play a MIDI keyboard, and Tangent names the chord, engraves it, explains it
three ways, and records the whole window as a video with your camera already in
it.

## In three bullets

- **See it.** All 88 keys, the chord named with its runners-up, sheet music in
  six clefs, and three theory diagrams that show why it works.
- **Teach from it.** Several clefs at once with every staff showing every note,
  panels you rearrange with the number keys mid-lesson, and chord names you can
  teach it yourself.
- **Record it.** One press gives you the audio, the MIDI, and an `.mp4` of the
  window itself, camera included, at 16:9, with no scene to build.

---

## For teaching

- **Several clefs at once, every staff showing every note.** A violist and a
  cellist read the same chord in their own clefs, side by side, from one
  keyboard. Six are available: treble, bass, alto, tenor, and the octave-down
  treble and bass that a guitarist and a double bassist actually read.
- **Notation that is actually correct.** All fifteen key signatures, and
  accidentals spelled for the key you are in, so an E flat is an E flat and not
  a D sharp. `8va` and `8vb` when the notes go past what ledger lines can say.
- **Letter names inside the noteheads**, on by default, off with `U` once they
  are not needed any more.
- **The runners-up, named.** The readout carries the winner and the next two
  best matches, so a student sees that C6 and Am7 are the same four notes. That
  is a lesson, not a limitation.
- **Three ways of showing why.** The circle of fifths with each key shaded by
  how much of what you are playing fits it; a Tonnetz where left to right is
  fifths and the diagonals are thirds, with triads lit as triangles; harmonic
  triangles with I, IV and V pointing up and i, iv and v inverted.
- **Rearrange the board mid-lesson.** `1` to `4` toggle the four theory panels.
  Pressing a number for a panel that is already up moves it to the end, so the
  same four keys choose what is showing and where it sits. All four off
  collapses the band and gives the height back to the keyboard.
- **A guitar neck** for a mixed-instrument class: real fingerings, playable as
  an input, with barres, a capo and every shipped tuning plus your own.
- **Teach it your own names.** Hold a voicing, name it, and it is yours from
  then on. Corrections are learnable too.
- **Sharps or flats**, your choice, everywhere at once.
- **No account, no wifi.** It is entirely offline, which matters in a room with
  a school network.

## For recording and performing

- **A take is the window.** The `.mp4` is the same panels in the same places at
  the same sizes, with your camera where the window already puts it. There is
  no scene to build, no overlay to align and no second layout to keep in step
  with the first. What you were looking at is what your audience sees.
- **16:9**, so a take needs neither letterbox nor crop.
- **One press, three files**: `.wav`, `.mid` and `.mp4`.
- **The MIDI is real MIDI**, so a take can be re-rendered with a different
  instrument, notated, or edited long after the video exists.
- **Five VST3 instrument slots**, each with its own level and its own editor
  window, so the sound in the recording is your instrument.
- **A transport you can use while playing**: record and stop, a pair of VU
  meters, and metronome and input faders within reach of a hand that is busy.
- **A count-in** in any time signature including 6/8, kept out of the file or
  recorded into it.
- **A take-settings panel** behind the cog: where takes go, the camera, the
  audio input, the count-in, the export, and what happens when a take finishes.
- **Video on all three platforms.** macOS encodes natively. Windows and Linux
  need `ffmpeg` on the PATH, and a take that cannot find it says so, names the
  install command, and still writes its audio and its MIDI.

## For playing

- **All 88 keys** in real time from any MIDI keyboard.
- **Chord detection by weighted scoring** rather than lookup: triads, sevenths,
  extensions, alterations, suspensions, rootless jazz voicings, slash chords
  and scales.
- **Transpose the readout** without moving your hands.
- **Click to place notes** on the piano, the neck or the diagrams, so you can
  work something out without an instrument in front of you.
- **Runs in your DAW.** VST3 plugin with the same display, on a MIDI or
  instrument track. It makes no sound; it is a monitor.

## The app itself

- **Dark mode**, and every colour in it is yours to set.
- **Three bundled typefaces**, cycled with `F`.
- **Panels you can tear off** into their own windows.
- **Hold `H`** for every keyboard shortcut.
- **macOS 11+** (Apple Silicon and Intel, signed and notarized), **Windows
  10+**, **Linux** (glibc 2.32+, ALSA, Wayland or X11).
- **MIT licensed.** Rebuild it, modify it, redistribute it.
- **Free, with nothing locked.** No trial, no watermark, no export limit. A
  supporter key gates no feature; what it does is put you on the list that
  hears about new versions, and in front of whatever gets made for supporters
  later.
