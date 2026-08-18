# The 4.4.0 announcement email

Draft for the people who already hold a supporter key. Sent from
`keys@ivorymidi.com` via Resend, the same way 3.0.0 went out. Gumroad still
refuses customer email below a payout.

`keys@ivorymidi.com` **can send but cannot receive**, so Reply-To must be a
real inbox or every reply disappears silently.

## Read this before sending

**Send it AFTER the release is published.** Every link below is a
`releases/latest/download/` permalink and resolves by exact asset name against
whatever release is currently latest, so they serve the previous version until
4.4.0 is published and 404 if an asset name is missing. One earlier release was
announced ahead of its assets and 404'd for ten minutes.

`scripts/check-store-links.sh` fetches all seven of them in one command. Run it
after publishing and before sending.

---

**Subject:** Tangent 4.4 writes down what you play

---

Hello,

Tangent 4.4 is out, and it is the largest release since the app was rewritten
in Rust. Your supporter key still works: keys have no expiry and cover every
version, including this one.

**It writes music now.** There is a sheet music panel: what you are playing,
engraved properly. Six clefs (treble, bass, alto, tenor, and the octave-down
treble and bass a guitarist and a double bassist actually read), all fifteen
key signatures, accidentals spelled correctly for the key you are in, and `8va`
when the notes climb further than ledger lines can be counted. Letter names sit
inside the noteheads by default, so it is readable on day one.

**Stack as many clefs as you like, and every staff shows every note.** A
violist and a cellist can read the same chord in their own clefs, side by side,
from one keyboard. That is a teaching view, and it is the reason the panel
exists.

**The chord name moved onto the staff, and it names the runners-up.** Not just
"C6" but "C6, or Am7". A chord that is two things at once is a fact about
harmony, and being told only one of them has always been the least honest part
of this app.

**The number keys arrange the display.** The theory band is four panels (circle
of fifths, Tonnetz, harmonic triangles, sheet music) and `1` to `4` toggle
them. Press a number for a panel that is already showing and it moves to the
end, so the same four keys both choose what is up and put it where you want it.
Turn all four off and the band collapses and gives the height back to the keys.

**And a take is now simply the window.** A take used to be an arrangement of
its own: the app's panels fitted into one pane, your camera composited into
another, and a layout picker deciding which floated over which. That was a
second design of the same picture, and the two disagreed with each other. Now
the video is the window. The same panels in the same places at the same sizes,
with your camera where the window already puts it. What you were looking at is
what the file contains, at 16:9 with no crop, from one keypress.

**Video works on Windows and Linux now**, which it never has before. Those two
need `ffmpeg` on the PATH; a take that cannot find it tells you the install
command and still writes its audio and its MIDI. macOS needs nothing extra.

The recorder grew up with it: a real transport, a pair of VU meters, metronome
and input faders you can reach while your hands are busy, five instrument slots
instead of three, and the take's settings behind a cog.

**There is more coming, and I would rather say so.** 4.4 is a save point, not a
destination. 5.0 is the version this app has been heading toward since the
theory band arrived, and it needs a full pass over the code and a few things
that are not built yet. This is everything that is ready now, and it is
finished work rather than a preview: signed, notarized, and what I use every
day.

**One thing worth saying plainly.** There is nothing behind a supporter key in
this build, and nothing is planned. The heart that used to be the one thing a
key switched on is drawn for everybody now, in whatever colour they pick. You
already had every feature; now you have the heart too. I would rather tell you
that than let you assume otherwise.

Everything else you already had is unchanged: the VST3 plugin, the guitar neck,
the chords you have taught it, your colours, your tunings. Your settings carry
over. One thing will look different on purpose: the chord strip under the
theory band is off, because the sheet music carries the chord name itself.
Right-click and choose Show Chord Strip if you want it back.

Each installer offers the app and the VST3 plugin as separate choices.

  macOS 11 or later, Apple Silicon or Intel
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos.pkg

  Windows 10 or later
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-windows-setup.exe

  Linux x86_64. The tarball has an install.sh that needs no root
  https://github.com/ganten1998/ivory/releases/latest/download/tangent-linux-x86_64.tar.gz

  Checksums
  https://github.com/ganten1998/ivory/releases/latest/download/SHA256SUMS

Release notes and source:
https://github.com/ganten1998/ivory/releases/latest

Thank you for paying for something that is free. It is still free, and it is
better because a few people did that anyway.

Ganten

---

## How to actually send it

The buyer list exists in exactly one place, the fulfil ledger, so sending lives
there too:

```sh
flyctl ssh sftp get /data/ledger.jsonl -a ivory-fulfil      # 1. fetch the list
sed -n '/^Hello,$/,/^Ganten$/p' \
  docs/release/4.4/RELEASE-EMAIL-4.4.0.md > /tmp/body.txt

cd tools/ivory-fulfil                                        # 2. DRY RUN first
cargo run -- announce --subject "Tangent 4.4 writes down what you play" \
  --body /tmp/body.txt --ledger ~/ledger.jsonl

RESEND_API_KEY=... MAIL_FROM='Tangent <keys@ivorymidi.com>' \
  REPLY_TO='ganten7@gmail.com' \
  cargo run -- announce --subject "Tangent 4.4 writes down what you play" \
    --body /tmp/body.txt --ledger ~/ledger.jsonl --send      # 3. and only then
```

It is a **dry run unless `--send`** is given: it prints the recipients and the
exact body and stops. It dedupes, because the ledger is one row per SALE and a
person who bought twice is still one person.

## Checklist before sending

- [ ] 4.4.0 published on GitHub and showing as Latest, not a pre-release
- [ ] `scripts/check-store-links.sh` green
- [ ] Gumroad page updated from `GUMROAD.md`, screenshots replaced
- [ ] Reply-To set to a real inbox
- [ ] Dry run read in full, recipient count sane
