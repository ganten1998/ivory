# Gumroad store content

The text a buyer sees after paying, and where each piece of it lives.

There are two surfaces, and only one of them is inside this repo:

| surface | who owns it | how it is edited |
|---|---|---|
| Post-purchase page ("View content") | Gumroad | web UI, paste the block below |
| Supporter-key email | us | `tools/ivory-fulfil/src/main.rs`, `email_body()` |

Gumroad's own receipt email is not customisable in any useful way, so the
supporter-key email is where the download links have to be. It is the one
message a buyer keeps.

## The links, and why they are shaped this way

Every download link points at
`https://github.com/ganten1998/ivory/releases/latest/download/<name>`, which
GitHub resolves by **exact asset name** against whatever release is currently
latest. Nothing here is version-pinned, so a page written today still hands out
2.9.0 in two years without being touched.

That guarantee rests entirely on `scripts/publish-github.sh` uploading the
version-less alias assets on every release. If a future release ships only
`Tangent-2.3.0-macos-arm64.dmg`, all of these 404 at once, for buyers as well as
for the README, and nothing anywhere reports it. See `docs/RELEASE.md` step 9.

| platform | asset name |
|---|---|
| macOS (Apple Silicon) | `Tangent-macos-arm64.dmg`, `Tangent-macos-arm64.zip` |
| Windows | `tangent-windows-x86_64.zip` |
| Linux | `tangent-linux-x86_64.tar.gz` |
| checksums | `SHA256SUMS` |

**Links, not uploaded copies.** Gumroad can host the binaries itself, and that
was the alternative. Links win because the buyer always gets the current
release: uploaded copies go stale the moment a version ships and have to be
re-uploaded by hand, which is exactly the step that gets forgotten. The cost is
that downloads depend on GitHub being reachable, which is the same dependency
the public README already has.

## Post-purchase page

Gumroad → Products → Tangent → **Content** tab → paste, then **Save changes**.

Gumroad's editor turns a bare URL into a link on paste, so this can go in as
plain text. Headings are optional; set them with the editor's own controls
rather than typing `#`, which it renders literally.

Preview it with **Preview** on the product page, not by buying: buying fires the
Ping and mints a real key.

---

Thank you for supporting Tangent.

Tangent is free and stays free. Your supporter key is a thank-you, not an unlock.
Nothing in the app is hidden behind it.

DOWNLOAD TANGENT

These links always give you the current version, so they are worth keeping.

Each installer offers the app and the VST3 plugin as separate choices, so you
can take either or both. The plugin goes where your DAW already looks.

macOS 11 or later, Apple Silicon or Intel
https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos.pkg

Windows 10 or later
https://github.com/ganten1998/ivory/releases/latest/download/Tangent-windows-setup.exe

Linux x86_64 - the tarball has an install.sh that needs no root
https://github.com/ganten1998/ivory/releases/latest/download/tangent-linux-x86_64.tar.gz

Rather unpack it yourself:
https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos-arm64.dmg
https://github.com/ganten1998/ivory/releases/latest/download/Tangent-macos-arm64.zip
https://github.com/ganten1998/ivory/releases/latest/download/tangent-windows-x86_64.zip

Checksums:
https://github.com/ganten1998/ivory/releases/latest/download/SHA256SUMS

THE PLUGIN

Tangent.vst3 shows everything the app does, reading the notes on the track it
is on. It makes no sound of its own - it is a monitor, not an instrument - so
put it on a MIDI or instrument track rather than in an effect slot. Its
settings live in the DAW project, so two instances and the standalone can each
be set up differently.

Your DAW will find it the next time it scans for plugins. It appears as
Tangent, by Ganten.

YOUR SUPPORTER KEY

The key is emailed to you within about a minute of your purchase, from
keys@ivorymidi.com. If it has not arrived, search your spam folder for that
address. If it is still missing, reply to your Gumroad receipt and I will send
it by hand.

To use it: open Tangent, right-click anywhere, choose "Support Tangent...", paste
the key and press Activate. Case, spaces, dashes and line breaks do not matter.

The key has no expiry, contacts no server, and works on every machine you own.

FIRST LAUNCH

macOS: run the .pkg and choose what you want installed. The app is signed with
a Developer ID certificate and notarized by Apple. You can install for all
users, which needs your password, or just for yourself, which does not.

Windows: run the setup and choose what you want installed. It is not
code-signed, so SmartScreen shows "Windows protected your PC" the first time.
Click "More info", then "Run anyway". Windows remembers the choice.

Linux: extract the archive and run ./install.sh. It needs no root by default,
and takes --app, --vst3, --system, --prefix, --uninstall and --dry-run. MIDI
goes through ALSA, so libasound.so.2 needs to be present. Requires glibc 2.32
or newer.

Plug your MIDI keyboard in before you start Tangent. Everything in the app
lives in the right-click menu, and holding H shows every keyboard shortcut.

Source code and release notes:
https://github.com/ganten1998/ivory

---

## Keeping the two in step

The email and the page say the same things about activation and first launch. If
you change one, change the other. The email version is
`email_body()` in `tools/ivory-fulfil/src/main.rs`; its tests assert the three
download links are present and unpinned, so deleting a link fails the build
rather than quietly shipping a page with nothing on it.

`cargo test -- --ignored --nocapture print_email` prints the email exactly as a
buyer receives it.
