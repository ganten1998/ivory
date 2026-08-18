# The 4.2 announcement email

Draft for the people who already hold a supporter key. Sent from
`keys@ivorymidi.com` via Resend, the same way 3.0.0 went out. Gumroad still
refuses customer email below a payout.

`keys@ivorymidi.com` **can send but cannot receive**, so Reply-To must be a
real inbox or every reply disappears silently.

**Send it AFTER the release is published.** Every link below resolves by exact
asset name against whatever release is currently latest, so they 404 until the
4.2 assets exist. One earlier release was announced ahead of its assets and
404'd for ten minutes.

---

**Subject:** Tangent 4.2 writes down what you play

---

Hello,

Tangent 4.2 is out. Your supporter key still works: keys have no expiry and
cover every version, including this one.

**It writes music now.** There is a sheet music panel: what you are playing,
engraved properly. Six clefs (treble, bass, alto, tenor, and the octave-down
treble and bass a guitarist and a double bassist actually read), all fifteen
key signatures, accidentals spelled correctly for the key you are in, and
`8va` when the notes climb further than ledger lines can be counted. Letter
names sit inside the noteheads by default, so it is readable on day one.

**Stack as many clefs as you like, and every staff shows every note.** A
violist and a cellist can read the same chord in their own clefs, side by side,
from one keyboard. That is a teaching view, and it is the reason the panel
exists.

**The chord name moved onto the staff, and it names the runners-up.** Not just
"C6" but "C6, or Am7". A chord that is two things at once is a fact about
harmony, and being told only one of them has always been the least honest part
of this app.

**The number keys rearrange the display.** The theory band is four panels now
(circle of fifths, Tonnetz, harmonic triangles, sheet music) and `1` to `4`
toggle them. Press a number twice and that panel moves to the end, so the same
four keys both choose what is showing and put it where you want it. Turn all
four off and the band collapses and gives the height back to the keyboard.

**And it records.** Load a VST3 instrument, play, and get a `.wav`, a `.mid`
and a composited `.mp4` from one take: the app's own display as the video with
your camera small in the corner, or a 9:16 cut for reels. Count-in in any time
signature including 6/8, a metronome, VU meters, level faders per instrument.
Video export is macOS-only for now; on Windows and Linux a take still writes
its audio and MIDI and says so plainly rather than failing quietly.

Everything else you already had is still there and unchanged: the VST3 plugin,
the guitar neck, the chords you have taught it, your colours, your tunings.

  macOS 11 or later, Apple Silicon or Intel
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos.pkg

  Windows 10 or later
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-windows-setup.exe

  Linux x86_64. The tarball has an install.sh that needs no root
  https://github.com/ganten1998/ivory/releases/latest/download/tangent-linux-x86_64.tar.gz

The .dmg, .zip and .tar.gz are all still on the release page if you would
rather unpack it yourself:
https://github.com/ganten1998/ivory/releases/latest

Your settings carry over untouched. One thing will look different on purpose:
the sheet music panel is on, so the chord strip that used to sit under the
theory band is gone. The staff carries the chord name itself now, and having
it in two places was two places for them to disagree. Press `4` to send the
staff away and the strip comes back.

Thank you for paying for something that is free. It is still free, and it is
better because a few people did that anyway.

Ganten

---

## How to actually send it

The buyer list exists in exactly one place, the fulfil ledger, so sending lives
there too:

```sh
flyctl ssh sftp get /data/ledger.jsonl -a ivory-fulfil      # 1. fetch the list
sed -n '/^Hello,$/,/^Ganten$/p' docs/release/4.2/RELEASE-EMAIL-4.2.md > /tmp/body.txt

cd tools/ivory-fulfil                                        # 2. DRY RUN first
cargo run -- announce --subject "Tangent 4.2 writes down what you play" \
  --body /tmp/body.txt --ledger ~/ledger.jsonl

RESEND_API_KEY=... MAIL_FROM='Tangent <keys@ivorymidi.com>' \
  REPLY_TO='ganten7@gmail.com' \
  cargo run -- announce --subject "Tangent 4.2 writes down what you play" \
    --body /tmp/body.txt --ledger ~/ledger.jsonl --send      # 3. and only then
```

It is a **dry run unless `--send`** is given: it prints the recipients and the
exact body and stops. It dedupes, because the ledger is one row per SALE and a
person who bought twice is still one person.

## Checklist before sending

- [ ] 4.2 release published on GitHub, with every asset name the links above use
- [ ] All three links actually fetched once (they 404 until the assets exist)
- [ ] Gumroad page updated from `GUMROAD.md`, screenshots replaced
- [ ] Reply-To set to a real inbox
- [ ] Dry run read in full, recipient count sane
