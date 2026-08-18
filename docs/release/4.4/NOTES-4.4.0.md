Tangent writes down what you play, and records the window you played it in.

This is the largest release since the Rust rewrite, and it is finished work:
signed and notarized on macOS, and the build the author uses every day. It is
also a save point rather than a destination. 5.0 is coming, and it will be the
version this app has been heading toward since the theory band arrived. This is
everything that is ready now.

**Nothing is behind a supporter key.** There are no locked features in this
build and none are planned. The heart is drawn for everybody, in the colour
they pick, and hovering it shows the people this app exists because of.

## Sheet music

There is a staff panel. What you are playing, engraved properly: six clefs
(treble, bass, alto, tenor, and the octave-down treble and bass a guitarist and
a double bassist actually read), all fifteen key signatures, accidentals
spelled for the key you are in, and `8va` when the notes climb further than
ledger lines can be counted. Letter names sit inside the noteheads by default,
so it is readable on day one.

Stack as many clefs as you like and every staff shows every note. A violist and
a cellist can read the same chord in their own clefs, side by side, from one
keyboard. That is a teaching view, and it is the reason the panel exists.

The chord name lives on the staff and it names the runners-up. Not just "C6"
but "C6, or Am7". A chord that is two things at once is a fact about harmony,
and being told only one of them was the least honest thing this app did.

## The theory band is four panels you arrange

Circle of fifths, Tonnetz, harmonic triangles, sheet music. `1` to `4` toggle
them. Press a number for a panel that is already showing and it moves to the
end, so the same four keys both choose what is up and put it where you want it.
Turn all four off and the band collapses and gives the height back to the
keyboard. Press any number from empty and that panel fills the band on its own.

## A take is the window

A take used to be an arrangement of its own: the app's bands fitted into one
pane, your camera composited into another, and a layout picker deciding which
floated over which. That was a second design of the same picture, and the two
disagreed.

Now the video is the window. The same bands in the same order at the same
sizes, with your camera where the window already puts it, full height at the
top left of the recorder band. There is no layout picker because there is
nothing left to arrange. What you were looking at is what the file contains.

The window is 16:9 while the usual bands are up, so a take needs no letterbox
and no crop.

**And video now works on Windows and Linux**, which it never has before. Those
platforms encode through `ffmpeg` and need it on the PATH; a take that cannot
find it names the install command and still writes its audio and its MIDI.
macOS encodes natively and needs nothing extra.

## The recorder band is a transport again

The take's settings left the band for a panel behind a settings cog: where
takes go, the camera, the audio input, the count-in, the time signature, the
export, and the four switches about what a take does. They are boxes with
captions and values, which is what they always should have been.

What the space bought: record and stop as a proper transport, a pair of VU
meters modelled on the one on the author's desk, reachable metronome and input
faders, and five instrument slots instead of three.

## Also

- The chord strip is a setting now, off by default, and the sheet music no
  longer overrules it. Turn it on for piano and chord name and nothing else,
  which is the shape this app had for years.
- Key signatures are on the right-click menu of the staff.
- Recursive VST3 scanning with folders you can add yourself, and a rescan that
  does not need a restart.
- A launch splash that spells TANGENT on the app's own Tonnetz.

Everything you already had is unchanged: the VST3 plugin, the guitar neck, the
chords you have taught it, your colours, your tunings. Your settings carry over.

## Downloads

Each installer offers the app and the VST3 plugin as separate choices.

  macOS 11 or later, Apple Silicon and Intel, signed and notarized
    Tangent-macos.pkg

  Windows 10 or later
    Tangent-windows-setup.exe

  Linux x86_64 and aarch64, glibc 2.32 or later, ALSA. The tarball has an
  install.sh that needs no root
    tangent-linux-x86_64.tar.gz
    tangent-4.4.0-linux-aarch64.tar.gz

Checksums are in SHA256SUMS.
