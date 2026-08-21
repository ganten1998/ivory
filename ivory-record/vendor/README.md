# Vendored `linuxvideo`

`linuxvideo/` is [`linuxvideo` 0.3.5](https://github.com/SludgePhD/LinuxVideo)
(0BSD) with three additions and no other changes. Upstream is unmodified
otherwise, deliberately: keeping the diff this small is what makes it
reviewable and what lets us drop the copy the moment the change lands upstream.

## Why

0.3.5 asks V4L2 for two capture buffers and gives the caller no say in it.
`ReadStream::dequeue` holds one buffer for as long as its callback runs, so the
driver has exactly one left to fill; a frame arriving in that window has
nowhere to go and overwrites one nobody has read.

That loss was also *unobservable*. V4L2 counts every frame the hardware
produces in `v4l2_buffer.sequence` whether the application collected it or not,
so a gap between consecutively dequeued buffers is an exact count of what was
lost — but 0.3.5 never surfaces the field. Without it, a camera dropping a
third of its frames and a camera dropping none look identical from up here.

The two are one change, not two: the buffer count is the fix and the sequence
number is the only way to tell whether the fix did anything.

## The diff

| Added | Where | What it is |
| --- | --- | --- |
| `VideoCaptureDevice::into_stream_with(count)` | `src/lib.rs` | `into_stream()` with the buffer count exposed. `into_stream()` now calls it with `DEFAULT_BUFFER_COUNT`, so existing callers are byte-identical in behaviour. |
| `ReadStream::buffer_count()` | `src/stream.rs` | How many buffers the driver actually granted. `VIDIOC_REQBUFS` is a request, not a demand — anything reasoning about queue depth has to read back what it got. |
| `ReadBufferView::sequence()` | `src/stream.rs` | The driver's frame counter for this buffer, already read by `dequeue` and previously discarded. |

`Cargo.toml` additionally carries a note and is excluded from the workspace by
the root `Cargo.toml`, so it stays out of ivory's lints and `cargo test
--workspace`.

Regenerate the diff with:

    diff -ru ~/.cargo/registry/src/*/linuxvideo-0.3.5/src ivory-record/vendor/linuxvideo/src

## What it measured

`cargo run --release -p ivory-record --example bufdepth` and `--example
seqprobe`, against the 2012 FaceTime HD at 1280x720 MJPG:

- The steady state loses **nothing at any depth**, two included. With the
  decode moved out of the dequeue closure and VA-API doing it in 2.3 ms, the
  capture thread no longer holds a buffer long enough to matter.
- Under a deliberate 300 ms stall every 15 frames, depth 2 loses 10 frames,
  depth 4 loses 5, and depth 6 and 8 lose none. Six is the knee, which is why
  it is the default.
- `sequence` **is** maintained by uvcvideo — it steps by exactly 1 across 35
  consecutive frames with a fast consumer — so the zeros above are a real
  result and not a dead field. `seqprobe` exists to keep that check honest on
  any other camera.
- A sustained slow consumer is not fixed by depth and cannot be: at a 120 ms
  hold, depth 8 loses 16 frames where depth 2 loses 27. A deeper queue takes
  longer to saturate; it does not stop a consumer that is slower than the
  producer.
- **Buffers cost the uncompressed rate even for MJPEG.** uvcvideo sets
  `sizeimage` to the YUYV worst case — 1,843,200 bytes at 720p, measured — so
  six buffers is 10.5 MB, not the ~1.5 MB a compressed-frame estimate suggests.

## Upstream

The change is small and general and should go back:
<https://github.com/SludgePhD/LinuxVideo>. Once it is released, delete this
directory, drop the `exclude` line from the root `Cargo.toml`, and put
`linuxvideo = "0.3.x"` back in `ivory-record/Cargo.toml`.
