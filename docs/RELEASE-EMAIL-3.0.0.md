# The 3.0.0 announcement email

Draft for the people who have already bought a supporter key. Send from
`keys@ivorymidi.com` via Resend, the same way the 2.3.0 note went out — Gumroad
still refuses customer email below a payout or $100 in sales.

`keys@ivorymidi.com` can send but cannot receive, so **Reply-To must be a real
inbox** or a reply disappears.

**Send it AFTER the release is published.** Every link below resolves by exact
asset name against whatever release is currently latest, so they 404 until the
3.0.0 assets exist. The 2.3.0 note went out correctly; the version before that
was redeployed ahead of its assets and 404'd for ten minutes.

---

**Subject:** Tangent 3.0 — it runs in your DAW now

---

Hello,

Tangent 3.0 is out. You already have a supporter key and it still works — keys
have no expiry and this one covers every version.

**It runs inside your DAW.** There is a VST3 plugin now, with the same display
as the app: the chord readout, the guitar neck, everything. Put it on a MIDI or
instrument track and it names what is on that track. It makes no sound of its
own — it is a monitor, not an instrument.

**There is a theory band.** A section above the keyboard with three ways of
seeing what you are playing, side by side if you want them: the circle of
fifths, with every key shaded by how much of your chord belongs to it; a
Tonnetz, where every major triad is a triangle pointing up and two chords that
share two notes share an edge; and the I-IV-V triangles. Press T to cycle
through them.

**Naming a chord is one keystroke.** Hold the chord, press N, and type — the
box opens with the current reading already selected. Hold H at any time to see
every shortcut.

**The guitar neck works both ways.** Click it to place notes and read the chord
off the piano above, instead of only the other way round. Hold and drag along a
fret to lay a barre.

**And there are proper installers**, one per platform. Each offers the app and
the plugin as separate choices and puts the plugin where your DAW already
looks, so there is nothing to drag anywhere.

  macOS 11 or later, Apple Silicon or Intel
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos.pkg

  Windows 10 or later
  https://github.com/ganten1998/ivory/releases/latest/download/Tangent-windows-setup.exe

  Linux x86_64 — the tarball has an install.sh that needs no root
  https://github.com/ganten1998/ivory/releases/latest/download/tangent-linux-x86_64.tar.gz

If you would rather unpack it yourself, the .dmg, .zip and .tar.gz are all
still there on the release page.

Your settings, your colours and any chord names you have taught it carry over
untouched.

Thank you for paying for something that was free. It is still free, and it is
better because a few people did that anyway.

Ganten

---

## Checklist before sending

- [ ] 3.0.0 is published and all three installer links resolve (click each one)
- [ ] `Tangent-macos-arm64.dmg` and `Ivory-macos-arm64.dmg` still resolve — the
      universal build renamed the artifact, and `publish-github.sh` uploads the
      arm64-named aliases so older emails keep working. Check both.
- [ ] The fulfil service is redeployed, so NEW buyers get the installer links
      (`tools/ivory-fulfil`, then `fly deploy`)
- [ ] Reply-To set to an inbox that exists
- [ ] Sent to the existing purchasers only
