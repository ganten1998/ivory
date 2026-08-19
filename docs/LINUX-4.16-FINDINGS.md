# Tangent 4.16.0 — Linux hardening report (dresden, 2026-08-19)

> **Status: both findings fixed in 4.17.0.** The report is kept verbatim below
> because its measurements are the argument, and one of them overturns advice
> this project had already written down.
>
> | Finding | Fix |
> |---|---|
> | 1 · dead file picker | `Dialog::FileBrowser` — an in-app directory listing, opened when `DesktopApp::native_dialogs_work()` says no portal offers `FileChooser` and `zenity` is not on `PATH`. No new dependency. Routes both the file and the folder pickers. The single-instance message box also prints to stderr now, as the panic hook already did. |
> | 2 · underruns | `BUFFER_PERIODS` — Linux asks for `Fixed(want * 4)`, so cpal's ALSA host lands on a `want`-frame period behind a four-period ring, which is the cadence every other platform already got. The Audio Status line names the ring where it differs from the period. |
> | 2 · realtime | **Not done, deliberately.** The measurement here overturns finding 1(c) of `LINUX-4.11-FINDINGS.md`, which asked for rtkit. Promotion made it an order of magnitude worse on this box. Recorded in §8 of HANDOFF so it is not "fixed" again later. |


Test box: 2012 MacBook Air (Ivy Bridge, Void Linux, XFCE+i3, PipeWire 1.6.7,
settled graph 44100 Hz / min-quantum 512). Both reported bugs diagnosed with
measured evidence. The fix does **not** mean giving up leanness — the Linux
build is accidentally running 4× leaner than designed, and the *intended*
configuration runs clean on this potato.

## 1. The dead "load audio track" file picker

`rfd` 0.15.4 on Linux only has two backends compiled in: xdg-desktop-portal,
then a zenity subprocess. The test box has neither (only a gnome-keyring Secret
portal impl exists), so `pick_file()` returns `None` — which rfd makes
**indistinguishable from the user pressing Cancel**, so the app can't even tell
it failed and silently no-ops. This also affects `rfd::MessageDialog` in
main.rs: early error boxes vanish silently on any portal-less box.

Local unblock on Void: `sudo xbps-install -S xdg-desktop-portal
xdg-desktop-portal-gtk`, plus `~/.config/xdg-desktop-portal/portals.conf`
containing `[preferred]` / `default=gtk` (XFCE needs it). zenity is no longer a
light fallback — on current Void it drags in GTK4 + libadwaita + **webkitgtk**.

The real hardening fix is app-side, and the pattern already exists:
`PluginPicker` in `ivory-ui/src/dialogs.rs` is documented as "a directory
listing and nothing more." Add an in-app egui file browser as the fallback,
probe availability up front (does `org.freedesktop.portal.Desktop` have a bus
owner — zbus is already in the dep tree via rfd — or is zenity on PATH), and
route to the in-app picker when neither exists. Zero new dependencies, works on
a box with nothing installed.

## 2. The underruns — measured root cause

cpal 0.16's ALSA host maps `BufferSize::Fixed(v)` to **ring = v, period = v/4**
(`cpal-0.16.0/src/host/alsa/mod.rs:1103`). So `WANT_BUFFER_FRAMES = 256`
produces a 256-frame total ring refilled every **64 frames (1.45 ms)** —
confirmed: the Tangent node runs at quantum 64 in pw-top. On macOS,
`Fixed(256)` means 256-frame callbacks; on Linux the same call asks for a
quarter of that, serviced by a plain SCHED_OTHER thread. The DSP itself is
already lean — ~35 µs busy per cycle, about 2.4% of even the 1.45 ms window —
so there is nothing to make leaner; it is purely a scheduling-cushion problem.

Four 30-second runs of the shipping 4.16.0 binary under identical 4×`yes` CPU
load, using the `IVORY_OUT_BUFFER` override already present in the binary:

| Config | Ring / period | RT? | Underruns / 30 s |
|---|---|---|---|
| Stock (`Fixed(256)`) | 256 / 64 | no | 6 (spread across run) |
| Stock + `chrt -f 70` on `cpal_alsa_out` | 256 / 64 | FIFO 70 | **75** — flood starts right after promotion |
| `IVORY_OUT_BUFFER=1024` | 1024 / 256 | no | **0** |
| `IVORY_OUT_BUFFER=512` | 512 / 128 | no | **0** |

The big surprise: the textbook fix — realtime priority on the audio thread —
makes things dramatically *worse* through pipewire-alsa. The run was clean
until the moment the thread was promoted, then underruns accelerated
continuously. The plugin's own `data-loop.0` already runs at RT 83; promoting
the client thread to FIFO 70 evidently inverts priority against the plugin's
non-RT IPC thread on this 2-core machine. **Do not add rtkit/RT promotion to
the Linux path.**

## 3. Source fix (one line, honors the "stay lean" constraint)

In `pick_buffer` / the stream open, on Linux request `Fixed(want * 4)`. This
does not change the buffer size in the sense that matters: the callback cadence
stays exactly the designed 256 frames (period = 1024/4 = 256, same as macOS),
and only the safety ring behind it deepens to the 4 periods ALSA always
assumes. Worst-case added output latency is ~23 ms at 44.1 kHz — and ×2
(512-frame ring, ~12 ms) also measured clean here with half the cushion.
`IVORY_OUT_BUFFER` remains the escape hatch either way. Consider showing the
period alongside the ring in the Audio Status panel, since the two now differ
on Linux.

**Until the rebuild:** `IVORY_OUT_BUFFER=1024 tangent` runs clean right now.

## Side observations

- The Mac packaging regression is fixed — the 4.16 Linux tarball ships
  `tangent-ffmpeg` (79 MB) again.
- The two `zbus` WARN lines per dialog attempt are rfd probing the absent
  portal; they disappear once a fallback exists.
