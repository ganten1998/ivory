# The 5.0.0-beta.2 announcement email

Draft for the people who already hold a supporter key. Sent from
`keys@ivorymidi.com` via Resend, the same way 3.0.0 went out. Gumroad still
refuses customer email below a payout.

`keys@ivorymidi.com` **can send but cannot receive**, so Reply-To must be a
real inbox or every reply disappears silently.

## Read this before sending

**Every link below is version-scoped, and that is not a style choice.**
5.0.0-beta.2 is published as a GitHub pre-release, so
`releases/latest/download/<name>` still resolves to 3.0.0 and will keep doing
so until a final 5.0 ships. That is the whole point of the pre-release flag:
nobody who has not asked for a beta gets handed one. It also means the
permalinks used in every previous email would quietly send these people to
3.0.0, so this email links to the tag instead.

**Decide whether to send this at all.** Previous announcements went out for
finished releases. This is a beta, and a beta announced to everyone who has
ever paid is a beta everyone will treat as finished. Two reasonable options:

- Send it, worded as it is below, which is explicit about what a beta is.
- Hold it, and send one announcement when 5.0 is final.

The draft assumes the first.

---

**Subject:** Tangent 5.0 is in beta: it writes the music down and records the room

---

Hello,

Tangent 5.0 is in beta. Your supporter key still works: keys have no expiry and
cover every version, including this one.

**This is a beta on purpose.** It is signed and notarized, it is the build I
use every day, and 5.0 changes what the app is for. So it goes out marked as
one, and the ordinary download links still point at the last finished release.
The links below are for this beta specifically.

**Tangent is not a MIDI monitor any more.** That is the honest summary of this
release. Two things did it.

**It writes music down.** There is a sheet music panel: what you are playing,
engraved properly. Six clefs (treble, bass, alto, tenor, and the octave-down
treble and bass a guitarist and a double bassist actually read), all fifteen
key signatures, accidentals spelled correctly for the key you are in, and `8va`
when the notes climb further than ledger lines can be counted. Letter names sit
inside the noteheads by default.

Put several clefs up at once and every staff shows every note. A violist and a
cellist read the same chord in their own clefs, side by side, off one keyboard.
That is a teaching view, and it is the reason the panel exists. The chord name
sits on the staff and names the runners-up too: not just "C6" but "C6, or Am7",
because a chord that is two things at once is a fact worth showing a student
rather than hiding from them.

**And a take is now simply the window.** This is the change 5.0 is named for. A
take used to be an arrangement of its own: the app's panels fitted into one
pane, your camera composited into another, and a layout picker deciding which
floated over which. That was a second design of the same picture, and the two
disagreed with each other.

Now the video is the window. The same panels in the same places at the same
sizes, with your camera where the window already puts it, at 16:9 with no crop.
There is no scene to build, no overlay to align and nothing to keep in step
with anything else. One press writes the audio, the MIDI and the `.mp4`.

Put those together and the app does something it could not do in 4.x: a lesson
recorded while you give it, or a take with the theory visible in it, from one
keypress. That is what I built the recorder for and it took until now to
actually work the way it should have.

The rest of the recorder grew up with it. The transport is a transport again,
with the take's settings behind a cog: record and stop, a pair of VU meters,
metronome and input faders you can reach while your hands are busy, and five
VST3 instrument slots instead of three, so the sound in the video is your
instrument.

The theory band is four panels now (circle of fifths, Tonnetz, harmonic
triangles, sheet music) and `1` to `4` toggle them. Press a number for a panel
that is already showing and it moves to the end, so the same four keys both
choose what is up and put it where you want it. That is meant to be used
mid-lesson.

**One thing worth saying plainly.** There is nothing behind a supporter key in
this build. The heart that used to be the one thing a key switched on is drawn
for everybody now, in whatever colour they pick, and it will stay that way at
least until 5.0 is final. You already had every feature; now you have the heart
too. I would rather tell you that than let you assume otherwise.

Everything else you already had is unchanged: the VST3 plugin, the guitar neck,
the chords you have taught it, your colours, your tunings. Your settings carry
over. One thing will look different on purpose: the chord strip under the
theory band is off, because the sheet music carries the chord name itself.
Right-click and choose Show Chord Strip if you want it back.

**And video is no longer macOS only.** Windows and Linux encode through
`ffmpeg`, which they need on the PATH; a take that cannot find it tells you the
install command and still writes its audio and its MIDI. That was the last
thing in this app that only worked on one platform.

  macOS 11 or later, Apple Silicon and Intel
  https://github.com/ganten1998/ivory/releases/download/v5.0.0-beta.2/Tangent-5.0.0-beta.2-macos-universal.dmg

  Windows 10 or later
  https://github.com/ganten1998/ivory/releases/download/v5.0.0-beta.2/tangent-5.0.0-beta.2-windows-x86_64.zip

  Linux x86_64. The tarball has an install.sh that needs no root
  https://github.com/ganten1998/ivory/releases/download/v5.0.0-beta.2/tangent-5.0.0-beta.2-linux-x86_64.tar.gz

The release page, with the aarch64 Linux build and the checksums:
https://github.com/ganten1998/ivory/releases/tag/v5.0.0-beta.2

If something in this one is wrong, I would genuinely like to know, and that is
most of what a beta is for.

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
  docs/release/5.0/RELEASE-EMAIL-5.0.0-beta.2.md > /tmp/body.txt

cd tools/ivory-fulfil                                        # 2. DRY RUN first
cargo run -- announce --subject "Tangent 5.0 is in beta: it writes the music down and records the room" \
  --body /tmp/body.txt --ledger ~/ledger.jsonl

RESEND_API_KEY=... MAIL_FROM='Tangent <keys@ivorymidi.com>' \
  REPLY_TO='ganten7@gmail.com' \
  cargo run -- announce --subject "Tangent 5.0 is in beta: it writes the music down and records the room" \
    --body /tmp/body.txt --ledger ~/ledger.jsonl --send      # 3. and only then
```

It is a **dry run unless `--send`** is given: it prints the recipients and the
exact body and stops. It dedupes, because the ledger is one row per SALE and a
person who bought twice is still one person.

## Checklist before sending

- [ ] Decided this beta is worth an announcement at all
- [ ] All four links above actually fetched once
- [ ] Confirmed `releases/latest` still shows 3.0.0, not the beta
- [ ] Gumroad page updated from `GUMROAD.md`, screenshots replaced
- [ ] Reply-To set to a real inbox
- [ ] Dry run read in full, recipient count sane
