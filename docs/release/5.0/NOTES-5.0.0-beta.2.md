Tangent writes down what you play, and records the window you played it in.

This is a beta. It is signed and notarized on macOS and it is the build the
author uses every day, but 5.0 is a large change to the two things you look at
most, so it goes out marked as one. Every permanent download link still points
at the last stable release, which is deliberate: nothing that a supporter key
already reached has moved.

**Nothing is behind a key.** There are no locked features in this build at all,
and there will not be any before 5.0 leaves beta. The supporter heart is drawn
for everybody now, in the colour you pick, and hovering it shows the people
this app exists because of.

## New in beta.2

**Video works on all three platforms.** macOS encodes natively through
AVFoundation with nothing to install. Windows and Linux encode through
`ffmpeg`, which they need on the PATH; a take that cannot find it tells you the
install command and still writes its audio and its MIDI.

That was the last thing in this app that only worked on one platform. The
encoder was half of it. The other half was the video compositor, which looked
macOS-only for years and never was: it is wgpu and egui, the same two things
that draw the window, and it only appeared tied to the platform because it
borrowed its graphics device from the window's renderer. It opens its own now.

Windows is compile-checked but has not been run by the author. If you are on
Windows, that is the most useful thing you could try in this beta.

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

This is the change 5.0 is named for. A take used to be an arrangement of its
own: the app's bands fitted into one pane, your camera composited into another,
and a layout picker deciding which floated over which. That was a second design
of the same picture, and the two disagreed.

Now the video is the window. The same bands in the same order at the same
sizes, with your camera where the window already puts it, full height at the
top left of the recorder band. There is no layout picker because there is
nothing left to arrange. What you were looking at is what the file contains.

The window is 16:9 while the usual bands are up, so a take needs no letterbox
and no crop.

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

  macOS 11 or later, Apple Silicon and Intel, signed and notarized
    Tangent-5.0.0-beta.2-macos-universal.dmg

  Windows 10 or later
    tangent-5.0.0-beta.2-windows-x86_64.zip

  Linux x86_64 and aarch64, glibc 2.32 or later, ALSA
    tangent-5.0.0-beta.2-linux-x86_64.tar.gz
    tangent-5.0.0-beta.2-linux-aarch64.tar.gz

Checksums are in SHA256SUMS.
