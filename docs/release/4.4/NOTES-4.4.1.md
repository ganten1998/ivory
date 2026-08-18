Tangent 4.4.1 is a Linux fix release. The macOS and Windows builds carry two
shared fixes that Linux exposed; everything else here is Linux only.

It was found the way these things are always found: by running 4.4.0 on the
machine it was built on, under a tiling window manager, and watching it fail.

## Linux

**The camera works.** 4.4.0 shipped a stub, so "select a camera" could never
succeed. There is now a V4L2 backend behind the same seam as the macOS one:
enumeration by stable bus identity, the shared format-choice policy, YUYV
converted directly and MJPEG decoded in pure Rust. MJPEG matters because most
UVC webcams offer their full frame rate only compressed.

**The right-click menu no longer vanishes on the way to its lower items.** The
close rule was "close when neither menu window has focus". i3 hands focus to a
submenu and never gives it back, so the first hover of a plain row read as
clicking away. The rule now also asks where the pointer is.

**Dialogs cannot open invisibly behind the main window.** Dialogs are modal, so
the app drops all input while one is up, and a tiling WM was putting them
underneath the floating main window. That reads exactly like the app freezing
the moment you pick a camera. Menus and dialogs now carry the X11 window-type
hints that tell a WM to float them.

**Filming can no longer take the app down.** The compositor needs a Vulkan
adapter; without one it fell through to wgpu's OpenGL backend, whose first
make-current against the window's own GL context aborts. It now asks for Vulkan
hardware, falls back to a software adapter, and refuses GL with a message
naming the fix. `install.sh` checks for a Vulkan driver at install time and
prints the distribution's one-line remedy.

## Every platform

**The Linux and Windows artifacts carry their own encoder.** An unmodified,
checksum-pinned static ffmpeg ships as `tangent-ffmpeg` beside the binary, and
Tangent finds it there before consulting `PATH`. Filming a take needs nothing
installed. Its GPL licence and provenance ship beside it. macOS needs none of
this: its encoder is AVFoundation.

**`take.json` records the video.** The manifest is written at Stop, before the
encoder has finished the file, so its video section was always null and
`take.mp4` never appeared in `files`. On every platform, since the recorder
shipped. The session now keeps what it wrote and folds the report in when the
encoder finishes.

**A slow machine no longer records a fast video.** A machine that composites
slower than real time used to fall behind schedule and stay there: every frame
carried late content at an early timestamp, and a whole performance came out
time-compressed. The pump now holds the timeline. Missed ticks repeat the
previous frame, which is a visible and counted stutter rather than a silent
lie, and the take summary reports repeated frames alongside dropped ones.

## Downloads

Each installer offers the app and the VST3 plugin as separate choices.

  macOS 11 or later, Apple Silicon and Intel, signed and notarized
    Tangent-macos.pkg

  Windows 10 or later
    Tangent-windows-setup.exe

  Linux x86_64, glibc 2.32 or later, ALSA. The tarball has an install.sh that
  needs no root, and carries its own encoder
    tangent-linux-x86_64.tar.gz

  Linux aarch64. This one does not bundle an encoder yet, so filming needs
  ffmpeg on the PATH; everything else is the same
    tangent-4.4.1-linux-aarch64.tar.gz

Checksums are in SHA256SUMS.
