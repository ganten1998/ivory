# Linux build-test findings, 4.11.0 / 4.11.1

**Source.** A test session on `dresden` (Void Linux x86_64, 2013 MacBook Air,
XFCE+i3, PipeWire 1.6.7), run against the shipped tarballs rather than a build
tree. Audio was verified by capturing the sink monitor and measuring peaks, not
by trusting the UI; stream geometry came from `pw-dump` and underruns from
stderr. Kept verbatim below.

**Status of each finding, as of this commit:**

| # | Finding | Status |
|---|---------|--------|
| 1 | Audio engine: rate, buffer, priority, honest failure | OPEN - belongs to the 5.0 audit |
| 2 | `tangent-ffmpeg` missing from the Linux tarball | FIXED - `build-cross.sh` packs it |
| 3 | The app lists its own display plugin as an instrument | FIXED - labelled, and the load error names the file |
| 4 | A dead saved slot silences the fresh default | NOT REPRODUCED - see below |
| 5 | Pre-restore state can be persisted | OPEN |
| 6 | Packaging and window warts | PARTLY FIXED - file modes; the rest open |

**On finding 4.** The report's narrative says a dead slot makes the app open
silent. The code does not do that, and the report's own measurement table says
"Built-in loads & sounds: PASS". From the renderer's side a failed load is a
slot with nothing in it, and `render_builtin` plays whenever no slot produced
audio. `a_slot_whose_plugin_failed_to_load_still_leaves_the_builtin_audible`
now pins that. What IS true is everything around it: the error was generic, and
the picker offered the dead plugin in the first place. Both are fixed.

---

Tines on Void

  - 

  

# Tines on Void
  
Build-test findings for Tangent 4.11.0 / 4.11.1 on Linux — the DX7 feature set, the plugin that wouldn't load, and where the audio breakup actually comes from.
  2026-08-19 · dresden — Void Linux x86_64, 2013 MacBook Air, XFCE+i3, PipeWire 1.6.7 · tested from ~/Downloads tarballs

  
  * DX7 + BANK: WORKS
  
  * FRESH INSTALL PLAYS
  
  * PICKER COLLISION
  
  * FFMPEG NOT PACKED
  
  * AUDIO ENGINE: FRAGILE

## The bug as reported

"nothing loaded and all i see is a Tangent instrument that doesn't load"

The DX7 was never broken. The instrument that refused to load is ~/.vst3/Tangent.vst3 — the Aug 18 dev build of Tangent's own DAW display plugin, which by its own design has no audio: AUDIO_IO_LAYOUTS is empty, process() touches nothing. The failure chain:

  
  - settings.json still had plugin_slots[0] = ~/.vst3/Tangent.vst3 from earlier plugin testing.
  
  - 4.11.0 restores the slot at startup and correctly rejects the plugin. The only signal is one line in the transport — easy to miss:

  
  
  [caption] The whole error UX for a dead startup slot. Slot 1 also reads "Tangent (did not load)" in small red text.

  
  - Because the slot is occupied (dead, but occupied), the fresh-install default — TINE ONE — never applies. The app opens silent.
  
  - The picker then lists the dead VST3 by filename, one row under the real thing:

  
  
  [caption] "Tangent" (selected, because it's the saved slot) is the audio-less display plugin. "Tangent DX7" two rows up is the instrument. Clicking the familiar name reproduces the same error and reads as "the plugin is broken."

Every machine that ever tested the DAW plugin has this file in ~/.vst3, so every dev/test box hits this exact greeting. A genuinely fresh machine does not — which is why it only showed up here.

## What checks out

verified on 4.11.1 unless noted; audio confirmed by capturing the sink monitor, not by trusting the UI

  
  |  | Check |  | Evidence
  
  |  | Fresh install plays TINE ONE | PASS
       | Virgin IVORY_SETTINGS_PATH → slot 1 auto-loads the built-in, zero config, <5s.
  
  |  | Built-in loads & sounds | PASS
       | Held key measured −19 dBFS on the sink monitor (4.11.0 and 4.11.1).
  
  |  | Bank ships compiled-in | PASS
       | Picker shows "Tangent Bank 01 – Tines and Boxes" + E.PIANO 1, 32 patches, Edit… / Load .syx… present.
  
  |  | Patch changing | PASS
       | Hand-tested; dx7_patch 0→24 persisted across quit/relaunch.
  
  |  | Slot persistence (@tangent-dx7) | PASS
       | Sentinel restores before the welcome dialog is even dismissed.
  
  |  | Third-party VST3 hosting | PASS
       | Pianoteq 9 loads and registers; chipsynth-OPS7 enumerates.
  
  |  | Editor deep pass, Save-to-my-bank, takes, video | NOT RUN
       | Video is blocked by finding 2 on any clean box anyway.

  
  
  [caption] The headline claim, passing: a never-configured start with TINE ONE in slot 1.

## The breakup, diagnosed

it is not the sample rate, and it is not the DX7's DSP

On Linux the output stream opens through cpal→ALSA→PipeWire at a hard-coded 44100 Hz with 256-frame periods (5.8 ms), and the render thread cpal_alsa_out runs at normal priority — no realtime scheduling — while PipeWire's own data loop sits at FIFO 83 right next to it. Measurements, all on the same box:

  
  |  | Condition | Underruns
  
  |  | Idle 30s + held note 10s, quiet system | 0
  
  |  | Held note 15s under 3 CPU-hog processes | 0
  
  |  | Session with plugin warm-up, viewport churn, heavy X11 traffic | 1277
  
  |  | Ordinary interactive session (patch auditioning) | 42
  
  |  | Graph forced to 1024-frame quantum against the app's fixed 256-frame ring, idle | 341

Raw CPU load does not cause dropouts — the scheduler still feeds the audio thread. The underruns track in-process and display-stack activity: plugin warm-up, GL/viewport churn, heavy UI moments. That pattern says the audio callback is waiting on something the UI holds (the plugin build's own doc comment names this exact hazard: "the editor holds its state behind a mutex for a whole frame and the audio thread must never wait on that"). Video-encoding takes stack the same starvation on top of real CPU saturation — which is where the user-visible breakup lives.

Because 44100/256 is fixed, the system has been bent around the app: the graph rate ritual in pipewire.conf.d every session. Even with the whole graph natively at 48 kHz, Tangent still opens 44100 and gets resampled — the rate cannot be fixed from outside the app. Forcing bigger periods from outside (PIPEWIRE_ALSA='{ alsa.period-bytes = 8192 … }') works for any other ALSA client (verified with aplay: 1024/48000) but makes Tangent's stream open fail — cpal asks for exact period sizes — and the app then launches silently, with no error anywhere: not in the log, not in the UI.

## Fixes for next build

ranked; 1 and 2 gate the "plays before anybody configures anything" promise

  finding 1 · audio engine
  

### Own the stream: rate, buffer, priority, and honest failure
  
Four parts, one contract. (a) Persist the sample rate (and honor the device default when unset) instead of hard-coding 44100 — this is what removes the "reconfigure my whole audio system per session" ritual; the take's WAV should follow it too. (b) Plumb a buffer setting into the live output stream — record_buffer_frames exists, persists, and currently does nothing to the 256-frame stream. (c) Request realtime for the render thread via rtkit (the audio_thread_priority crate does exactly this); rtkit demonstrably grants FIFO on this box. (d) Open with set_period_size_near semantics and surface a failed stream open in the UI — today it's silence with an empty log. Separately: audit any lock shared between the egui frame and the audio callback; the underrun pattern points there, not at the DSP.

  finding 2 · packaging
  

### tangent-ffmpeg is not in the Linux tarball
  
Both 4.11.0 and 4.11.1 ship ~10 MB without the encoder; the 4.4.1 tarball built by the Linux script was 45 MB with it. The 4.11.x tarballs are being packed on the Mac (Apple provenance + Dropbox xattrs; 4.11.0 even carried ._ AppleDouble litter — 4.11.1 cleaned that). Video export works on dev boxes that happen to have distro ffmpeg and breaks on clean machines. Pack Linux artifacts with the Linux packaging script, or add the pinned static ffmpeg to the Mac packing path. Same check applies to the aarch64 tarball.

  finding 3 · picker
  

### The app lists its own display plugin as a loadable instrument
  
The host knows its own VST3's identity. Filter it from the instrument picker, or label it ("display only — loads in a DAW, not here") instead of letting it fail after selection. The load error should also say which file failed and point at the built-in: "you probably want Tangent DX7."

  finding 4 · defaults
  

### A dead saved slot silences the fresh-default
  
When slot 1 fails to restore at startup, fall back to @tangent-dx7 (keeping the error visible) rather than opening silent. Every box that ever tested the DAW plugin — i.e. every test machine — currently fails the out-of-box experience this way.

  finding 5 · settings
  

### Pre-restore state can be persisted
  
Observed once: a settings write ~30s after launch recorded plugin_slots as all-null while the slot restore hadn't landed yet; a later write restored the sentinel. Kill the app in that window and the selection is silently gone. Don't serialize slots until restore has resolved.

  finding 6 · hygiene
  

### Small packaging and window warts
  
tangent.png and fonts/*.ttf ship mode 0600 — unreadable by other users after a root install. The tarball still carries mac/Dropbox xattr pax headers (harmless, noisy under GNU tar). Under i3 the main window sometimes maps partially off-screen (x = −554 on a 1440-wide display); viewport children (picker, patch window, welcome) behave.

## State of this box after the session

what changed on dresden, and how to undo it

  
  |  | Item | State
  
  |  | settings.json | slot 1 = @tangent-dx7, patch 25, record_buffer_frames 1024 (currently inert, see finding 1b). This is the fixed, desired state.
  
  |  | pipewire.conf.d/10-audio.conf | A/B-tested both rates, settled on 44100 / quantum 512 — matches Tangent's fixed 44.1k stream (zero resampling) and the Qtractor sessions. Verified: native 256/44100 stream, sound playing, 0 underruns. The stale "48000/1024" comment that drove the per-session flipping is rewritten; the ritual is obsolete — rate was never the lever.
  
  |  | ~/.vst3/Tangent.vst3 | Left in place (Aug 18, 4.4.1-era). Delete or rebuild it when the 4.11 plugin ships; until finding 3 lands it will keep appearing as a dead "Tangent" row in the picker.
  
  |  | ~/tangent-build | Still at 4.4.1 — the 4.11.x source isn't on this machine. Sync it over if Linux-side debugging is expected again.
  
  |  | Interim RT test | Until finding 1c, RT can be granted per-run: sudo chrt -f -p 70 $(ps -Lo tid=,comm= -p $(pgrep -x tangent) | awk '$2=="cpal_alsa_out"{print $1}') — worth an A/B during a video take.

Method: shipped binaries driven on the live X session; audio verified by capturing the sink monitor and measuring peaks; stream geometry from pw-dump; underruns counted from stderr logs; slot behavior cross-checked against settings.json writes; fresh-install simulated via IVORY_SETTINGS_PATH. Sources of truth on-box: the 4.4.1 tree for host/plugin internals, strings(1) on the 4.11.x binaries for the rest.