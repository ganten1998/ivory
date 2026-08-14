Tangent
=====

Tangent draws all 88 piano keys and lights them from your MIDI keyboard in real
time. As you play, it names what you are playing — triads and sevenths through
altered dominants, slash chords, rootless jazz voicings, scales and modes.

Everything is in the right-click menu. That is the entire interface.


First launch
------------

macOS

  Open the .dmg (or unzip the .zip) and drag Tangent.app into your Applications
  folder.

  That is all. Tangent is signed with a Developer ID certificate and notarized by
  Apple, with the ticket stapled to the app, so it opens on a double-click with
  no security prompt and without being online.

  Requires macOS 11 (Big Sur) or later, on Apple Silicon.

Windows

  Unzip the archive anywhere and run tangent.exe. There is no installer.

  The executable is not code-signed, so SmartScreen shows "Windows protected
  your PC" the first time. Click "More info", then "Run anyway". Windows
  remembers the choice.

Linux

    tar -xzf tangent-<version>-linux-<arch>.tar.gz
    cd tangent-<version>-linux-<arch>
    chmod +x tangent
    ./tangent

  MIDI goes through ALSA, so libasound.so.2 needs to be present — package
  libasound2 on Debian/Ubuntu, alsa-lib elsewhere. It already is on essentially
  every desktop install. PipeWire systems work unchanged: your keyboard still
  shows up as an ordinary ALSA sequencer port. Wayland and X11 are both
  supported. Requires glibc 2.32 or newer.

  Your keyboard has to be plugged in and switched on before Tangent can see it —
  ALSA only creates a port for hardware that is actually there.

  For a launcher entry:

    install -Dm755 tangent         ~/.local/bin/tangent
    install -Dm644 tangent.desktop ~/.local/share/applications/tangent.desktop
    install -Dm644 tangent.png     ~/.local/share/icons/hicolor/128x128/apps/tangent.png

  The fonts in fonts/ are already embedded in the binary. They ship as loose
  files for license compliance, and in case you want them installed system-wide.


Connecting a MIDI keyboard
--------------------------

Plug it in, start Tangent, play. Tangent connects on its own at startup, preferring
ports whose names contain "USB-MIDI", then "Scarlett" or both "USB" and "MIDI",
and otherwise simply taking the first port it finds.

If you connected after launching, or you have more than one device:
right-click > "Select MIDI Input...", choose the port, click OK. Tangent does not
reconnect by itself if a device is unplugged mid-session — plug it back in and
pick it from that list again.

From a terminal:

    tangent -l                   list the available MIDI input ports
    tangent -p "Digital Piano"   connect to one specific port

(on macOS that binary lives at /Applications/Tangent.app/Contents/MacOS/tangent)

No keyboard to hand? Right-click > "Enable Keytoggle", then click keys on the
drawn piano to hold and release notes. Chord naming behaves exactly the same.


When Tangent names a chord differently than you would
---------------------------------------------------

Two menu items, and they do different things.

"Teach Chord Name..." pins your name to the exact voicing you are holding. Type
it, click OK, and that is what Tangent calls it from then on. Tick "Apply in all
keys" and the name follows that shape into every key. It is greyed out when you
are not holding anything, because it acts on what you are playing.

"Correct Chord Name..." teaches a general preference instead. It lists the
readings Tangent actually weighed for this voicing, with their scores; pick the
one you would rather see and click "Learn". Tangent then leans that way
everywhere, so similar voicings shift too — about one chord in ten, sometimes
in unrelated keys. That is the feature working, not a fault. If the name you
want is not in the list, Tangent never considered it and cannot be argued into
it — pin it with "Teach Chord Name..." instead. The first correction that lands
switches Chord Learning on, which also reactivates any earlier corrections.

"Manage Taught Chords..." lists everything you have pinned, with a Delete button
beside each, shows whether learning is on and what it has picked up, and offers
"Forget Learning" — which restores stock chord naming exactly. "Disable Chord
Learning" in the menu silences learning without erasing anything.


Settings
--------

Colors, dark mode, window size (50%–200%), borderless mode, flats versus sharps,
and the detachable chord display all live in the right-click menu, and save
themselves the moment you change them. "Reset Settings to Default" puts
everything back.

Tangent keeps two files, at the same path on every platform:

    ~/.config/ivory/settings.json    appearance and window preferences
    ~/.config/ivory/overrides.json   taught chords and learned leanings

On Windows that folder is C:\Users\<you>\.config/ivory\. Deleting one of these
files resets that part of Tangent and nothing else.


Privacy
-------

Tangent makes no network connections and collects no data. Your settings and your
taught chords stay on your machine. There is no telemetry, no update check, and
no account. The one link in the app — the author's site, in the About box —
hands the address to your browser if you click it, and that is the whole of it.


If something looks wrong
------------------------

  "Tangent is already running"
      Only one copy runs at a time. Close the other window; if there is no
      window, quit the leftover process.

  No chord names appear
      Chord detection may be switched off. Right-click > "Enable Chord
      Detection".

  Keys never light up
      Tangent is probably on the wrong port. Right-click > "Select MIDI
      Input..." and pick your keyboard.


Licenses
--------

Tangent is MIT-licensed — see LICENSE. The Rust libraries it links against are
listed in THIRD-PARTY-LICENSES. The bundled Courier Prime fonts are
Copyright 2015 The Courier Prime Project Authors, licensed under the SIL Open
Font License 1.1 — see OFL.txt.

  macOS     inside the app: right-click Tangent.app > Show Package Contents >
            Contents/Resources
  Windows   next to tangent.exe
  Linux     in this folder; OFL.txt is in fonts/

THANKS

  Omer, for the rounded macOS app icon.
  Hatsu, for extensive Windows testing and feedback.

Tangent is pay-what-you-can, and nothing is a perfectly good amount. Thanks for
playing.
