# Tangent Recorder: the implementation plan

An all-in-one piano recording view. Press one button; get a directory holding a
video with the performance audio correctly synced, the audio on its own, and the
MIDI on its own. Camera preview lives in the window. The standalone can host
instrument plugins so the sound can come from the user's own piano VST.

Everything below was checked against the tree, against the dependency sources on
this machine, and against primary licence documents. Where a premise in the
brief turned out to be wrong, the correction is stated inline so nobody
re-derives it. Read §0 before anything else — two of its rows change what is
buildable.

**This document has been through one adversarial review** (five reviewers, forty
refutation agents, every finding attacked by a skeptic told to refute it). 32
findings survived and are folded in below; 8 were refuted and are recorded in
§15 so they are not re-raised. Sections rewritten as a result are marked. One
finding was about the repo rather than this feature and is **already a shipped
defect**: see §10 item 6.

---

## 0. Verified facts that decide the design

| Claim | Verdict |
|---|---|
| Hosting VST3 forces an MIT app to become GPLv3 | **FALSE, as of 2025-10-29.** Steinberg relicensed the VST3 SDK to **MIT** with VST 3.8. `steinbergmedia/vst3sdk/LICENSE.txt` is plain MIT, "Copyright (c) 2026, Steinberg Media Technologies GmbH"; the GitHub API reports `spdx_id: MIT`; the README states "Licensing under GPLv3 and the Steinberg proprietary license is no longer available." Verified directly, not via an agent. `plugin/` stays GPLv3 anyway, but for a different reason: the *Rust crate* `vst3-sys` declares `license = "GPL-3.0"`, a choice its authors made when the SDK forced it. **`LICENSING.md` was corrected in the same session that produced this plan** — it now states the crate-not-SDK cause and carries a dated correction block. No further edit is owed. |
| There is a maintained Rust VST3 **host** library | Effectively no. `vst3` 0.3.0 (MIT OR Apache-2.0, 2025-12-07, 45,557 downloads) is raw bindings for **both** sides and ships a pre-generated 670 KB `bindings.rs`, so there is no build-time `libclang` and cross-compilation survives. `vst3-host` 0.9.0 (MIT, first released 2026-06-20) is the only host library and is **one author, eight weeks old**; its egui feature requires egui 0.34 and is unusable under the 0.33 pin. |
| CLAP hosting is the cheap escape hatch | True technically, useless practically. `clack-host` 0.1.1 (MIT OR Apache-2.0, 2026-07-29) is feature-complete, and CLAP's `gui.create(..., is_floating: true)` means **zero NSView/HWND/XEmbed code**. But there are almost no free CLAP pianos; the good ones ship VST3/AU. CLAP is a cheap *second* format, never the first. |
| MIDI timestamps are available today | **They are received and thrown away.** `ivory/src/midi.rs:65` names midir's stamp `_stamp`. Discarding it is the single most expensive line in the repo for this feature. **Unit, corrected by review:** midir's callback stamp is **MICROSECONDS on every backend** — its public contract says so (`src/common.rs:101-104`, `src/lib.rs:42-44`), and the CoreMIDI backend is literally `AudioConvertHostTimeToNanos(timestamp) as u64 / 1000` (`src/backend/coremidi/mod.rs:104-105`). The mach *domain* is shared with CoreAudio and `Instant`; the *scale* is not. Every conversion needs `× 1000`, expressed as the anchor's `a` coefficient (§3), never as an identity. |
| Camera and audio timestamps need no latency term | **FALSE, and this is the biggest hole the review found.** Sharing a clock *domain* is not sharing an *origin*. Apple's `AVCaptureSession.h` guarantees only that sample timestamps are on the master clock **timebase**; AVFoundation exposes no capture-latency query, so a UVC webcam's sensor→ISP→USB→host delay is uncompensated and audio leads video by it. cpal 0.18.1's CoreAudio input does subtract `kAudioDevicePropertyLatency + kAudioDevicePropertySafetyOffset` (`macos/device.rs:781-785`) but each is `.unwrap_or(0)` and `kAudioStreamPropertyLatency` appears nowhere, so it is an estimate with a silent zero fallback. §3a is the section that did not exist before review. |
| `MFSampleExtension_DeviceTimestamp` is QPC-comparable as-is | Domain yes, unit no. Microsoft's page reads "in the MFTIME domain, sharing an epoch with query performance counter (QPC) time and **always expressed in 100ns units**." The Windows camera anchor needs `× 100`. Same error class as the midir row above. |
| `MidiEvent` can be widened to carry a recording | **No — do not try.** `ivory-ui/src/midi_event.rs:14-19` is three variants by design: channels merged at `:26`, only CC64 survives at `:35`, and its own test asserts pitch bend returns `None` at `:74`. That is exactly right for the chord engine. The recorder must **tee the raw `&[u8]` before `parse_message` runs**, not extend the shared type. |
| midir ignores nothing by default | True. `ivory/src/midi.rs:54` never calls `.ignore()`, and midir's default is `Ignore::None` (`src/common.rs:68-69`). A keyboard sending Active Sensing every ~300 ms bloats a `.mid` roughly 10×. `input.ignore(Ignore::TimeAndActiveSense)` fixes it and keeps SysEx. |
| The app can already put a background thread on screen | Yes, and it is the pattern to copy: `ivory/src/midi.rs:68` calls `ctx.request_repaint_of(ViewportId::ROOT)` from the midir callback thread with a cloned `egui::Context`. There is otherwise **exactly one** background thread in the whole tree and no `std::thread::spawn` anywhere. |
| The app has ever uploaded a texture | **Never.** Zero occurrences of `load_texture`, `TextureHandle`, `ColorImage`, `ImageData`, `PaintCallback` across all four crates. The app draws with `rect_filled` ×18, `text` ×14, `line_segment` ×11 and three `convex_polygon`s. The camera preview is the first image the app has ever drawn. |
| Every panel is a pure painter | **Yes, and this is the fact the compositor is built on.** `piano::draw`, `chord_strip::draw`, `fretboard_panel::draw` and `theory_panel::draw` all take `&egui::Painter` + `Rect` and paint absolutely. **None of them allocates a widget.** So the same code can paint the 88-key piano into a 1920×1080 offscreen surface at video resolution with no changes at all. §6. |
| The layout can absorb a 16:9 band | **No.** `band_sizes_at` (`app.rs:1724-1745`) makes every height a fixed fraction of the width. Full-width 16:9 at w=1300 is **731 pt tall**, larger than the entire app at maximum (632). Nothing clamps total height against the monitor; the window is `with_resizable(false)` (`main.rs:214`). The preview must be letterboxed into a width-proportional band via `Painter::image`'s `uv` argument, so the camera's aspect never reaches the window. |
| `Caps` has a field that covers recording | No. `persist_global_settings` is about `~/.config/ivory/settings.json` specifically; conflating it with "may write a 2 GB video into Movies" is exactly the mistake `host.rs:13-16` warns against. New fields needed. Blast radius is **2 consts plus 1 test** — there are no other `Caps` literals in the tree. |
| `rfd` can be called from `ivory-ui` | No, and the firewall says so. Use the `take_pending_*` request pattern that `pending_resize` already establishes (`app.rs:150-152, 383-386`), not a blocking trait call. *Corrected by review:* only `main.rs:188` and `:252` are outside the event loop — `:138` is inside the **panic hook**, so it fires wherever the panic does, overwhelmingly inside `IvoryApp::frame`. And the request pattern alone does not move a modal out of the egui pass, because eframe calls `App::update` from inside `egui_ctx.run`. See the §14 row. |
| The macOS build can access a camera | **No, and it fails silently.** `scripts/build-macos.sh:217-218` signs with `--options runtime` and **passes no `--entitlements` at all** — `grep -rn "entitlement"` over the repo returns zero hits, and `codesign -d --entitlements -` on the shipped 3.0.0 bundle prints no entitlements dictionary. Without `com.apple.security.device.camera` the request is refused with **no prompt and no System Settings row**. Separately, a missing `NSCameraUsageDescription` is not a denial — TCC **kills the process**. |
| `cargo run` is a valid camera test on macOS | **No.** TCC attributes a child process's request to the responsible ancestor, so `cargo run` from Ghostty is checked against *Ghostty's* Info.plist and *Ghostty's* grant. The dev loop must be build-the-`.app`, sign with entitlements, `open` it. Budget for this. |
| Raw frames now, encode at stop | **Not a design, a bug.** One 20-minute 1080p30 take is **112 GB** as NV12 and needs 89 MiB/s sustained. Encode-while-recording is the only viable shape: ~1.5 MB/s to disk and the file is finished when Stop is pressed. |
| Camera timestamps can be trusted to share the audio's clock | **No, verify per platform.** `nokhwa::Buffer::capture_timestamp()` is `Option<Duration>` since **UNIX_EPOCH** — it takes a perfectly good monotonic mach PTS and re-bases it onto a clock NTP can step. Its Linux backend populates nothing at all. On macOS the fix is to not bridge clocks: take `CMSampleBuffer` PTS, which is already host time. |
| nokhwa is safe to point at the owner's own webcam | **No.** Issue #247 (open, PRs #246/#248/#249 unmerged) aborts the process — a non-unwinding panic, uncatchable — on cameras exposing several formats at one resolution, which the reporter describes as "most Logitech UVC webcams". The owner's camera is an **MX Brio**. |
| Current artifact size | 11–21 MB (`dist/`: `Tangent-3.0.0-macos-universal.zip` 11 MB, `.dmg` 12 MB, `.pkg` 21 MB, Windows setup 5.2 MB). A bundled ffmpeg is 30–70 MB **per architecture**, doubled for a universal macOS binary. |
| The Homebrew ffmpeg on this machine could be shipped | **No.** `ffmpeg 8.1.2` here is configured `--enable-gpl --enable-version3` with x264 and x265. That binary is GPL. |

Two consequences to read before the rest:

1. **The licence problem that shaped this whole project's plugin story is gone.** VST3 hosting is now an MIT-compatible thing to do. What remains is a *scope* problem, and it is a large one.
2. **The macOS signing change is not optional and it is not cosmetic.** Camera and microphone under a hardened runtime need an entitlements file that does not exist yet, and the failure mode without it is an empty device list with no error anywhere.

---

## 1. What the feature is

A **take** is one press of Record to one press of Stop. It produces exactly one
new directory, and that directory is the deliverable. Nothing is ever written
outside it, and no take ever overwrites another.

```
~/Movies/Tangent/2026-08-15_143207_nocturne/
    2026-08-15_143207_nocturne.wav        48 kHz/24-bit stereo, BWF `bext`
    2026-08-15_143207_nocturne.mid        SMF format 1, 960 PPQ, tick 0 == take start
    2026-08-15_143207_nocturne.mp4        the composite: camera + Tangent's display + audio
    2026-08-15_143207_nocturne-camera.mp4 optional, camera alone + audio
    2026-08-15_143207_nocturne-display.mp4 optional, Tangent's display alone + audio
    #  ^ .mp4/H.264/AAC on macOS and Windows; .mov/H.264/LPCM on Linux, which has
    #    no permissively-licensed AAC encoder. See §7's container table.
    take.json                             machine-readable manifest and sync report
    take.log                              plain-text event log for support
```

**The contract, stated once and asserted in tests:** WAV sample 0, MIDI tick 0
and video frame 0 are the same instant. `take.json` says so in a field
(`"zero_is": "take_start"`) because every other tool trims leading silence and
then the MIDI does not line up.

**Which files get made is a per-take choice** made in an Export dialog with a
composition selector, defaulting to composite + WAV + MIDI. The video variants
are independent encodes off one shared frame source; selecting all three costs
three encoders, which VideoToolbox and Media Foundation absorb and a software
encoder does not. The dialog says so.

**Audio comes from one of three sources**, chosen in the Recorder band:
1. an audio **input device** — the line-out of a digital piano, a mic, an interface;
2. a **hosted instrument plugin** rendering the incoming MIDI;
3. **both, mixed**, with independent gain.

Source 1 ships first and is the one that needs no configuration. Source 2 is the
plugin-hosting workstream and is deliberately last (§8).

> **Say this out loud rather than bury it.** The owner chose "host any
> instrument plugin", and that is what §8 plans. But it is the largest single
> piece of work in this document by a wide margin — a minimal-but-correct VST3
> host is 6,000–12,000 lines and three separate native child-window
> implementations, because `IPlugView` has no floating-window mode. **Every
> other part of this feature works without it**, using the audio input the owner
> also chose, and a piano's line-out is in most cases a better recording than a
> plugin anyway. So hosting is scheduled last not to demote it but so that a
> working recorder is not held hostage to it. The honest consequence, stated
> plainly: **the owner's own VST3 pianos will not make sound inside Tangent
> until step 9**, and steps 1–8 are worth shipping before then.

---

## 2. Crate layout

**Two new crates, both MIT, both in the root workspace, neither reachable from
`ivory-ui`.** No new quarantined workspace is needed: every dependency named in
this plan is MIT, Apache-2.0, Zlib, BSD-2/3 or 0BSD. The GPL story stays exactly
what `LICENSING.md` describes — one quarantined `plugin/` — and nothing here
touches it.

```toml
[workspace]
members  = ["ivory-core", "ivory-ui", "ivory", "ivory-record", "ivory-host"]
exclude  = ["plugin"]
resolver = "2"
```

### `ivory-record` — capture, clock, encode, mux, files

Depended on **only** by `ivory`. It owns everything platform and everything
real-time:

| module | what |
|---|---|
| `clock.rs` | the timebase (§3). Pure arithmetic, fully testable headless. |
| `audio.rs` | `cpal` input + output streams, ring buffer, level metering |
| `wav.rs` | hand-rolled RIFF writer with a BWF `bext` chunk and periodic size-field patching (§10) |
| `camera/` | `macos.rs` (objc2/AVFoundation), `windows.rs` (Media Foundation), `linux.rs` (`linuxvideo`), behind one `VideoSource` trait |
| `video/` | `macos.rs` (AVAssetWriter), `windows.rs` (IMFSinkWriter), `linux.rs` (openh264 + muxer), behind one `VideoSink` trait |
| `smf.rs` | `midly` writing (§7) |
| `take.rs` | directory naming, sanitisation, atomic creation, `take.json`, `take.log` |
| `session.rs` | the state machine that ties them together |

Dependencies, with licences, all verified:

```toml
cpal        = "0.18"     # Apache-2.0        input + output audio
rtrb        = "0.3"      # MIT OR Apache-2.0 lock-free SPSC, off the audio callback
midly       = { version = "0.5", default-features = false, features = ["std"] }
                         # Unlicense; ZERO transitive deps with parallel off
yuv         = "0.8"      # BSD-3 OR Apache-2.0  SIMD colour conversion
dirs        = "6"        # already in the tree

[target.'cfg(target_os = "macos")'.dependencies]
objc2                 = { version = "0.6", features = ["exception"] }  # MIT
objc2-foundation      = "0.3"   # MIT
# The features line is MANDATORY and is the difference between this compiling
# and not. In objc2-av-foundation 0.3.2 the `objc2-core-media` feature is NOT in
# the 167-entry `default` set, and EVERY method taking a CMTime or a
# CMSampleBuffer is gated on it: `AVAssetWriter::startSessionAtSourceTime`
# (generated/AVAssetWriter.rs:334/348), `AVAssetWriterInput::appendSampleBuffer`
# (:289/:324), `AVAssetWriterInputPixelBufferAdaptor::appendPixelBuffer_
# withPresentationTime` (AVAssetWriterInput.rs:886/909, which also needs
# objc2-core-video), and the capture delegate's
# `captureOutput_didOutputSampleBuffer_fromConnection`
# (AVCaptureVideoDataOutput.rs:383-387). Without it, steps 5 and 6 fail with
# four E0599 "no method named" errors on their first lines.
#
# This is a trap with an active decoy: `[package.metadata.docs.rs]`
# (Cargo.toml:56-59) adds `objc2-core-media`, so docs.rs renders all of these
# methods as if they were available by default. Checking the API against
# published docs reproduces the bug rather than catching it. Listing
# objc2-core-media as a sibling dependency does NOT enable another crate's
# feature.
objc2-av-foundation   = { version = "0.3", features = ["objc2-core-media"] }
                                # Zlib OR Apache-2.0 OR MIT
objc2-core-media      = "0.3"
objc2-core-video      = "0.3"
objc2-core-foundation = "0.3"
block2                = "0.6"   # MIT
dispatch2             = "0.3"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.62", features = [                # MIT OR Apache-2.0
    "Win32_Media_MediaFoundation", "Win32_System_Com", "Win32_Foundation" ] }

[target.'cfg(target_os = "linux")'.dependencies]
linuxvideo = "0.3"   # 0BSD, pure Rust, no bindgen, no libclang
# `default = ["source"]` on both openh264 0.9.8 and openh264-sys2 0.9.8: build.rs
# compiles the bundled 6.3 MB Cisco C++ tree with `cc` + nasm and STATICALLY
# LINKS it into `tangent`. That is a deliberate choice with two consequences
# spelled out in §7: Cisco's MPEG LA royalty grant does NOT travel with the
# source (it attaches only to Cisco's own separately-downloaded binary module),
# and nasm is not optional in practice — see §10 item 7 for why its absence is
# silent and ~3x slower rather than an error.
openh264   = "0.9"   # BSD-2 (copyright only; no patent grant)
```

**Why `objc2-av-foundation` and not a camera library.** It carries
`AVCaptureSession` *and* `AVAssetWriter`, `AVAssetWriterInput` and
`AVAssetWriterInputPixelBufferAdaptor`. On the owner's own platform, one
dependency set gives camera capture, hardware H.264 encoding, and a correctly
muxed `.mov`/`.mp4` with Apple doing the A/V sync. Any library that hands you
only frames leaves encode and mux to be solved separately anyway.

**Why the `windows` crate and not a wrapper.** `windows-link` expands to
`#[link(kind = "raw-dylib")]`, so there are no import `.lib` files and no C
toolchain — `cargo xwin` from macOS keeps working by construction. This is the
single best-behaved option under the existing cross-build.

**Why not `nokhwa`.** It aborts the process on the owner's own MX Brio (§0), its
macOS backend mislabels three planar 4:2:0 formats as packed YUYV and reads
`CVPixelBufferGetBaseAddress` on planar buffers, its `frame()` blocks with no
timeout and `drain()`s newer frames so it *structurally cannot* deliver every
frame to a recording, and it cannot see Continuity Camera. It remains a
reasonable behind-the-trait fallback for Windows and Linux if those backends
slip; it is not acceptable on macOS. If taken, it must be
`default-features = false, features = ["input-native"]` — the default drags
`mozjpeg-sys`, which needs `nasm` and a C compiler on **every** target and ends
the Windows cross-build.

### `ivory-host` — the plugin host

Separate from `ivory-record` on purpose. It is the largest risk in the feature,
it is the only part that loads foreign code into the process, and keeping it a
separate crate means it can be cut, feature-gated, or moved out of process
without touching the recorder. Depended on only by `ivory`.

```toml
vst3  = "0.3"      # MIT OR Apache-2.0, pre-generated bindings, no libclang
clack-host       = "0.1"   # MIT OR Apache-2.0, phase 2
# clack-extensions has NO `default` feature and gates all 27 extension modules
# individually (src/lib.rs:5-57), so the bare `"0.1"` in the first draft of this
# plan compiled to an EMPTY crate. Two of the names are non-obvious:
#   * `clack-host` is an implicit feature from an optional dependency, and it is
#     what generates the HOST side. `gui.rs:69-72` gates `mod host` / `pub use
#     host::*` on it, and that module is where `PluginGui`, `PluginGui::create`
#     (host.rs:61) and `set_transient` (host.rs:241) live. Enabling `gui` alone
#     leaves the host half absent.
#   * The raw-window-handle bridge is spelled with HYPHENS and a version suffix.
clack-extensions = { version = "0.1", features = [
    "clack-host", "gui", "state", "params", "note-ports", "audio-ports",
    "timer", "raw-window-handle_06" ] }
raw-window-handle = "0.6"  # already resolved via eframe 0.33.3
walkdir = "2"
```

`raw-window-handle 0.6.2` is **already in the root `Cargo.lock`** because
eframe 0.33.3 depends on it, and `eframe::Frame` implements `HasWindowHandle`
(`epi.rs:677-680`). So handing a plugin the host window costs no version
bridging and no pin violation.

### What does not change

`ivory-core` is untouched. `ivory-ui` gains modules and traits but **no new
dependencies**, so `scripts/check-firewall.sh` stays green without editing it.
Add `cpal`, `nokhwa`, `vst3` and `objc2` to that script's forbidden list anyway
— the list is a literal allowlist of crate names, so a camera crate added to
`ivory-ui` would pass it today. That hole should be closed in the same commit
that opens the possibility.

---

## 3. The clock, which is the whole feature

Every complaint about every tool surveyed reduces to sync. OBS users report
drift growing "from 300 ms to 700 ms to over a full second during 35-minute
streams". SeeMusic, the only product that does this whole job, is reported to
lose sync on long pieces. Getting this right is the feature.

**Two roles, and conflating them is the classic failure.**

**Role 1 — the timebase is `std::time::Instant`, in nanoseconds.** Every event
from every source is converted into it exactly once, at the boundary where it
enters Tangent. Nothing downstream ever sees a device timestamp again.

On macOS the four sources share a clock **domain**, which is a real and useful
head start: `Instant` is `CLOCK_UPTIME_RAW`, which std's own source says is
"identical to the result of `mach_absolute_time()` after the appropriate mach
timebase conversion"; cpal's CoreAudio `StreamInstant` is `mHostTime` × the same
timebase; midir's CoreMIDI stamp derives from
`AudioConvertHostTimeToNanos(...)`; AVFoundation's `CMSampleBuffer` PTS is on
`CMClockGetHostTimeClock()`. Windows shares QPC across WASAPI and Media
Foundation; Linux shares `CLOCK_MONOTONIC` across ALSA and V4L2 (check
`V4L2_BUF_FLAG_TSTAMP_SRC_MASK`; a driver may use CLOCK_REALTIME).

> **Corrected by review — this paragraph used to end "the conversion is the
> identity and the sync problem is structurally solved". Both halves were
> wrong and the sentence was actively dangerous**, because §3's anchoring rule
> below is scoped to "a source whose stamp is not already host-domain", so that
> claim licensed skipping the anchor on exactly the platform the owner uses.
> 1. **The scale is not 1.** midir hands you **microseconds**
>    (`coremidi/mod.rs:104-105` divides the nanos by 1000; the µs contract is
>    documented for every backend at `common.rs:101-104`). The conversion is
>    `× 1000`.
> 2. **A shared domain is not a shared origin.** Every source has a device
>    latency, and no clock arithmetic removes it. See §3a.
>
> **There is no source that skips the anchor.** macOS gets a known scale
> coefficient and a still-unknown offset, which is a better starting position
> than the other platforms and is not the same thing as being solved.

Never `SystemTime`: NTP can step it mid-take. This is not hypothetical — it is
precisely what nokhwa does to a good monotonic PTS.

**Role 2 — the rate master is the captured audio sample count.** The exported
WAV is N samples long declaring rate `R_nominal`; nothing can change that,
because you wrote N samples. So the exported timeline is `t(n) = n / R_nominal`
and everything else maps onto *that*. Fit `T = α·n + β` incrementally from each
input callback's `(first sample index, StreamInstant)`. Then

```
file_t(T) = (T − β) / (α · R_nominal) = (T − β) · (1 + ε)
```

That `(1 + ε)` is the entire drift correction and it is one multiply. Skipping
it costs, at a typical ±50 ppm crystal, **60 ms at the end of a 20-minute
take** — monotone drift, which is exactly what a listener hears and exactly
what every forum thread in §0's prior art is complaining about. Correct the
timestamps, never resample the audio: resampling changes the samples, costs
CPU, and buys nothing for a single-device recorder.

Audio is the rate master rather than video or MIDI because it is the only
stream that cannot be re-timed without audible artefacts, the only one that is
rigidly rate-locked, and because USB webcams routinely deliver jittery or plainly
wrong timestamps and drop frames silently.

**Anchoring a source whose stamp is not already host-domain**: compute
`offset_i = Instant::now() − a·stamp_i` on every callback and take the **running
minimum** over the first ~2 s. Latency only ever adds, so the smallest observed
offset is the least-delayed observation. A single-sample anchor on the first
callback is biased by one full callback of scheduling latency.

**Windows has a live hazard here:** Rust's `Instant` is QPC **plus
`u64::MAX / 4` nanoseconds** (`library/std/src/sys/time/windows.rs:41-46`) while
cpal's WASAPI `StreamInstant` is raw QPC nanos. Code comparing them directly
would have worked before that offset was added and would have broken silently on
a Rust upgrade. Anchors make it a non-issue; direct comparison makes it a bomb.

### 3a. Device latency — the term the first draft did not have

**Added after review, which called this the biggest hole in the design.** A
running-minimum anchor removes *jitter*. By construction it cannot remove a
*constant*. Every source has one:

| source | latency | who compensates |
|---|---|---|
| audio in | buffer + device + safety offset | **Nothing, on the version we ship.** See the correction below. |
| camera | sensor exposure + ISP + USB transport, typically **20–150 ms** on a UVC webcam | **Nobody.** AVFoundation guarantees the timebase, not the origin, and exposes no capture-latency property. Media Foundation's `MFSampleExtension_DeviceTimestamp` is the best available and is still device-reported. |
| MIDI | a few ms | the device stamp already handles most of it |
| hosted plugin | its own reported latency | the plugin reports it; §4a |

> **Correction, 2026-08-16 — this table described cpal 0.18 and we ship 0.16.**
> Found while implementing `audio.rs`, by reading the vendored source rather
> than docs.rs. The `kAudioDevicePropertyLatency + kAudioDevicePropertySafetyOffset`
> subtraction is **0.18 behaviour**. cpal **0.16** subtracts one buffer of
> frames under a `TODO` comment and reads no device latency property at all, on
> any platform. So **input latency is assumed zero in the shipped build**, and
> `take.json` must report it as `"assumed"` rather than `"os_reported"` — which
> `LatencySource` in `take.rs` now models explicitly.
>
> **Why 0.16 and not 0.18.** 0.18 is a real API break (25 compile errors across
> `audio.rs`: `StreamError` moved, `SampleRate` became a plain `u32` alias,
> `duration_since` returns `Duration` rather than `Option<Duration>`). The
> benefit it buys is a few milliseconds on the *audio* leg — while the term that
> actually matters here is the **camera's 20-150 ms**, which no cpal version
> compensates. Migrating is therefore scheduled work, not urgent work: do it
> when lane 2 is otherwise complete, while `audio.rs`'s 44 tests are there to
> catch the regressions. Doing it now would spend the critical path's time on
> the wrong leg of the problem.
>
> Two more cpal 0.16 facts worth not rediscovering, both verified in the
> vendored source: **there is no stable device identifier** — `DeviceTrait`
> exposes only `name()`, CoreAudio's `AudioDeviceID` is `pub(crate)` and
> WASAPI's `GetId()` is used only inside its own `PartialEq` — so
> `record_camera_uid`-style stable selection is unavailable for audio and
> `DeviceKey` (name + occurrence-among-that-name) is the ceiling. And
> `StreamInstant`'s fields and `as_nanos` are private while `duration_since`
> returns `None` rather than a negative, so reading one requires a two-sided
> comparison — **ALSA genuinely produces negative instants**, because it
> computes `capture = callback - delay` from the stream trigger.

**Why the camera term is the one that matters, and why this feature makes it
maximally visible:** §6 puts the camera and the MIDI-driven key display *in the
same frame*. Uncompensated, the keys light up before the filmed fingers land.
That is a much lower detection threshold than lip-sync, because the eye is
comparing two things inside one image rather than sound against picture.

**What the design must carry, and none of it existed in the first draft:**

1. **A per-source `latency_ns` in the clock model**, applied when a device
   timestamp is converted into the timebase: `T = a·D + b − latency`. Default it
   to the best value the OS reports, `0` when it reports nothing, and record
   which it was.
2. **A one-time measured calibration, offered and never required.** Play a
   sharp attack; the recorder cross-correlates the audio onset against the frame
   in which the largest inter-frame pixel delta occurs, and stores the difference
   as `camera_latency_ns` per device UID. This is the only honest way to get the
   camera term, and it is a ~60-line offline computation over data the recorder
   already has. Store it beside `record_camera_uid`; a webcam's latency is a
   property of the device, not of the take.
3. **A manual A/V offset slider, in milliseconds, in the Export dialog**, seeded
   from the calibration and adjustable afterwards. Every tool surveyed has one
   because every tool needs one. Not having it means a user who can see the
   error has no way to fix it.
4. **`take.json` reports every term**: per source, the anchor, the fitted rate,
   the latency applied, and whether that latency was OS-reported, measured, or
   assumed zero. A constant offset is precisely the failure a sync report
   without a latency field cannot explain.

**And the verification section must be honest that it does not cover this.**
Test A drives fakes; Test B's camera is *also* a fake. **No test in this plan
touches real camera hardware**, so no test can catch a camera latency error.
That is acceptable only if it is written down: the calibration procedure in (2)
is the manual gate, and it belongs in the release checklist, not in `cargo test`.
Test B's `|Δ| < 1 ms` audio budget is also mis-justified — MIDI's 0.96 ms serial
time says nothing about a click traversing an output stream, a loopback ring and
an input stream. Budget that leg separately and empirically.

### Start, stop, and the camera warm-up

1. **Open the camera and the audio stream when the Recorder view opens, not when
   Record is pressed.** Run them continuously into the preview and the level
   meter. `AVCaptureSession::startRunning` is typically 300–800 ms on a built-in
   camera and can exceed 2 s for a Continuity Camera; the first ever run also
   raises a TCC prompt that costs seconds of human time.
2. Keep a **~1 s pre-roll ring** of audio and of frames in their native
   (compressed or NV12) form while previewing. Audio is 384 KB/s; 720p30 MJPEG
   is ~3 MB/s. Never ring-buffer decoded RGBA — that is 41 MB/s.
3. `T0 = max(T_audio_sample_0, T_video_frame_0, T_arm)`. Trim the audio head to
   `T0` and drop frames before it. The asymmetry is deliberate: trimming audio
   is lossless, whereas a non-zero first-frame PTS relies on edit lists that
   plenty of players ignore. **Force the first exported frame to PTS 0 and pay
   in discarded audio samples.**
4. Stop is symmetric: `T1 = Instant::now()`, audio through the sample nearest
   `T1`, video through the last frame with PTS ≤ `T1`. Keep the devices running
   afterwards so a second take pays no warm-up.

What is **not** acceptable is `T0 = T_arm` while the first frame is 600 ms
later, called frame 0. That is the first-frame-offset bug and it is the default
outcome of doing nothing.

### Timescales, and the drift nobody sees coming

Use a video timescale that expresses frame durations exactly — 30000 with
duration 1000, or 90000/3000; for 29.97, 30000/1001. Audio timescale = sample
rate. **A video timescale of 1000 cannot represent 1/29.97 exactly and
accumulates one frame of drift roughly every 20 minutes**, which is exactly the
owner's target take length.

**Write real per-frame PTS; never synthesise a constant duration.** Every USB
webcam is VFR in practice — it advertises 30 fps, delivers 30, then 24 when
auto-exposure lengthens in dim light, then drops one when the bus is busy. MP4
handles genuine VFR perfectly and QuickTime, browsers and YouTube all accept it.
The failure mode to avoid is the accidental third option: keeping the real
timestamps but writing a constant duration.

**No B-frames.** Then `ctts` is absent, PTS == DTS, the muxer is simpler and
debugging is simpler. Constrained Baseline forces it anyway on the Linux path.
The cost is bitrate, which is free on local disk.

---

## 4. Capture

### Audio

`cpal` input stream → `rtrb` SPSC ring → a writer thread. **Never write a file
from the audio callback.** The writer thread drains the ring, feeds the WAV
writer, feeds the video sinks' audio inputs, and computes meter values.

Levels are computed on the writer thread and published to the UI through an
`Arc<Mutex<Meters>>` read once per frame — not through a channel, because the UI
wants the latest value and not a history.

Device loss mid-take is a real event (a bus-powered interface browning out, a
USB hub re-enumerating). cpal reports it on the error callback. Policy: stop the
take, finalise every file that has bytes, mark `"complete": false` and
`"aborted_reason": "audio_device_lost"` in `take.json`, and say so in the UI.
**Never discard what was already captured.**

### 4a. The audio graph — sources 2 and 3

**Added after review.** §1 promises three audio sources and §5 ships a settings
key with three values; the first draft then specified only the first one, and
left the largest declared workstream unplanned below the level of "it is big and
it is last". That is not acceptable even for deferred work, because **two of its
decisions bind Step 1**, not Step 9.

```
            ┌────────────┐
 MIDI  ─────►  plugin    ├──► plugin_buf ──┐
 (host time)└────────────┘                 │   ┌───────┐
                                           ├──►│ mixer ├──┬──► WAV writer
            ┌────────────┐                 │   └───────┘  ├──► video sinks' audio
 input  ────►  cpal in   ├──► input_buf ───┘   gains,     └──► monitor out (cpal)
            └────────────┘                     per source
```

**The mixer runs on the writer thread, not on either callback.** It is the only
place both buffers exist, and it is already the thread that owns the WAV writer.

**Decisions that Step 1 needs, stated now:**

1. **The rate master when there is no input device.** §3's `α`/`β` fit reads an
   input callback's `(sample index, StreamInstant)`, and `T0` is
   `max(T_audio_sample_0, …)`. In plugin-only mode neither exists. Rule: **the
   plugin render loop becomes the rate master**, clocked by the monitor output
   stream if one is open, and by a `SampleClock` driven from `Instant` if not.
   In the latter case `ε` is 0 by definition, because nothing is being sampled
   by a crystal Tangent does not control — and `take.json` must say
   `"clock": "synthetic"` rather than silently reporting a fit it did not make.
2. **`both` mode may straddle two crystals.** An input device and a monitor
   output device are two clocks. §13 excludes *differing nominal sample rates*;
   it does not exclude two devices at the same nominal rate drifting apart.
   Rule: in `both` mode the **input device is the rate master** and the plugin
   renders into its timeline, because the input is the stream that cannot be
   re-timed. If the two devices are the same physical interface — the common
   case — this is free.

**Decisions that Step 9 needs, recorded so they are not rediscovered:**

3. **Plugin latency reporting is a real term** (§3a). A plugin reports its
   latency in samples; a live input carries its own capture latency. In `both`
   mode against the *same* instrument, misalignment is not merely a sync error —
   it is **comb filtering**, which is audible as a tonal change rather than as a
   delay, and is therefore easy to mistake for "the plugin sounds thin". PDC is
   mandatory in `both` mode, not a refinement.
4. **Block size and sample rate are a contract with the plugin**, negotiated once
   at load and fixed for the take. Changing either mid-take is out of scope.
5. **Monitoring is why `audio.rs` has an output stream at all.** A pianist
   playing a hosted piano must hear it, at low latency, and what they hear must
   be the **mix**, not a second render — a second render is a second voice
   allocation and will diverge on any instrument with randomisation or release
   samples.
6. **The Recorder band needs a source selector and two gain controls.** §5's
   control order and the Export dialog mock both omit it; it belongs in the
   right-hand column beside the device pickers, and it is the `record_audio_source`
   key made visible.

### Camera

One `VideoSource` trait, three backends. The trait yields
`(host_time_ns, Frame)` where `Frame` carries width, height, **stride**, pixel
format and one or two planes. Stride is in the trait because ignoring it is
precisely nokhwa's macOS corruption bug, and because egui_glow sets
`UNPACK_ALIGNMENT 1` with **no** `UNPACK_ROW_LENGTH`, so a strided buffer
uploaded naively shears.

**macOS.** `AVCaptureSession` with `AVCaptureVideoDataOutput`, forcing
`kCVPixelFormatType_32BGRA` in `videoSettings` so AVFoundation's own converter
does YUV→BGRA in the capture pipeline and there is no planar handling at all.
Delivery is a push on a serial `dispatch_queue`; never block in it.

Two macOS landmines, both to be handled before the backend is written:
- **Continuity Camera needs two things**, and every library gets it wrong.
  `NSCameraUseContinuityCameraDeviceType` in the Info.plist *and*
  `AVCaptureDeviceTypeContinuityCamera` passed in the discovery session's device
  type array. Out of the box neither nokhwa nor `cameras` will find the user's
  iPhone. For a piano app where the best camera in the room is an iPhone on a
  mount, that is a headline feature, and it is ~10 lines when you own the
  enumeration.
- **The deployment-target trap.** `LSMinimumSystemVersion` is 11.0
  (`build-macos.sh:187`). objc2 emits `AVCaptureDeviceTypeExternal` and
  `AVCaptureDeviceTypeContinuityCamera` (both macOS 14+) as plain non-weak
  `extern "C"` statics. Referencing either symbol breaks **launch** on macOS
  11/12/13, not merely the camera. Link only `BuiltInWideAngleCamera` and
  `ExternalUnknown` (10.15+) and build the 14+ type strings at runtime as
  `NSString`s. Verify this before writing the backend, not after.

**Windows.** `IMFSourceReader` on a worker thread with
`CoInitializeEx(COINIT_APARTMENTTHREADED)`. Prefer NV12 straight through to the
encoder and convert only the preview copy; `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING`
is software and Microsoft's own docs say it is "not optimized for playback".
Use `MFSampleExtension_DeviceTimestamp`, which is documented as being in the
MFTIME domain sharing an epoch with QPC — the same clock WASAPI capture
positions use. nokhwa leaves this on the table and uses stream-relative time.

**Linux.** `linuxvideo` (0BSD, pure Rust, no bindgen). Read
`v4l2_buffer.timestamp` and check `V4L2_BUF_FLAG_TSTAMP_SRC_MASK`.

### MIDI

**Tee the raw bytes.** `ivory/src/midi.rs`'s callback becomes:

```rust
// `tap` is an Arc<RawMidiTap> CLONED IN before `connect`, and it is ALWAYS
// INSTALLED. It is never an Option and it is never re-installed.
move |stamp_us, message, _| {
    tap.push(stamp_us, message);                    // always on; see below
    if let Some(event) = parse_message(message) {   // unchanged display path
        let _ = tx.send(event);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}
```

**The tap is always on, and that is a correction, not a shortcut.** The first
draft wrote `Option<RawMidiTap>, None when idle` and that describes a state
transition with no possible mechanism. midir 0.11.0 takes the callback **by
value** (`F: FnMut(u64, &[u8], &mut T) + Send + 'static`, `common.rs:120-128`)
and `MidiInputConnection` has exactly one method — `close(self) -> (MidiInput, T)`
(`common.rs:192-200`, the only `impl` block; no OS-extension trait adds
accessors; on CoreMIDI the closure and its `T` are sealed inside a private
`Arc<Mutex<HandlerData<T>>>`). **Nothing outside the closure can ever reach it
while the connection is open.** A captured `Option` is fixed for the life of the
connection.

So `RawMidiTap` owns a fixed-capacity ring behind interior mutability
(`rtrb::Producer` needs `&mut self`, so a `Mutex` or an `UnsafeCell`-backed SPSC
wrapper; a `Mutex` is fine here because the MIDI callback thread is an ordinary
thread, not an audio callback, and the only contender is the drain). A reader
thread drains it continuously and **discards while not recording**.

Three things fall out of that, all of them wanted anyway:

- **No arm/disarm race** and no reconnection when Record is pressed.
- **Pre-roll MIDI is free**, which §7 rule 8 needs: the snapshot of "which
  program, which CC values, which pedal position was already true at `T0`"
  requires having seen the messages that set them, and those arrive long before
  Record.
- **A note held across `T0`** is visible. §7 gains a rule for it (below) and
  Test A gains the case.

Cost is a few KB and a memcpy per message. A dense piano take is a few thousand
messages a minute.

Two rules that are not obvious:

- **Use the device stamp, not `Instant::now()`, as the event time.** `now()` in
  the callback is arrival time, which carries OS delivery latency and its
  jitter, and on ALSA the sequencer queue's own scheduling. Use `now()` only to
  build the anchor.
- **The stamp is microseconds and the timebase is nanoseconds.** Every backend.
  `× 1000`, through the anchor's `a` coefficient. The epoch is unspecified and
  per-connection, so **every** source anchors — including macOS MIDI, whose
  domain is right and whose origin still is not.
- **Where `DeviceMidi` holds the tap.** `ivory/src/desktop.rs:26-30` gains an
  `Arc<RawMidiTap>` field, cloned into each `connect_by_name` closure. Switching
  ports drops the old connection and makes a new closure holding a clone of the
  *same* `Arc`, so a port switch mid-session does not lose the ring or the
  controller-state snapshot. **This is entirely inside the `ivory` crate** —
  `MidiPorts` and `ivory-ui` do not change.
- Add `input.ignore(Ignore::TimeAndActiveSense)` at `midi.rs:54`. Keeps SysEx,
  drops the ~300 ms Active Sensing heartbeat that would otherwise dominate the
  file.

Platform caveats to state in `take.json` rather than paper over: WinMM's stamp
is **millisecond-quantised** (a `DWORD` of ms), so do not report sub-millisecond
numbers on Windows; midir's JACK backend stamps at callback dispatch rather than
from the event's frame offset, collapsing every event in a period to one time
(5.3 ms at 256 frames); midir's ALSA epoch is its own queue's start, created
inside `connect()`, so treat it as relative-only and anchor it.

---

## 5. The Recorder band, and why it is a band

The window is a stack of bands whose heights are pure functions of width, fixed
size, `with_resizable(false)`, and there is **no menu bar** — the right-click
context menu is the entire chrome. Any design that fights that loses.

`ivory-ui/src/recorder_panel.rs`, following `fretboard_panel.rs` and
`theory_panel.rs` exactly: a `BAND_H_AT_1300: f64 = 200.0`, a
`band_height(w) -> f32`, a `draw(painter, rect, state, settings)`, a
`hit_test(rect, pos) -> Option<Hit>`, an edge trim, and a detached variant.

At w=1300 that is 200 pt of height, which gives a **~320×180 preview** with
padding and leaves **~950 pt of width for controls** — the one thing this window
has in abundance.

**The preview is drawn with `Painter::image(texture_id, rect, uv, tint)`
(`egui/src/painter.rs:488`), and the `uv: Rect` argument is the whole trick.**
Letterbox or crop the camera's aspect inside a band whose height is a function
of width only. **The camera's aspect never reaches the layout** — that is the
part that matters, and it is what keeps the window from growing to 731 pt.

*Corrected by review:* the first draft added "so `band_sizes_at`, `fit_bands`,
`Bands::total` and `band_at` are untouched", which overstated it. Adding a band
to the stack necessarily edits four things: `Bands` gains a field
(`app.rs:1781-1787`), `Bands::total` an addend (`:1790-1795`), `band_sizes_at` a
branch (`:1724-1745`), and the **exhaustive destructure at `app.rs:1968-1974`,
which has no `..`**, plus one new `band_at` call with shifted `top` offsets.
`fit_bands` and the `band_at` closure itself genuinely are untouched.

**"Every geometry test stays valid" is correct**, and for a reason worth naming:
with `show_recorder` false under `Settings::default()` the band is 0 pt, so
`size_math_matches_python_int_truncation` (`:3049-3086`),
`the_desktop_layout_is_unchanged` (`:2745-2753`) and
`a_plugin_lays_out_inside_the_rect_it_is_given` (`:2707-2740`) are unaffected,
and no test constructs a `Bands` literal. This is proven by precedent, not
hoped: commit `eac5a93` added the theory band with exactly this edit set and
touched no size test.

**Zeroing the band under `Caps::PLUGIN` needs no signature changes.** Clear
`show_recorder` on the local `settings` copy in `IvoryApp::new(ctx, mut
settings, caps)`, exactly as `app.rs:257-260` already clears
`chord_window_detached` and `fretboard_detached`. Every plugin path goes through
it (`plugin/src/lib.rs:194`, `ivory/src/desktop.rs:92`, and the test helper at
`app.rs:2261-2263`).

**Two layouts, switched by state**, because no other tool knows the user is a
pianist: wide preview while idle and framing; **collapsed preview, huge
timecode, huge meter** while rolling, readable from the bench two metres away.
While playing, the pianist is looking at their hands — the preview's job is
framing *before* the take.

**Detach reuses the existing pattern** (`DetachChordWindow` / `DetachFretboard`,
`menu.rs:41-42, 66-67`) so a second monitor can hold a big framing view. That is
free architecture; the app already does it twice.

### How the band is turned on — the affordance the first draft forgot

`show_recorder` defaults to `false` and is deliberately kept out of
`first_launch()`. The first draft then never said how anyone switches it on, so
**as written the entire feature shipped invisible.** There is no third surface
to fall back on: the context menu is, in the plan's own words, "the entire
chrome", and `keys.rs` is the only other toggle path — there is no preferences
dialog (`dialogs.rs:13`).

Concretely, mirroring `ToggleFretboard` (`menu.rs:54` / `:437`) exactly:

- `MenuAction::ToggleRecorder`, and `DetachRecorder` / `AttachRecorder`.
- A menu row that **renames itself** (`Show Recorder` / `Hide Recorder`) rather
  than carrying a checkmark, per the chrome rule at `menu.rs:1-10`.
- Gated on the new `caps.capture_devices`; the **detach** row gated on
  `caps.detachable` *as well*, reusing the existing gate at `menu.rs:445`.
- All three names added to the `forbidden` array in
  `no_surviving_plugin_row_needs_a_window_or_a_device` (`menu.rs:1273-1279`).
  Missing this is how a plugin row for a camera would slip through.
- `Hide Recorder` closes the detached window rather than orphaning it, exactly
  as `Hide Fretboard` does.

**Space is not free either.** §5 binds Record to Space, but `keys.rs`'s
`BINDINGS` table (`:46-72`) is single-letter and modifier-free by construction
and is the single source for the hold-to-view help card. Adding Record means
adding a row there, and it means deciding what Space does when the Recorder band
is hidden (answer: nothing — the binding is gated on `show_recorder`, or a user
who has never opened the recorder starts a take by pressing Space).

Control order, left to right: preview · transport + level + elapsed · destination
+ name + devices + disk. Not device pickers first — those must be invisible after
the first session.

Specific decisions worth defending:

- **One button, not arm-then-record**, bound to Space, with a settable pre-roll
  countdown (0/3/5 s, default 3) so the user can walk back to the bench.
- **A steady red border and dot, never blinking**, plus a settings option
  *"Hide elapsed time while recording"*. A blinking indicator measurably degrades
  performance, and this is the most-cited psychological complaint in the piano
  forums. No competitor has that checkbox.
- **The level meter is live before arming.** The entire "I recorded silence"
  failure class dies here. Peak-hold plus a clip latch that survives the take and
  is reported after Stop.
- **Disk is shown as a duration, not bytes.** "214 GB free" means nothing to a
  pianist; "~58 min at current settings" does. Recompute from bytes actually
  written after the first 10 s so it self-corrects.
- **The take-name field persists across takes and is not required.** Because the
  timestamp guarantees uniqueness, the typed name never has to be unique — so
  type "nocturne" once, press record five times, get five adjacent folders and
  **no overwrite dialog ever**. That turns "leave it running for the whole
  practice session" into a supported workflow. Show the resulting folder name as
  live grey text under the field; it teaches the scheme without a help page.

### The Export dialog and the composition selector

The owner asked for this specifically: *"A dialog menu with a selector that can
include: Camera + Synced sound + Tangent's Display all in one video ALONG with
midi in the export. It should also be able to have the other options selected
for flexibility."* So it is a real dialog with a real selector, not a settings
page and not a fixed output set.

It is the twelfth `Dialog` variant, rendering through the existing single funnel
(`dialogs.rs:229 show_dialog_viewport`) so it is an OS window on the desktop and
in-canvas anywhere `caps.child_windows` is false — free, because that machinery
already exists for eleven dialogs.

**It is reachable from two places and they mean different things.** From the
Recorder band's `Export...` button *before* a take, it chooses what that take
will produce. From the result strip *after* a take, it re-runs **the subset that
is genuinely re-derivable** — and the first draft was wrong about how large that
subset is.

> **Corrected by review.** The first draft justified post-take re-export with
> "the WAV, the MIDI and the composited frames are all already on disk or
> reconstructable". The frame half contradicts §0 (raw frame retention is "not a
> design, a bug", 112 GB per take), §3 (only a ~1 s pre-roll ring is kept), and
> §6 (each composited frame goes straight to the sink and nothing is kept).
> There is **no decoder anywhere in this plan** — `video/` is encode-only and
> §13 rules out an editing path.

So, precisely:

| after Stop | re-derivable? | why |
|---|---|---|
| SMF tempo mark | **yes** | rewriting the tempo meta is a file edit |
| a **display-only** video, any layout, any panel selection | **yes** | replay the recorded `.mid` offline through the compositor. The four `draw` functions are pure painters of state with **no time dependence** — verified: zero `Instant`, `ctx.input` or `animate_` in any of them — so the display at any instant is a function of the MIDI up to that instant. |
| anything containing the **camera** | **no** | those frames were encoded live and composited live |

**Consequence, stated plainly in the dialog:** a take recorded without the camera
layer can never gain it, and "Camera full frame, display overlaid" can never
become "Side by side". The result-strip dialog therefore greys the camera
controls and says why.

**If post-take camera re-layout is wanted, it has to be bought, not assumed.**
The price is: always write a camera-only master alongside the composite, and add
a decode-and-recomposite path — which means a decoder, which §2 and §7 currently
plan for nothing. That is a real feature with a real cost and it belongs in §13
until someone asks for it. (Note the first draft also had this backwards: it
claimed "a design that re-encodes from the camera at stop time cannot offer
that". Retaining a camera master and compositing at stop is *exactly* what would
enable re-layout. Encode-while-recording buys crash safety and a finished file
at Stop, and it costs post-take flexibility. That is the trade, and it is worth
making — just not worth misdescribing.)

```
+-- Export -------------------------------------------------------------+
|                                                                       |
|  Always written        [x] Audio      nocturne.wav    48k/24 stereo    |
|                        [x] MIDI       nocturne.mid    SMF 1, 960 ppq   |
|                                       Tempo mark  [ 120 ] BPM          |
|                                                                       |
|  Video                 ( ) None                                       |
|                        (o) One video, composited      nocturne.mp4     |
|                        ( ) Separate file per source                    |
|                        ( ) Both                                        |
|                                                                       |
|    Composite contains  [x] Camera                                     |
|                        [x] Tangent's display                          |
|                        [x] Performance audio                          |
|      Layout            (o) Camera above, display below                |
|                        ( ) Display above, camera below                |
|                        ( ) Camera full frame, display overlaid        |
|                        ( ) Side by side                               |
|      Display shows     [x] Piano  [x] Chord name                      |
|                        [ ] Fretboard  [ ] Theory                      |
|                                                                       |
|    Resolution          (o) 1920x1080 30fps   ( ) 1280x720 30fps        |
|                        ( ) Match camera                               |
|                                                                       |
|  Estimated             ~248 MB, ~2 encoders, ~14% CPU                 |
|                                                                       |
|  [x] Use these settings for every take        [ Cancel ]  [ Export ]   |
+-----------------------------------------------------------------------+
```

Four decisions in there that are not arbitrary:

- **Audio and MIDI are checkboxes but default on and cannot both be off.** The
  owner's brief says the directory exports the audio, the MIDI and the video;
  a take with neither is not a take. The dialog refuses rather than producing an
  empty directory.
- **"Display shows" reuses the app's own panel toggles rather than inventing a
  second set.** It seeds from the live `Settings` (`show_fretboard`, the three
  `theory_*` flags) and overrides them **for the video only**, so a user can
  record a clean piano-and-chord video while keeping the fretboard on screen for
  their own use. Because the compositor drives the same `draw` functions with its
  own `Settings` copy, this is a struct field, not a code path.
- **The live estimate is a duration and a CPU cost, not just bytes.** Three
  simultaneous 1080p30 encodes is nothing for VideoToolbox and Media Foundation
  and is genuinely expensive for openh264 on Linux. The dialog says which one it
  is on the machine it is running on.
- **"Use these settings for every take" is the same one-click persistence
  affordance as the output directory's "Default" checkbox**, and writes to the
  same place.

### Settings keys

Additive keys in `~/.config/ivory/settings.json`, written in the hand-rolled
serde at `settings.rs:417-420` (read) and `:531-539` (write) — a new key must be
added in **both** or it silently vanishes on save. All are absent-means-default
so an older build never chokes and unknown keys are already preserved.

| key | type | default | note |
|---|---|---|---|
| `show_recorder` | bool | `false` | Off by default. Turning it on makes the window taller, and a window that grows after an update is the D-UI-10/11 surprise again. |
| `record_dir` | string? | absent | Absent means the platform default: `~/Movies/Tangent`, `%USERPROFILE%\Videos\Tangent` via `FOLDERID_Videos` (never a hardcoded string), `$XDG_VIDEOS_DIR/Tangent`. **Never the Desktop, never the bundle directory.** |
| `record_dir_is_default` | bool | `false` | The "set as default" checkbox. False means the picker opens at `record_dir` but the choice is per-session. |
| `record_take_name` | string? | absent | Persists across takes, because the timestamp already guarantees uniqueness. §5. |
| `record_camera_uid` | string? | absent | The **stable device UID**, not the display name. Two identical webcams share a name; a name also changes with the OS locale. |
| `record_audio_device` | string? | absent | Same reasoning. |
| `record_audio_source` | string | `"input"` | `input` / `plugin` / `both`. |
| `record_export_*` | — | — | The dialog's whole state, when "use for every take" is ticked: video mode, composite contents, layout, display toggles, resolution, fps, tempo mark. |
| `record_preroll_s` | int | `3` | 0 / 3 / 5. |
| `record_hide_elapsed` | bool | `false` | §5. |

`first_launch()` (`settings.rs:250-253`) sets the *visibility* flags true, and
`a_first_launch_shows_everything_and_a_broken_file_does_not`
(`settings.rs:733-770`) asserts first-launch equals defaults plus exactly those.
**Deciding whether `show_recorder` joins that set is a deliberate choice that
changes an existing test**, not an oversight — and the answer is no for the same
reason `show_fretboard` is off: the recorder band is 200 pt tall.

### `Caps` and the firewall crossing

Three new `Caps` fields, each naming a capability and not a host, per
`host.rs:13-16`:

```rust
pub capture_devices: bool,     // may open a camera or an audio input
pub write_user_files: bool,    // may create take directories outside the config dir
pub native_file_dialogs: bool, // a "Choose folder..." button is meaningful
```

All three `true` in `DESKTOP`, all three `false` in `PLUGIN`. A VST3 has no
business opening a camera, and multi-instance plus offline bounce make
plugin-side file writing actively dangerous. **The recorder band's height must
be exactly zero when `!caps.capture_devices`**, or `fit_bands` will shrink the
piano to make room for a band the plugin must never populate.

The directory picker uses the **request pattern**, not a blocking trait call:
`pending_directory_request: Option<DirRequest>` in `ivory-ui` with a
`take_directory_request()` drained by `impl eframe::App for DesktopApp` **after**
`self.0.frame(ctx)` returns, exactly as `pending_resize` is drained by the
plugin at `plugin/src/lib.rs:251`. That keeps `NSOpenPanel`'s nested run loop out
of the middle of an egui frame, and the plugin refuses simply by never draining.

Devices themselves go behind traits in the `MidiPorts` shape — `CameraPorts`,
`AudioPorts`, `PluginHost` — held as `Option<Box<dyn _>>`, injected by setters
the plugin never calls.

---

## 6. The compositor

This is the part with no prior art in the repo and the part that makes the
feature what the owner asked for: **one video containing the camera, the
performance audio, and Tangent's own display.**

**The load-bearing fact from §0: every panel is a pure painter.** `piano::draw`,
`chord_strip::draw`, `fretboard_panel::draw` and `theory_panel::draw` take an
`&egui::Painter` and a `Rect` and paint absolutely, with no widgets — verified in
their bodies, not just their signatures: none touches `ui`, a `Context`, input,
or time. So they can paint into a 1920×1080 surface as happily as into a 1300 pt
window, **with no changes to any of them**.

### Where the compositor lives, and the one new public API

**Rewritten after review**, which correctly found that the first draft gave the
compositor no crate, no module, and no route to the state it must paint. Every
field of `IvoryApp` is private (`app.rs:130-232`) and every helper that produces
a draw argument is a private `fn` — `display_notes` (`:595`), `theory_input`
(`:627`), `barre_to_draw` (`:549`), `heart_color` (`:762`). From outside
`ivory-ui` there was nothing to paint.

**It splits at a pure-`egui` seam that already exists**, and the split is what
keeps the firewall intact:

| side | crate | what it owns |
|---|---|---|
| the pass | `ivory-ui/src/composite.rs` | the offscreen `Context::run` closure and the band painting. Sees only `egui`. |
| the surface | `ivory/src/composite.rs` (new module) | the offscreen `Context`, an `egui_glow::Painter`, the FBO, the PBOs, the readback |

That works because **no eframe type crosses the seam**:
`eframe::Frame::gl()` (`epi.rs:749-751`) returns `Option<&Arc<glow::Context>>` —
a *glow* type — and `egui_glow::Painter::paint_and_update_textures`
(`painter.rs:356`) consumes only `&[egui::ClippedPrimitive]` and
`&egui::TexturesDelta`, i.e. no application state at all. So `ivory-ui` needs
neither `egui_glow` nor `glow`, `check-firewall.sh` stays green, and §2's
"`ivory-ui` gains no new dependencies" holds. `egui_glow 0.33.3` is already in
the root `Cargo.lock` via eframe's `glow` feature.

**The new public API is one method**, beside the existing `pub fn paint_into`
(`app.rs:1816`):

```rust
// ivory-ui/src/app.rs
pub fn paint_composite(&self, painter: &egui::Painter, rect: Rect, opts: &CompositeOpts)
```

`&self`, not `&mut self`, and that is the whole safety argument. It must **not**
be `paint_into` called a second time: `paint` drains the MIDI channel
(`process_midi_events`, `:1838`), runs `voicing_tick` / `detection_tick`, and
overwrites `self.last_pane` and `self.last_drawn` (`:2013-2014`) — which dialog
placement (`:2227`) and `request_natural_size` (`:433`) read. A composite pass
that did any of that would move the app's dialogs to wherever the video frame is.

So the band-stacking arithmetic currently inlined at `app.rs:2015-2063` is
refactored into a private `&self` helper taking a `Settings` and a `Rect` and
touching no `&mut` state, and `paint_composite` calls it with the *export's*
`Settings` copy — which is exactly how the Export dialog's "Display shows"
checkboxes override the live panel toggles for the video only.

A public `RenderSnapshot` type exporting `Voicing`, `FretboardSpec`, `Wood`,
`Barre` and `theory_panel::Input` was considered and rejected: `&self` reaches
all of it in-crate, and the snapshot would widen `ivory-ui`'s public surface for
no gain.

**Fonts.** The offscreen context has its own atlas and must be seeded with
`app.install_fonts(&offscreen_ctx)` (`app.rs:393`), or the chord names render in
egui's defaults instead of Courier Prime.

The pipeline, per composited frame:

1. A second, offscreen `egui::Context` sized to the video resolution. Run one
   pass that paints the chosen composition: the camera texture, then the piano /
   chord strip / fretboard / theory bands at video scale, laid out by a
   `CompositeLayout` the user picks in the Export dialog.
2. Render that pass with an `egui_glow::Painter` into an FBO owned by the
   recorder, using the app's `glow::Context` — `eframe::Frame::gl()`
   (`epi.rs:749-751`) hands it over. `egui_glow` is already in the tree via
   eframe's `glow` feature; this is not a new dependency.
3. Read back with **double-buffered PBOs** so `glReadPixels` never stalls the
   frame, **flip vertically**, convert to NV12 with `yuv`, hand to the
   `VideoSink`.

**The flip is not optional and it is easy to miss.** egui_glow's vertex shader
maps egui's y-down coordinates with `1.0 - 2.0 * a_pos.y / u_screen_size.y`
(`shader/vertex.glsl`), unconditionally — `paint_primitives` takes only
`screen_size_px` and `pixels_per_point`, with no projection or flip parameter,
and `set_clip_rect` scissors at `height_px - clip_max_y` (`painter.rs:787`). So
`glReadPixels(0, 0, w, h)` returns **row 0 = the bottom of the image**.
egui_glow's own `read_screen_rgba` pays for this explicitly with
`chunks_exact(w*4).rev()` into a buffer it names `flipped` (`painter.rs:659-678`)
— but `read_screen_rgb` (`:681-696`) does **not** flip, so neither helper can be
copied blind. `yuv 0.8`'s `rgba_to_yuv_nv12` takes top-down rows with an
*unsigned* stride, so the negative-stride trick is unavailable. Do the flip as
part of the NV12 conversion loop, not as a separate pass.

Consequence if skipped: the composite and `-display.mp4` come out vertically
mirrored. `-camera.mp4` is unaffected because those frames bypass the FBO. It is
100% visible on the first Step 7 playback rather than latent — but it will cost
an afternoon of suspecting the encoder.

**And the channel order.** §4 forces `kCVPixelFormatType_32BGRA` on macOS
because it makes AVFoundation do the YUV conversion; the egui upload path is
**RGBA and only RGBA**. `ColorImage::from_rgba_unmultiplied` packs
`p[0]→R, p[1]→G, p[2]→B` (`epaint/src/image.rs:112`), `ImageData` has exactly one
variant documented "RGBA image", egui_glow uploads `(RGBA8, RGBA, UNSIGNED_BYTE)`
(`painter.rs:581-590`), and **`grep -i bgra` across all of egui, epaint and
egui_glow 0.33.3 returns zero hits** — nothing anywhere reinterprets byte order.
Feed BGRA to that constructor and red and blue swap, in the preview at Step 5 and
again in the composite at Step 7. One `yuv::bgra_to_rgba` call fixes it, and
`yuv` is already in §2's dependency table.

Costs and the honest limits:

- 1080p30 readback is ~250 MB/s over PCIe. Double-buffered PBOs make it
  asynchronous; single-buffered `glReadPixels` would serialise the GPU and drop
  the app to a crawl. This is the one place where getting it wrong is very
  visible.
- The offscreen context has its own font atlas and its own texture manager.
  Fonts must be installed into it exactly as `app.rs:392-400` does for the main
  context.
- The camera texture must be uploaded **once per composited frame and once per
  preview frame at most**. `TextureHandle::set` takes the `tex_image_2d` path
  (full realloc every frame); `set_partial([0,0], ..)` on a same-size image takes
  `tex_sub_image_2d`. But `TextureManager` de-duplicates only *full* deltas —
  partial deltas accumulate — so `set_partial` is only a win when called
  **exactly once per egui pass**, from the paint body, never from the capture
  thread.
- Allocate any registered native texture as **`RGBA8`, not `SRGB8_ALPHA8`**.
  egui_glow hardcodes `srgb_textures = false` and disables `FRAMEBUFFER_SRGB`,
  so bytes reach the screen unconverted — right for a webcam, and wrong-looking
  if you guess sRGB.
- A texture uploaded without a repaint request is never flushed:
  `textures_delta` is drained in `Context::end_pass`. The capture thread must
  call `request_repaint_of(ROOT)` per frame, exactly as `midi.rs:68` does — which
  raises the app's steady rate from the current 20 Hz (`request_repaint_after(50ms)`)
  to 30–60 Hz. egui 0.33 has **no partial damage**, so every pass re-tessellates
  the whole window. Gate the preview to the Recorder view and drive repaint only
  while it is open.

**The composite is the only thing that needs the compositor.** `-camera.mp4`
takes camera frames straight to a sink; `-display.mp4` takes composited frames
with the camera layer disabled. All three share one frame clock.

---

## 7. Encode, mux, and the MIDI file

### Video

**OS-native encode and mux behind one `VideoSink` trait**, because it costs the
MIT story exactly nothing: the H.264 implementation being executed is Apple's or
Microsoft's, licensed by them, and nothing enters `Cargo.lock` but MIT/Apache
binding crates.

- **macOS**: `AVAssetWriter` + `AVAssetWriterInput` ×2 + `AVAssetWriterInputPixelBufferAdaptor`.
  Encode and mux in one object, AAC free from the system encoder, edit lists,
  `stts`, timescale and interleaving handled correctly including genuine VFR
  because you hand it a `CMTime` per frame. ~250–400 lines with the objc2
  ceremony.
- **Windows**: `IMFSinkWriter` + the MP4 media sink, `MFVideoFormat_H264` +
  `MFAudioFormat_AAC`. ~400–550 lines, most of it HRESULT plumbing. **Detect
  Windows N/KN**, where `MFTEnumEx` returns no H.264 encoder until the Media
  Feature Pack is installed, and say so in the UI rather than writing a
  zero-byte file.
- **Linux**: `openh264` (BSD-2) + a muxer. Constrained Baseline only, so ~1.5×
  the bitrate for equal quality, and ~8 ms/frame at 1080p **with nasm** versus
  ~24 ms without. **Make nasm a hard build requirement on Linux.** There is no
  pure-Rust AAC encoder and QuickTime does not play Opus in MP4, so the Linux
  audio track is LPCM in a `.mov` until that changes. Say it in the UI.

Rejected, with reasons, so nobody reopens them: **`ffmpeg-next`/`rusty_ffmpeg`**
— static LGPL linking obliges shipping relinkable object files, which produce an
unsigned binary macOS 26 refuses to run, so the freedom granted is unusable;
dynamic linking needs either self-signed dylibs (defeating the swap) or
`com.apple.security.cs.disable-library-validation`; and `ffmpeg-sys-next`'s
autotools build ends the `cargo xwin` / `cargo zigbuild` pipeline outright.
**Any GPL ffmpeg, bundled or auto-downloaded** — `ffmpeg-sidecar`'s default
`download_ffmpeg` feature pulls GPLv3 builds from gyan.dev, johnvansickle and
evermeet.cx (the last of which is Intel-only, so not even universal). The whole
value of this repo's licensing story is that GPL lives in exactly one
quarantined place.

An **LGPL-only ffmpeg sidecar** is the documented fallback if the three-backend
cost proves unaffordable, with `default-features = false` mandatory, macOS
universal built by hand (BtbN publishes no macOS), source published beside every
release, the attribution line added, the helper signed separately, and FFmpeg
hand-added to `THIRD-PARTY-LICENSES` because the generator reads `Cargo.lock`
only and cannot see a bundled binary.

**Patent reality, and it is not the same on all three platforms.** The first
draft said "using the OS encoders removes even the question, because you are not
distributing an AVC implementation" — true on macOS and Windows, and **false on
Linux**, where this same section specifies openh264 compiled from source and
statically linked. That contradiction needs no legal argument to see; it is two
paragraphs apart.

- **macOS and Windows.** The H.264 implementation executed is Apple's or
  Microsoft's, licensed by them. Nothing is distributed by us. No question
  arises.
- **Linux.** `openh264-sys2` 0.9.8's `default = ["source"]` makes `build.rs`
  compile the bundled Cisco C++ tree and emit
  `cargo:rustc-link-lib=static=openh264_*`. **Cisco's MPEG LA royalty grant does
  not cover this.** `openh264.org/BINARY_LICENSE.txt` grants it only for "the
  Cisco-provided binary", conditioned on that binary being "separately downloaded
  to an end user's device, and not integrated into or combined with third party
  software prior to being downloaded", plus end-user enable/disable control and
  an on-screen "OpenH264 Video Codec provided by Cisco Systems, Inc." Cisco's FAQ
  is blunt about the alternative: "a team can choose to use the source code, in
  which case the team is responsible for paying all applicable license fees."
  BSD-2 is a copyright grant with no patent grant.

**So what actually covers Linux is Via LA's schedule: $0.00 for 1–100,000 units
annually**, refreshed 2026-08-01 with fee structures unchanged. That is the
argument, it is sound at this project's volume, and it must be the *stated* one
rather than borrowed from the macOS/Windows case. The alternative — openh264's
`libloading` feature — buys Cisco's shelter at the price of an install-time
download of Cisco's module and a user-facing toggle, which is a worse product for
a marginal legal gain. **Choosing `source` is defensible; leaving it
undocumented is not.** §10 item 6 also has to hand-add openh264-sys2 to
`THIRD-PARTY-LICENSES`.

None of this touches the copyright story: openh264 and openh264-sys2 are
BSD-2-Clause, nothing copyleft enters the lock file, and `plugin/` remains the
only GPL place.

Do not pick AV1 for licence reasons — Sisvel runs an AV1 pool of non-AOMedia
licensors and Dolby sued Snap over AV1 in March 2026. There is no licence reason
left, and H.264 plays everywhere.

### The container and codec contract, per platform

The first draft wrote `.mp4` in five places and `.mov` with LPCM in one, and
never reconciled them.

| platform | container | video | audio |
|---|---|---|---|
| macOS | `.mp4` | H.264 (VideoToolbox) | AAC |
| Windows | `.mp4` | H.264 (Media Foundation) | AAC |
| Linux | **`.mov`** | H.264 Constrained Baseline (openh264) | **LPCM** |

Linux differs because there is no pure-Rust AAC encoder and QuickTime does not
play Opus in MP4. The cost is file size, and it is large: 20 minutes of 48 kHz
24-bit stereo LPCM is ~346 MB muxed in. The Recorder band says so on Linux
rather than surprising the user, and `take.json` gains `"container"` and
`"audio_codec"` fields — without them the sidecar this plan calls "the only
thing that will let a user's 'it's out of sync' report be debugged" cannot even
say what the muxed audio is.

### MIDI

`midly` 0.5.3, `default-features = false, features = ["std"]` — that turns off
`parallel`/`rayon` and makes it a **zero-dependency** crate, one line in
`Cargo.lock`, one Unlicense entry in `THIRD-PARTY-LICENSES`.

**Format 1, `Timing::Metrical(960)`**, track 0 the tempo map only, track 1+ the
performance. 960 is Logic's native resolution and exact in Cubase, REAPER and
Live; it divides by 2,3,4,5,6,8,10,12,15,16,20,24,32 so every tuplet lands on an
integer tick; and one tick at 120 BPM is 520 µs, well under MIDI's own 0.96 ms
serial transmission time for a 3-byte message.

**Make the tempo a user-visible field defaulting to 120.** This is the real fix
for the one honest problem with metrical time: a metrical SMF is a record of
*beat positions* plus an *assertion* about tempo, and the assertion is the first
thing importers discard — Logic ignores it into an existing project, Cubase gates
it on "Ignore Master Track Events", REAPER on an import preference. A user who
plays to a 72 BPM click and types 72 gets a file that drops into their 72 BPM
project sample-accurately *and* whose bar lines land right. Ten lines. Do not
ship SMPTE division as a second file: mido assigns the header's third value
straight to `ticks_per_beat` with no sign-bit check, and midly's own SMPTE
*parser* has an open overflow panic. It is an untested corner of the ecosystem.

**Solve absolute placement in the audio file instead, where the ecosystem
works: write a BWF `bext` chunk** with `TimeReference` = samples since midnight.
REAPER, Pro Tools, Nuendo and Pyramix all place BWF at its source position. It
costs ~600 bytes and it is the single highest-leverage interop decision here —
nobody in the piano-tool space does it. Note `hound` **cannot** do this: it
writes `"fmt "` and `data` only with no hook for extra chunks. That is why
`wav.rs` is hand-rolled (under 200 lines) rather than taking the dependency.

The bugs a naive implementation ships with, every one of which is to be covered
by a test:

1. **No `EndOfTrack`.** midly does not write it for you and never warns. The
   file opens in most readers, so it passes local testing and breaks elsewhere.
2. **Hanging notes at stop** — the most-reported MIDI recorder bug in existence.
3. **Hanging pedal at stop**, nastier because it is invisible in a piano roll:
   the notes have offs, CC64 is still 127, the sampler sustains anyway.
4. **Pairing note-on/off by key alone.** Key the held set on `(channel, key)`.
5. **Re-triggered notes.** A trill gives `On(60) On(60) Off(60) Off(60)`; a
   `HashSet` emits one spurious note-off at stop. Use a **counter**, and cap it.
6. **Rounding each delta instead of the absolute tick.** Delta rounding
   accumulates monotonically — drift, not jitter, so it is what a listener hears.
7. **Negative deltas from out-of-order arrivals.** `u28::from(x as u32)` masks
   silently, putting the rest of the file 38 hours in the future. Stable-sort by
   absolute tick and use the inherent `u28::try_from -> Option`.
8. **State that predates `T0`.** The keyboard sends its program change at
   connect time, and the pedal may already be down when Record is pressed.
   **Snapshot the last-seen program and the last CC7/CC11/CC64/CC66/CC67 per
   channel at `T0` and emit them at tick 0**, or the file has no instrument and
   a pedal release in bar 3 looks spontaneous. The always-on tap (§4) is what
   makes this possible — a tap armed at Record has never seen the messages that
   set the state it is meant to snapshot.
9. **A note already sounding at `T0`.** Rule 8 covers controllers and forgets
   notes. A key depressed before Record and released during the take produces a
   note-off with no matching note-on, which decrements rule 5's held counter
   below zero — and if that counter is unsigned it **wraps**, after which the
   stop sequence tries to emit ~4 billion note-offs. The rule: the counter
   **saturates at zero**, an unmatched note-off is written as-is (it is what the
   player did), and the pre-`T0` note is *not* synthesised at tick 0 — a note
   whose attack was never recorded should not be given a fake one. Test A gains
   "a note held across `T0`" beside its existing "a pedal already down at `T0`".

At stop, in this order: one note-off per outstanding on, then pedal releases
(pedal-up must be **at or after** the note-offs or a reader re-releases notes
meant to ring), then `EndOfTrack`. **Do not extend the take to fit them** —
`T1` is the stop instant, and if the MIDI runs four seconds longer than the audio
and video, every downstream tool that trusts durations gets it wrong.

Content: note on/off **with release velocity preserved** (a `0x8n` note-off
carries it and Yamaha/Kawai actions send it meaningfully), CC64 as a
**continuous 0–127 value, not a boolean** (half-pedalling sweeps the full range;
`midi_event.rs:35` collapses it at 64 and the recorder must not), CC66/CC67,
every other CC, pitch bend, both aftertouches, program change, and SysEx
verbatim.

---

## 8. Plugin hosting

The owner's choice, made explicitly: *host any instrument plugin*. This section
exists to say what that costs and to keep it from blocking everything else.

### The spike ran on 2026-08-16, and it passed

Written before any of the design below was committed to, because "can a real
VST3 be loaded in-process from Rust at all" is the question that would have
invalidated everything else. `ivory-host/src/scan.rs` and
`cargo run -p ivory-host --example scanvst`.

**Result: 112 of 112 plugins on this machine loaded. Zero failures.** Including
**Pianoteq 9** (Modartt), which reports three classes — `Plugin Compatibility
Class`, `Audio Module Class`, `Component Controller Class` — exactly as the ABI
says it should. `vst3` 0.3.0 pulled in one transitive dependency
(`com-scrape-types`) and no build script, confirming the pre-generated-bindings
claim that the cross-build depends on.

Four things the spike settled that reading could not:

1. **`bundleEntry(CFBundleRef)` is mandatory on macOS and is the whole trap.**
   `dlopen` + `GetPluginFactory` is the Windows/Linux sequence and it is not
   enough here. A real `CFBundleRef` for the `.vst3` directory has to be built
   with `CFBundleCreate` and handed over first. Plugins do real work in there —
   locating sample and preset directories relative to the bundle — so skipping
   it yields a factory pointer that fails later rather than an error now.
   `CoreFoundation` also has to be named in a `#[link]` attribute; Rust links
   `libSystem` and not CF, and the symptom is a linker error naming
   `_CFBundleCreate` with nothing pointing at the cause.
2. **Modules must never be unloaded.** `Library::drop` is a deliberate,
   documented no-op. Plugins spawn threads and register atexit handlers during
   `bundleEntry`; unmapping the library from under a running thread is a
   use-after-free whose stack trace points into someone else's code. Every
   mature host either keeps modules resident for the process lifetime or
   sequences teardown very carefully. A recorder loads one piano and keeps it.
3. **SCANNING MUST BE OUT OF PROCESS, even though hosting can start in
   process.** This is a design change the spike forced. Loading all 112 in one
   process produced **seven Objective-C duplicate-class warnings** — Universal
   Audio ships `UAFlippedNSView`, `UANavDel`, `UAPluginWebView`,
   `UAScriptMessageHandler`, `Ntwk_BrowserDelegate_Mac` and others in *every*
   one of its bundles, and the runtime says so itself: "This may cause spurious
   casting failures and mysterious crashes." **`RTLD_LOCAL` does not help** —
   Objective-C class registration is global regardless of the `dlopen` flags,
   which is worth knowing before someone tries to fix it that way. A scanner
   opens every plugin by definition, so the scanner is where this bites. Run it
   in a helper process, cache the results, and re-scan only on change. That also
   makes a plugin that crashes on load a cache entry rather than a crash of
   Tangent, which is most of the value of out-of-process hosting for a tenth of
   the work.
4. **A crash in a hosted plugin is still a crash of the take.** In-process
   hosting stays the v1 plan for the *instrument* (one plugin, chosen
   deliberately, kept resident), but §8's out-of-process note is now backed by
   an observed hazard rather than a general principle.

### Spike 2, same day: a plugin instantiated and rendered audio

`ivory-host/src/instance.rs` and
`cargo run -p ivory-host --example playnote -- Pianoteq`.

**Pianoteq 9 rendered a middle C from Rust**, peak 0.342 / RMS 0.046, written
through `ivory-record`'s own BWF writer and played back — which also proves the
two lanes compose, since the host and the recorder were built independently.

The sequence, which is a state machine plugins enforce rather than a list of
calls: `createInstance` → `initialize(host)` → `queryInterface(IAudioProcessor)`
→ `setupProcessing` **before** `setActive` → `activateBus` on every bus →
`setActive(true)` → `setProcessing(true)` → `process`. Teardown runs it backwards
in `Drop`, so a `?` mid-setup cannot leave a plugin active with no owner.

Three findings, each of which would have shipped as a bug:

1. **`numOutputs` must cover EVERY activated output bus, not the one you intend
   to read.** This is what silence looks like: Pianoteq exposes **eight** stereo
   outputs, all activated, and handed `numOutputs: 1` it wrote nothing at all
   **and still returned `kResultOk`**. Not an error, not a warning — a
   correct-looking call producing a silent file. Every multi-output instrument
   (Kontakt, Omnisphere, any drum machine) has this shape. Buses past the first
   get separately-allocated scratch, because pointing several at one buffer
   invites a plugin that *sums* into its outputs to accumulate eight times into
   the one being read.
2. **PLUGINS LOAD ASYNCHRONOUSLY AND RENDER SILENCE UNTIL THEY ARE READY.** The
   sweep across the owner's own instruments, note played immediately after
   instantiation:

   | plugin | cold | after 5 s warm-up |
   |---|---|---|
   | Pianoteq 9 | **0.342** | — |
   | Analog Lab V | **0.224** | — |
   | Augmented GRAND PIANO | 0.003 | **0.167** |
   | CP-70 V | 0.0007 | **0.143** |
   | Piano V3 | **0.000** | **0.217** |
   | Stage-73 V2 | **0.000** | **0.198** |

   Four of six are silent or near-silent cold and all four are fine five seconds
   later. A recorder that instantiates on Record and starts capturing would
   produce a silent take from most of this library, and the user would
   reasonably report it as "Tangent doesn't work with my piano".

   **The rule this forces is the one §3 already has for the camera:** instantiate
   the plugin when the Recorder view opens, not when Record is pressed, and keep
   rendering into a discarded buffer until it produces output. Arming must be
   gated on the instrument being *ready*, with the UI saying so — not on the
   instrument being *loaded*. That is a real UI state, not an internal detail.
3. **VST3 velocity is a float 0.0..=1.0, not a MIDI byte.** Passing 100 makes
   every note fortissimo and clipped. Cheap to get wrong, and it sounds like a
   plugin problem.

### Milestone, 2026-08-16: a complete take, recorded from a hosted plugin

`cargo run -p ivory-host --example record_plugin -- Pianoteq`

A four-chord ii-V-I played into Pianoteq 9 produced a real take directory — a
1.2 MB 24-bit BWF `.wav`, a 202-byte format-1 `.mid`, and a `take.json` — with
peak 0.61. **This is the first point at which the two lanes are one product:**
the host and the recorder were written independently, by different workstreams,
and this is them composing.

The part worth defending is that the performance is *timed* rather than
convenient. Chord starts are 0 / 913 / 1847 / 2791 ms, deliberately **not**
multiples of the 512-frame block — 913 ms is 85.59 blocks — and every event is
placed by `Timeline::file_sample`, the same function that decides where it lands
in the `.mid`. A demo that fires notes at block boundaries proves nothing about
sync, and sync is the feature.

**And it is verified, not asserted.** `ivory-host/tests/plugin_take_sync.rs`
(`#[ignore]`d, needs a real instrument) asks whether the exported audio has an
attack at every instant the exported MIDI names:

```
      0ms  before 0.00000  after 0.15937  ratio inf
    913ms  before 0.01904  after 0.15683  ratio  8.2
   1847ms  before 0.01646  after 0.16185  ratio  9.8
   2791ms  before 0.01389  after 0.16124  ratio 11.6
```

Two things about that test are worth keeping:

- **It asks the question backwards on purpose.** The obvious check — detect
  onsets, compare to the MIDI — does not work on a piano. Chords are voiced with
  milliseconds of finger spread, sympathetic strings ring, and a rising-edge
  detector fires on beating between partials long after the attack; measured
  here it reported six "onsets" in the first second of a four-chord progression.
  Asking "at each instant the MIDI names, did the energy jump?" is both simpler
  and stricter: late, early and missing notes all fail it, and a ringing chord
  does not.
- **It was proven able to fail.** Delaying only the audio path by 300 ms while
  leaving the `.mid` alone turns every ratio to ~0.9 and the test reports the
  disagreement. A sync test that has never been shown to fail is decoration.

One incidental finding: the first Audio Module Class on this machine
alphabetically is `Ample Bass J`, and a bass sampler asked to play a C5 chord
legitimately produces almost nothing — so the test failed for a reason that had
nothing to do with sync. It now prefers a piano and honours
`TANGENT_TEST_VST3=<substring>`. Worth remembering for any future "run it
against whatever is installed" harness.

**Scope, honestly.** `clack`'s own `host/examples/cpal` is a complete
cross-platform CLAP host at **2,182 lines across 9 files** and does not include
state persistence. A minimal-but-correct **VST3** host written on `vst3` 0.3.0
means COM lifetime management, `IPluginFactory` / `IComponent` /
`IEditController` / `IAudioProcessor` / `IConnectionPoint`, the
`IComponentHandler` callbacks, bus arrangement negotiation, `IParameterChanges`
and `IEventList` marshalling, plus **three separate native child-window
implementations** and the `IPlugFrame` resize handshake — because
`iplugview.h` offers exactly one path, `attached(void* parent, FIDString type)`,
with no floating-window mode. **6,000–12,000 lines; 3–6 months part-time.**
Leaning on `vst3-host` 0.9.0 cuts that to weeks at the cost of depending on an
eight-week-old single-author crate for the most crash-prone subsystem in the
product.

**Two structural decisions that make this survivable.**

1. **CLAP first, VST3 second — but ship neither before the recorder works, and
   do not expect CLAP to defer the window problem.**

   > **Corrected by review, and this changes the rationale.** The first draft
   > said CLAP's `gui.create(..., is_floating: true)` means "zero NSView/HWND/
   > XEmbed code" and that CLAP-first defers "the hard part (three native window
   > embeddings)". **Floating is the exceptional path and is host-negotiated,
   > not host-chosen.** Upstream `clap/ext/gui.h` says at the top of the file:
   > "The Embedding protocol is by far the most common, supported by all hosts
   > to date, and a plugin author should support at least that case."
   > `is_api_supported(..., is_floating)` and `create(..., is_floating)` both
   > return a **bool the plugin controls**. And the plugin frameworks refuse it:
   > nih-plug at the exact revision this repo already pins (`28b149e`) has
   > `// We don't do standalone floating windows` / `if is_floating { return
   > false; }` (`wrapper/clap/wrapper.rs:2583-2586`), writes
   > `*is_floating = false` in `get_preferred_api` (`:2625`), re-checks in
   > `ext_gui_create` (`:2636-2638`), and returns false from `set_transient`.

   CLAP still goes first, but for the reasons that survive: `clack-host` is
   feature-complete and MIT, the extension set is small and legible, and it
   proves **the audio graph of §4a, the MIDI feed, state persistence and the
   plugin-source UI** — which is the majority of the risk that is not
   window-shaped. Budget the native window embedding in the CLAP phase, not
   after it. `raw-window-handle 0.6.2` already matching eframe's is what makes
   that embedding cheap to *start*; it does not make it free.

   Shipping CLAP alone would be engineering a pianist cannot use — there are
   almost no free CLAP pianos. Shipping it first is still the cheapest way to
   de-risk VST3, just by less than the first draft claimed.
2. **Out of process, eventually; in process, with a watchdog, for v1.** A plugin
   crash currently takes Tangent and the user's take down with it. `vst3-host`
   ships an opt-in helper binary for exactly this reason. v1 is in-process and
   says so; the crate boundary is drawn so that moving it out later does not
   touch the recorder.

**The macOS blocker that applies to every format, and it is two entitlements,
not one.** Under `--options runtime`, library validation is on by default and
blocks loading any dylib not signed by the same Team ID — whether or not the
plugin is itself notarized. But `disable-library-validation` alone is not
enough, and the review proved the rest on this machine with a hardened host
`dlopen`ing a foreign-signed dylib:

| signing | result |
|---|---|
| runtime, no entitlements | `dlopen` fails, "different Team IDs" — confirms the first claim |
| runtime + `disable-library-validation` **only** (what the first draft prescribed) | loads, then the plugin's `MAP_JIT` mmap fails with **EINVAL** |
| runtime + `disable-library-validation` + `allow-jit` | works |

So hosting needs `com.apple.security.cs.disable-library-validation` **and**
`com.apple.security.cs.allow-jit` (or `allow-unsigned-executable-memory`).
Almost every serious instrument plugin JITs — that is what a modern sampler's
DSP graph and any plugin with a scripting layer does — so this is the normal
case, not an exotic one. The failure mode is opaque enough to burn a day: the
plugin loads, reports success, and then dies inside its own allocator.

Two further facts worth writing down now:

- **Signing the plugin dylib with `allow-jit` is a no-op.** `codesign` accepts
  it, but `codesign -d --entitlements` then shows the dylib carrying no
  entitlements at all. In-process plugins run under the **host's** entitlements.
- **Which cuts the other way too: an in-process plugin inherits Tangent's camera
  and microphone entitlements.** A hosted third-party binary gains the ability to
  open the user's camera, silently, under Tangent's TCC grant. That is a real
  argument for the out-of-process helper in decision 2 above, and it is worth
  stating in the release notes when hosting ships.

**None of these three entitlements is required for the camera or the microphone,
and none must be added until hosting ships.**

**And the thing the owner may actually want.** If plugin hosting turns out to be
scope the project cannot carry, `rustysynth` 1.3.6 (MIT, **zero transitive
dependencies**, ~185 KB of source) plus **SalC5Light** (SF2, 24.5 MB, **public
domain**, 7 velocity layers, the same Yamaha C5 source as the celebrated
Salamander library) gives a real piano sound in 1–2 weeks and ~400–600 lines,
and guarantees that a recording has audio on day one with no user configuration.
Note `rustysynth` **rejects SF3** by design (there is a test named
`test_load_reject_sf3`), so the 12.6 MB compressed MuseScore font is not an
option; and `oxisynth`, which does support SF3, is **LGPL-2.1** and would
contaminate the MIT story. This is written down as a fallback, not as the plan.

---

## 9. Output contract

**Directory name:** `YYYY-MM-DD_HHMMSS[_slug]`, e.g. `2026-08-15_143207_nocturne`.
ASCII sort equals chronological sort. **No colons** — illegal on Windows, and on
macOS the Finder still swaps `/` and `:` at the presentation layer. **No spaces**
— these paths get pasted into shells and drag targets.

Slug sanitisation: strip `< > : " / \ | ? *` and `\x00-\x1F`; strip leading `.`
so the folder is never hidden; collapse runs of whitespace and punctuation to a
single `-`; trim trailing `-`, `_`, `.` and space, because **Windows silently
strips trailing dots and spaces during path normalisation** and a name ending in
one becomes a name you cannot reliably reopen; reject `CON PRN AUX NUL COM0-9
LPT0-9` case-insensitively *including with an extension*; truncate on a **char
boundary**, not a byte index, to ~40 chars.

**Creation is two calls, and the first draft only had the second one.**

```rust
fs::create_dir_all(&root)?;          // the destination root: ~/Movies/Tangent
fs::create_dir(&root.join(&take))?;  // the take: must be atomic, must not be _all
```

`~/Movies/Tangent` **does not exist on a fresh machine** — verified here:
`~/Movies` exists, `~/Movies/Tangent` does not. `create_dir` on a path whose
parent is missing returns `NotFound` (os error 2), not `AlreadyExists`, so the
first take on any new install would have failed with "No such file or directory"
naming the *take* folder and hiding the real cause. Windows is the same
(`Videos\Tangent`), and Linux is worse: on a minimal install `$XDG_VIDEOS_DIR`
itself is often absent, so even the grandparent may be missing.

Create the root **when the output directory is chosen**, not at Stop, so a
permissions failure is reported while the user is looking at a file picker
rather than after a 20-minute take.

**The take directory stays `create_dir`**, and that is the part the first draft
got right: it errors `AlreadyExists`, whereas `Path::exists()` then
`create_dir_all` is TOCTOU and `create_dir_all` happily succeeds on an existing
directory — silently writing a second take's files over a first take's. Retry
`-2`, `-3` … to 99 **on `AlreadyExists` only** (never on `NotFound`, or a missing
parent becomes a 99-iteration loop), then fail loudly. Real collision causes: NTP
stepping backwards, DST, a crash-recovery re-import, a scripted test, a user
hammering record after a 0.4 s take.

**Validate `MAX_PATH` at Change-folder time, not at stop time.** Budget ~155
chars; refuse a folder if `chosen_dir.len() + 130 > 250` with a message, rather
than discovering it after a 20-minute take.

**Media files carry the take name; the sidecar and log do not.** Constant
`video.mp4` names are nicer to script and wrong here, because these files get
separated from their folder constantly — dragged into a timeline, attached to an
email, uploaded. Ten tabs called `video.mp4` is the failure mode. `take.json` and
`take.log` keep constant names because they are only ever read in place.

**The video's audio is encoded from the same sample buffer as the WAV**, never
from a second capture, so there is exactly one audio clock in the system.

`take.json` carries the manifest and the sync report: `T0` as ISO-8601 and as
raw monotonic nanos, `R_nominal` and the measured `R_true` with ε in ppm,
per-source anchor and fitted rate, frames expected versus received, the tempo
written into the SMF, peak dBFS and a clip flag, the plugin name and preset if
one was used, and `"complete": false` written at take start and flipped at clean
stop. **That flag is the crash detector**, and the whole file is the only thing
that will let a user's "it's out of sync" report be debugged without owning
their machine.

### Crash safety

The organising principle: **every take must be recoverable from whatever bytes
reached the disk, with no dependency on a clean shutdown path ever running.**
OBS's own documentation is the counterfactual — "the whole file becomes
unusable".

- **MP4** needs a `moov` atom not known until the end. Write fragmented
  (`frag_keyframe + empty_moov` equivalent on each backend) so worst-case loss is
  one GOP, and remux to a plain MP4 on clean stop.
- **WAV** has 32-bit size fields normally patched at close; a WAV whose header
  says 0 plays as empty. **Patch the two size fields every few seconds** — two
  `pwrite`s at fixed offsets, no seeking of the write cursor. At 48 kHz/24-bit
  stereo the RIFF 4 GiB ceiling is ~4.1 hours; warn past 3, do not fail.
- **MIDI** buffers in RAM (a dense 20-minute take is well under 1 MB) and is
  written at stop; also flush a recovery journal so a crash leaves a
  reconstructable file.

Four scenarios, each with a defined outcome: camera in use by another app →
refuse to arm, name the app if the OS says, offer to record audio+MIDI only;
disk fills at minute 18 → stop cleanly at the last complete frame, finalise
everything, keep the take; audio device vanishes → as §4; crash mid-take → the
next launch sees `"complete": false`, finalises what is there and offers it.

---

## 10. Build, sign, package

**Everything in this list is required and none of it is cosmetic.**

1. **`build/macos/tangent.entitlements`** — a new file:
   ```xml
   <key>com.apple.security.device.camera</key><true/>
   <key>com.apple.security.device.audio-input</key><true/>
   ```
   plus `com.apple.security.cs.disable-library-validation` **only when plugin
   hosting ships**, never before.
2. **`scripts/build-macos.sh:217-218`** gains `--entitlements` on **both**
   `codesign` calls, inner executable and bundle. Then re-notarize.
3. **The Info.plist generator at `build-macos.sh:171-192`** gains
   `NSCameraUsageDescription`, `NSMicrophoneUsageDescription` and
   `NSCameraUseContinuityCameraDeviceType`. Missing the first two is a **crash**,
   not a denial.
4. **The ad-hoc fallback path (`build-macos.sh:224`).** *Corrected by review —
   the first draft said to add entitlements here, which is a no-op.* Line 224 is
   `codesign --force --deep -s - "$APP"`: it sets **no `--options runtime`**, so
   CS_RUNTIME is absent and a hardened-runtime entitlement added there has zero
   effect. Proven on this machine with a `dlopen` probe: no-runtime + the
   entitlement is indistinguishable from no-runtime + nothing. The real
   divergence is that local ad-hoc builds are **not hardened at all** while
   releases are, so an ad-hoc build can do things the shipped app cannot. Either
   add `--options runtime` *and* the entitlements to that branch so it matches
   the release, or leave it alone and stop claiming it is a valid rehearsal.
   Recommended: add both, because §14's dev-loop rule depends on the local
   bundle behaving like the shipped one.
5. **`scripts/check-firewall.sh` — the fix is `--target`, not more names.**
   *Corrected by review.* Lines 25 and 52 run `cargo tree -p "$crate" --edges
   normal --prefix none` with no `--target`, and **`cargo tree` filters to the
   host platform by default**. Proof already in this repo: `ivory/Cargo.toml:64`
   declares `windows-sys` under `[target.'cfg(windows)'.dependencies]`, and the
   script's exact idiom finds nothing on this Mac while `--target all` finds it
   (43 crates versus 53 for `ivory-ui`). So a Windows-gated camera crate added to
   `ivory-ui` would pass the firewall on the owner's machine forever. Add
   `--target all` **first**, then add `cpal`, `vst3`, `objc2`, `windows`,
   `nokhwa`, `ivory-record` and `ivory-host` to the list. Of those, only
   `windows` is currently invisible without the flag — but it is the one that
   matters, since the Windows backend is the one nobody can test locally.
6. **`THIRD-PARTY-LICENSES` is regenerated by nothing and checked by nothing —
   and it is ALREADY STALE IN SHIPPED 3.0.0.** *This is a live defect the review
   found in the repo, not in this feature.* Verified here:
   - `build-macos.sh:129`, `build-cross.sh:77` and `build-linux-native.sh:34` all
     guard with `[ -f THIRD-PARTY-LICENSES ] || scripts/gen-third-party-licenses.sh`.
     The file is git-tracked, so the guard is always satisfied and **the
     generator never runs**. `release.sh` checks artifacts for `README.txt`,
     `LICENSE` and `OFL.txt` (`:172-174`) and never for this file.
   - The committed file lists `toml 0.5.11` (line 1437) and `winres 0.1.12`
     (line 1907). **Neither is in `Cargo.lock`** (`grep -c` returns 0 for both).
     The lock has 400 packages minus 3 workspace members = 397 third-party
     crates; the file's trailer says **`(399 crates)`** — exactly the two
     phantoms. Tangent 3.0.0 shipped that on all three platforms.

   The current error direction is over-listing, which is harmless. **This feature
   inverts it**: cpal (Apache-2.0), openh264-sys2 (BSD-2 + vendored Cisco), yuv
   (BSD-3), midly (Unlicense), the objc2 family (Zlib/Apache/MIT), linuxvideo
   (0BSD), vst3 and clack — all attribution-requiring, all invisible in the
   shipped file, with no script able to notice. Required:
   - **(a)** drop the `[ -f … ] ||` guard in all three build scripts, or add a
     `release.sh` gate that regenerates into a temp file and **fails on any
     diff** against the committed one. The generator writes to a fixed
     `OUT="THIRD-PARTY-LICENSES"`, so a diff-check needs an overridable output
     path first.
   - **(b)** hand-add openh264/openh264-sys2's vendored Cisco tree, since the
     generator reads `Cargo.lock` and cannot see vendored C++.
   - **(c)** the skip-set edit — and note the first draft's "both branches" was
     wrong. Only the `cargo-license` branch has a skip set (`:83`); the
     `cargo metadata` fallback (`:101-102`) filters against `workspace_members`
     and picks up new members automatically. `cargo-license` is not installed on
     this machine, so the branch that actually runs is the self-maintaining one.
   - **(d)** check whether Apache-2.0's NOTICE requirement is handled at all.
     cpal would be the first Apache-2.0-only runtime dependency in the tree.
7. **`nasm` is in no dependency list, and openh264 fails soft.** *Corrected by
   review — the first draft's "the remote-build path is already the only Linux
   route" was false and made this look handled.* `build-linux-remote.sh:127`
   sshes over and runs `build-linux-native.sh`, and `release.sh:98` runs
   `build-linux-native.sh` **directly** whenever `uname -s` is Linux. The
   dependency lists that matter are `build-linux-native.sh:11-19` and the three
   `DEPS=1` package lists at `build-linux-remote.sh:78` (xbps), `:82` (apt) and
   `:86` (dnf). **None of the five contains `nasm`**, and `base-devel` /
   `build-essential` / `gcc gcc-c++ make` do not supply it. openh264-sys2's
   `build.rs:233-241` does `let Ok(object_files) = nasm_build...` and **silently
   falls back to the C++ path on failure** — no warning, no error, a ~3× slower
   encoder, and 1080p30 stops making realtime on a laptop. Add `nasm` to all four
   lists **and** assert its presence in `build-linux-native.sh`, because a silent
   3× regression is exactly the kind of thing that ships.
8. **TCC and the cert leaf — deleted.** *The first draft asserted that adding
   entitlements changes the code-signing requirement and strands TCC grants. That
   is false, and the review disproved it by experiment:* signing
   `dist/Tangent.app` with and without the camera/audio entitlements produces
   **byte-identical requirement blobs** (`codesign -d -r`, sha256 `9fd42590…`
   both times); only the CDHash changes. The designated requirement is
   `identifier "org.codeberg.ganten1998.ivory" and anchor apple generic and
   certificate 1[…6.2.6] and certificate leaf[…6.1.13] and certificate
   leaf[subject.OU] = "6GFLLR9599"` — bundle id, Apple anchor, Developer ID
   marker OIDs, team OU. **No entitlement term, and no specific leaf.** So
   entitlements and even certificate renewal leave Developer ID TCC grants valid.
   The leaguecage precedent does not transfer: that app's requirement named its
   own self-signed leaf.

---

## 11. Verification

Sync is not verifiable by watching a video, and it must not depend on a human.

**Test A — the arithmetic. Headless, `cargo test`, no devices. 90% of the real
bugs live here.** Put the timeline behind `AudioSource`, `VideoSource` and
`MidiSource` traits — the `MidiPorts` pattern already establishes both the shape
and the justification — and drive them from a fake that synthesises an
adversarial take: audio nominal 48000 but true 48002.4 (+50 ppm) for 20 minutes;
video at 30 fps with the first frame 743 ms late, a 3-frame drop at t=612 s and
±4 ms jitter; MIDI with two out-of-order arrivals, a re-triggered note, a pedal
already down at `T0`, and a note plus pedal still held at stop.

Assertions: every MIDI event's absolute tick, converted back through the
*written* tempo and PPQ, within **1 ms** of truth **both at t=0 and at
t=1200 s** — the second is the drift test, and without the `(1+ε)` correction it
fails by 60 ms and nothing else in the suite notices; every frame PTS within half
a frame; `EndOfTrack` equal to the audio sample count in ticks ±1; exactly one
note-off per outstanding on at the stop tick with pedal release at or after;
program and pedal snapshotted at tick 0; no delta over `u28::MAX`; and
`midly::Smf::parse` reads the file back and re-serialises **byte-identically**.
Runs in about a second with no hardware.

**Test B — the real pipeline, proven from the artifacts.** Inject a
synchronising slate into all three streams at one known instant, gated behind
`IVORY_RECORD_SLATE=1` in the repo's existing `IVORY_*` convention. Audio:
electrically through a loopback device, never acoustically. Video: a synthetic
camera behind the `VideoSource` trait emitting **binary-coded burnt-in
timecode**, decoded back out of the *exported* file — which is the strongest
available test, because a bare PTS check cannot catch a pipeline that reorders,
duplicates or drops frames while keeping the PTS sequence tidy. MIDI: a virtual
output via `midir::os::unix::VirtualOutput`, which exists on every platform
**except Windows** — say so rather than pretending otherwise.

Measure: cross-correlate the muxed audio against the WAV (peak lag 0 samples,
normalised peak > 0.999, searched over ±200 ms with parabolic interpolation);
click onset in the WAV versus the slate note's tick through the file's own tempo
(|Δ| < 1 ms, justified by MIDI's own 0.96 ms serial transmission); decoded
timecode versus the same note (< half a frame); and **slate at both the start
and the end of a 20-minute take**, because a pipeline with no drift correction
passes every other assertion and fails only that one.

`#[ignore]`d and script-driven, exactly as `blast_radius.rs` already is.

---

## 12. Ordered steps

Each step is independently committable and leaves the app shippable.

> **Reordered twice.** First after review, because audio capture sat two steps
> before the entitlements it needs. Then again on 2026-08-16, when the owner
> asked for whatever delivers the **finished** product soonest without costing
> quality — which is a different question from what the risk-ordered plan below
> was answering, and has a different answer.
>
> **The critical path is the VST3 host.** It is 6,000-12,000 lines and three
> native child-window implementations; every other item here is weeks. A long
> pole scheduled last does not overlap with anything, so every day it is not
> started is a day added to the finish, one for one. Scheduling it *last* was
> right for "get something usable soonest" and is wrong for "get everything
> soonest".
>
> **And the machine settles the format question.** This Mac has **112 VST3
> plugins**, including **Pianoteq 9** (universal arm64 + x86_64), Piano V3,
> Augmented GRAND PIANO, CP-70 V and Stage-73 V2 — alongside 92 Audio Units of
> the same Arturia library. So VST3 is both the cross-platform answer and the
> one the owner can test on day one, and CLAP's only remaining argument (that
> it is cheaper) is now moot: there are zero CLAP plugins installed and the
> review already killed the "floating GUIs mean no window code" shortcut. **VST3
> first, CLAP later or never.**
>
> The revised shape is three lanes that run concurrently rather than nine steps
> that run in series. Lane 1 is the long pole and starts immediately. Lanes 2
> and 3 fit inside it. The seams between them — the `VideoSource` /
> `VideoSink` / `MidiPorts` traits, `Caps`, the §4a audio graph and
> `paint_composite` — are all specified above, which is what makes concurrency
> safe rather than merely fast.
>
> | lane | contents | gated on |
> |---|---|---|
> | **1. Host** (critical path) | the spike, then COM plumbing, buses, MIDI in, audio out, state, then the three window backends | the entitlements, for loading anything at all |
> | **2. Capture** | audio + WAV, take directory, camera ×3, encode ×3 | the entitlements, for testing anything at all |
> | **3. Surface** | Recorder band, menu row, compositor, Export dialog | lane 2's camera for the preview only |
>
> **Step 0 is now the entitlements**, and it absorbs the hosting keys at the same
> time. That is the one genuine ordering win available: camera, microphone,
> `disable-library-validation` and `allow-jit` all land in one file, so the app
> is signed and notarized **once** for the whole feature rather than twice, and
> lane 1 is not blocked waiting for a signing change that lane 2 was going to
> make anyway.
>
> The numbered steps below keep their numbers so nothing that references them
> goes stale; read the lane table for what runs when.

**Step 1 — the raw MIDI tap and the clock. No UI, no devices, no permissions.**
Always-on tap out of `midi.rs` yielding `(stamp_µs, Vec<u8>)`,
`Ignore::TimeAndActiveSense`, `clock.rs` (including the per-source `latency_ns`
term from §3a, even though every value is 0 at this stage), `smf.rs`, and **Test
A in full**. Pure computation. This is the highest-value, lowest-risk step and it
de-risks the hardest correctness problem in the feature before anything
platform-specific exists.

**Step 2 — the entitlements, the plist, and the dev loop.** *Moved ahead of
audio.* The new `build/macos/tangent.entitlements`, `--entitlements` on both
`codesign` calls, the three `NS*` plist keys, and `--options runtime` added to
the ad-hoc branch so a local build rehearses the shipped one. Ships on its own,
before any device code exists, so the signing change is proven in isolation.
Verify with `codesign -d --entitlements -` on the built bundle. **Establish the
build-`.app`-and-`open` dev loop here**, because from Step 3 onward `cargo run`
stops being a valid test.

**Step 3 — the take directory.** Root creation with `create_dir_all`, take
creation with `create_dir`, naming, sanitisation, `take.json`, `take.log`,
crash-safe finalisation, the recovery path. Unit-tested against every Windows
reserved name and every path-length edge. No devices, so it can be written
alongside Step 2.

**Step 4 — audio capture and the WAV.** `cpal` in, `rtrb`, writer thread,
hand-rolled BWF writer, meters, and the §4a clock rules for the no-input-device
case. Drive it from a `#[ignore]`d integration test and a `--record-test` CLI
flag. **This is the first step that requires a signed bundle to test at all.**

**Step 5 — the camera, macOS only.** `VideoSource` trait with `stride` and
`latency_ns`, AVFoundation backend, Continuity Camera (plist key *and* the
device-type array), the deployment-target weak-linking, BGRA→RGBA for the
preview, the preview texture, `MenuAction::ToggleRecorder` and the keys binding,
and the Recorder band in its idle layout. **First point at which the owner can
see something** — which is also why the menu row cannot slip to a later step.

**Step 6 — video encode and mux, macOS.** `AVAssetWriter`. A take now produces a
real synced `.mp4`. **Test B on macOS**, plus the §3a camera-latency calibration,
which is the one thing no automated test covers.

**Step 7 — the compositor.** `ivory-ui::app::paint_composite` and the `&self`
refactor of the band arithmetic, `ivory/src/composite.rs` with the offscreen
context, FBO, PBO readback **and the vertical flip**, the composition layouts,
and the Export dialog with its selector. This is the step that delivers the
headline deliverable.

**Step 8 — Windows, then Linux.** Media Foundation capture and sink (with the
`× 100` on `MFSampleExtension_DeviceTimestamp` and the N-edition detection); V4L2
and openh264 (with `nasm` in every dependency list and asserted at build time).
Windows is the schedule risk: it is cross-compiled from macOS with no Windows box
in the loop. **Linux ships `.mov` + LPCM, not `.mp4` + AAC** — say so in the UI.

**Step 9 — the plugin host.** The §4a audio graph made real: source selector,
mixer, gains, monitoring, PDC. Then CLAP via `clack` (budgeting native window
embedding *inside* this phase, not after it), then VST3 via `vst3` 0.3.0, then
`disable-library-validation` **and** `allow-jit`, then re-notarization.

*(The first draft opened with a "Step 0: correct LICENSING.md". That edit was
already made in the same session and the item is deleted rather than left to be
done twice.)*

---

## 12a. Full and Minimal builds

The owner asked for the recorder to be an optional install — recommended and
default — with a **Minimal** build available *if the bundle becomes too big*.

**The size trigger will not fire, and the measurements say so.** Every dependency
this feature adds was built in isolation against the real release profile
(`opt-level = 3`, `lto = true`, `strip = true`) and diffed against an empty
binary:

| dependency | stripped delta |
|---|---|
| `midly` (SMF writing) | **+16 KB** |
| `cpal` (audio I/O) | **+16 KB** |
| `rtrb` (lock-free ring) | **+16 KB** |
| `vst3` (the whole VST3 ABI) | **+0 KB** |
| `openh264` (Linux only, Cisco C++ compiled from source and statically linked) | **+0.52 MB** |

`vst3` costing nothing is not a rounding error: its 670 KB `bindings.rs` is
almost entirely `#[repr(C)]` struct and vtable *declarations*, which generate no
code until called and dead-strip when unused. The OS-native encoders and the
camera add **zero payload** by construction — VideoToolbox, AVFoundation and
Media Foundation are system frameworks, and the `objc2` / `windows` crates are
binding layers with no runtime.

Against a shipped universal binary of **21.09 MB** (10.38 arm64 + 10.70 x86_64,
a 10.73 MB zip), the entire recorder plus plugin host is on the order of
**a few hundred KB of our own code**. Tangent goes from ~21 MB to ~22 MB.

**Measured end to end once both variants existed:** `cargo build --release -p
ivory` is **10.42 MB** and `--no-default-features` is **10.40 MB**. The Full
build is **16 KB larger than Minimal.**

Stated honestly, because the number will move: that is with the clock, the SMF
writer and the MIDI tap implemented, and it does not yet include the camera
backends, the encoders, the compositor or the host. Those are *our* code, so
expect the gap to grow to a few hundred KB — not to megabytes, because none of
them carries a payload. **The conclusion does not change: nothing here makes the
bundle too big, and a Minimal build justified on size would be justified on
16 KB.**

Even openh264 — the one genuine binary payload, and Linux-only, since macOS and
Windows call the OS encoder — measured at **0.52 MB**, not the 2-5 MB a C++
codec tree suggests. LTO strips the decoder half, which a recorder never uses.
That was worth measuring rather than estimating: the guess was off by 4-10x in
the direction that would have justified machinery nothing needs.

So the two things that could still make it big are both already rejected:
a bundled ffmpeg (30-70 MB per arch — §7) and a bundled SoundFont (24.5 MB —
not planned, §8). **Neither is in the plan, so the size trigger has no path to
firing.**

### So build the split anyway — for the reasons that are actually good

Size is the weakest argument for a Minimal build. Three better ones survive:

1. **Entitlements.** The Full build must ship
   `com.apple.security.cs.disable-library-validation` and `allow-jit` so it can
   host plugins. Apple warns that the first weakens Gatekeeper and subjects the
   app to extra checks. **A Minimal build ships no entitlements at all** — which
   is exactly what Tangent 3.0.0 does today. Someone who wants a chord display
   should not have to run a binary that can load arbitrary third-party code.
2. **Permissions.** Minimal has no `NSCameraUsageDescription` and no
   `NSMicrophoneUsageDescription`, so it can never prompt, never appears in
   Privacy & Security, and cannot open a camera even if asked. That is a real
   difference to a privacy-conscious user, and it is enforced by the absence of
   the code rather than by a setting.
3. **Blast radius.** Full loads foreign code in-process; a plugin crash is a
   Tangent crash. Minimal cannot.

### How it is built, and where the `cfg` is allowed to live

```toml
# ivory/Cargo.toml
[features]
default       = ["recorder"]
recorder      = ["dep:ivory-record", "dep:ivory-host"]

[dependencies]
ivory-record = { path = "../ivory-record", optional = true }
ivory-host   = { path = "../ivory-host",   optional = true }
```

**`#[cfg(feature = ...)]` goes in the `ivory` binary crate and NOWHERE else.**
That is not a preference; `ivory-ui` has a standing rule against feature cfgs
(PLUGIN-PLAN §1) precisely because they are the seed of the two-GUI fork the
egui 0.33 pin exists to prevent. So:

* **`ivory-ui` compiles the Recorder band in every build**, and it is switched
  off the way the plugin already switches off everything it cannot do — through
  `Caps`. A third const joins `DESKTOP` and `PLUGIN`:

  ```rust
  /// The standalone, built without the recorder. Everything the chord display
  /// needs; nothing that opens a device or writes a file the user did not ask
  /// for.
  pub const MINIMAL: Caps = Caps {
      capture_devices: false,
      write_user_files: false,
      native_file_dialogs: true,   // still has a real window and a real rfd
      ..Caps::DESKTOP
  };
  ```

  The recorder band's height is already specified as **exactly zero** when
  `!caps.capture_devices` (§5), so the layout, the geometry tests and the menu
  gating all work unchanged. The menu row is absent rather than inert, which is
  the same rule the plugin follows.
* **The `ivory` binary** does not link `cpal`, `vst3`, `objc2-av-foundation` or
  any camera code in a Minimal build. The exclusion is the compiler's, not a
  flag's — which is the same argument the `ivory-ui` firewall rests on.

### What ships

| artifact | entitlements | plist camera/mic keys | can host plugins |
|---|---|---|---|
| **Tangent** (default, recommended) | camera, audio-input, disable-library-validation, allow-jit | yes | yes |
| **Tangent Minimal** | none | none | no |

`scripts/build-macos.sh` takes a variant argument and selects the entitlements
file and the plist fragment from it. `build/macos/tangent.entitlements` is the
Full set; Minimal passes no `--entitlements` at all, exactly as 3.0.0 does.

**The installer offers Full by default.** The macOS `distribution.xml` already
models the app and the VST3 plugin as separate selectable choices, and the NSIS
script already has components — so this is a third choice in an existing
mechanism, not new machinery. Minimal and Full are **mutually exclusive**: they
are the same bundle id (`org.codeberg.ganten1998.ivory`, which must never
change) and the same settings file, so installing one must replace the other.
That exclusivity has to be stated in the installer, or a user ends up with two
Tangents and no way to tell which one Finder will launch.

**Cost, honestly:** two builds, two signings and two notarizations per platform
per release, roughly doubling `release.sh`'s wall time. That is the real price
of the split, and it is worth paying for the entitlement story rather than for
the megabyte.

---

## 13. Explicitly NOT in v1

- Editing, trimming, or any timeline. The take is what was played.
- Multi-camera, or any camera switching mid-take.
- Screen capture of other applications. `screencapturekit` exists if this is ever
  wanted; it is a different feature.
- Streaming, or any network path.
- An optional large sample-library download. Hosting, integrity checking and a UI
  for it are their own project.
- Effects, EQ, compression, or any processing of the recorded audio.
- Recording from the plugin *and* an input at different sample rates.
- A second SMPTE-division `.mid`. §7.

---

## 14. Traps, each with its prevention

| Trap | Prevention |
|---|---|
| Camera works in `cargo run`, fails in the shipped app (or the reverse) | TCC attributes to the responsible ancestor process. **The only valid test is a signed `.app` launched with `open`.** Build this into the dev loop from step 4, not step 5. |
| App notarizes, launches, finds no camera, reports nothing | The entitlements file. Assert its presence in `release.sh` with `codesign -d --entitlements -`. |
| Launch breaks on macOS 11–13 after the camera lands | objc2 emits macOS-14 device-type statics as non-weak imports. Link only 10.15 symbols; build 14+ type strings at runtime. |
| The **recorder band** makes the window taller than the screen | The band's height stays a pure function of width and the camera aspect is absorbed by `Painter::image`'s `uv` — 200 pt instead of 731. Assert it in the existing `app.rs` geometry tests. **Scoped by review:** this bounds the recorder's own contribution, not the stack's total, and nothing in the tree bounds the total. At 200% with theory and fretboard on, **shipped 3.0.0 is already 1264 pt tall** with no recorder at all. That is a pre-existing gap this feature discloses rather than introduces, and closing it (clamp `window_size_percent` against `monitor_size`) is its own small change that should not be smuggled in here. |
| `NSOpenPanel` is raised from inside an egui pass | *Corrected by review — the first draft's mechanism was wrong.* eframe calls `App::update` from **inside** `egui_ctx.run` (`native/epi_integration.rs:273-282`), so draining a request in `impl eframe::App for DesktopApp` after `self.0.frame(ctx)` returns is **still inside the egui pass**. The request pattern is still right for keeping `ivory-ui` free of `rfd`, but it does not by itself move the modal out of the frame. Use `rfd::AsyncFileDialog` and deliver the result back over a channel, so nothing blocks the main thread inside `run`. (Also correct the §0 row: `main.rs:138`'s dialog is in the **panic hook**, which fires wherever the panic does — overwhelmingly inside `IvoryApp::frame`. Only `:188` and `:252` are genuinely outside the loop. It survives today because with no parent set, rfd 0.15.4 routes it to the out-of-process `CFUserNotificationDisplayAlert` rather than a nested `NSRunLoop`.) |
| The plugin build gains a camera | Band height must be exactly 0 when `!caps.capture_devices`, or `fit_bands` shrinks the piano for a band the plugin never draws. Test it under `Caps::PLUGIN`. |
| 20-minute takes drift by ~60 ms | The `(1+ε)` map, and Test A's assertion **at t=1200 s specifically**. An assertion only at t=0 passes on a broken pipeline. |
| A `.mid` with hanging notes or a stuck pedal | The stop sequence in §7, counter-based held tracking keyed on `(channel, key)`, and the explicit tests. |
| A dropped frame becomes silent drift | Count two ways — expected `(T1−T0)·fps` versus received, **and** timestamp gaps > 1.5 frame intervals — and put both in `take.json`. |
| `set_partial` uploads three times in one pass | `TextureManager` de-duplicates full deltas only. Call it exactly once per pass, from the paint body. |
| Adding a convenient dependency to `ivory-ui` | `check-firewall.sh` needs **`--target all`** before it needs more names — see §10 item 5. Without it the script cannot see any target-gated dependency, which is exactly how a Windows-only camera crate would get in. |
| A GPL dependency reaching the root lock file | Every crate in §2 verified permissive. But `gen-third-party-licenses.sh` **never runs** (§10 item 6) and cannot see vendored C++ or a bundled binary regardless. Fix the regeneration gate before adding any dependency at all. |
| The composite comes out upside down, or with red and blue swapped | Both are certain, not possible: egui_glow's shader is unconditionally y-flipped, and its upload path is RGBA-only while macOS capture is forced to BGRA. §6 names both fixes. |
| A hosted plugin loads and then dies in its allocator | `disable-library-validation` is not sufficient; `allow-jit` is also required. §8. |
| The first take on a new machine fails with "No such file or directory" | `create_dir_all` the destination root when it is chosen; `create_dir` the take folder at Stop. §9. |

---

## 15. What the review refuted

Recorded so it is not re-raised. Each of these was filed as a defect by a
reviewer and then killed by a skeptic who checked it against the code or a
primary source.

| Claimed defect | Why it does not hold |
|---|---|
| "ε cannot be known when the video PTS that need it are written — a causality hole at the centre of the drift correction" | The fit is incremental and the PTS are written into a container that stores per-sample durations; ε converges within seconds and the residual is re-applied at mux. No hole. |
| "`T0 = max(...)` plus forcing first frame PTS 0 injects up to a full frame period of video-early error" | The error is bounded by half a frame interval, not a full one, and it is the correct trade against relying on edit lists that players ignore. |
| "The stop rule is self-contradictory: `T1` is after the last available audio sample" | `T1` is the stop *instant*; the rule already says audio runs through the sample nearest `T1` and video through the last frame ≤ `T1`. Consistent. |
| "`rtrb` cannot carry the tap payload: `Producer<T>` needs a sized `T` and SysEx is variable-length" | Real constraint, wrong conclusion — a fixed-size record with a length prefix, or a byte-oriented ring, carries it. The library does not dictate the payload shape. |
| "'Set as default' is two keys whose semantics contradict each other" | `record_dir` and `record_dir_is_default` are the path and whether it persists. Re-derived; no contradiction. |
| "Requirement 3 is never addressed" (filed twice, by two reviewers) | §8 addresses it explicitly and §1 states the scheduling consequence in a callout. Both filings mistook "deferred and labelled" for "dropped". |
| "egui_glow has no PBO helper and the Painter leaks unless explicitly destroyed" | The FBO bet is sound; the absence of a PBO *helper* is not a defect, and `Painter::destroy` is a documented, ordinary requirement. |
| "'All three platforms at once' is not delivered" | Sequencing is not descoping. §2 carries all three dependency blocks, §4 all three camera backends, §7 all three encoders, and §12 step 8 delivers Windows and Linux **ahead of** the plugin host. |
