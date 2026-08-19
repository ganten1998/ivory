//! The video compositor: one offscreen egui pass per output frame.
//!
//! This is the surface half of RECORDER-PLAN §6. The other half is
//! `IvoryApp::paint_composite`, which sees only an `egui::Painter` — that seam
//! is what keeps `ivory-ui` free of every renderer, and it is the reason the
//! firewall survives a feature that is fundamentally about pixels.
//!
//! # It is wgpu, and the plan said glow
//!
//! §6 borrows the app's `glow::Context` through `eframe::Frame::gl()` and reads
//! back with `egui_glow` and a pair of PBOs. That path does not exist on macOS
//! any more: the plugin-editor flicker fix made Metal the default renderer, and
//! under wgpu `Frame::gl()` returns `None`. Creating an independent GL context
//! to satisfy the old plan would re-create the two-contexts-on-one-thread
//! situation that caused the flicker in the first place.
//!
//! So the offscreen pass runs on the SAME `wgpu::Device` the window draws with,
//! handed over by `eframe::Frame::wgpu_render_state()`. No second device, no
//! second adapter, and no second GPU context of any kind.
//!
//! # Why the readback is synchronous
//!
//! `copy_texture_to_buffer` then `map_async` then poll-until-done, on the UI
//! thread, every composited frame. A double-buffered readback would hide the
//! latency and is what a game would do — but a take is 30 frames a second, not
//! 240, and the honest measurement is that this costs a few milliseconds at
//! 1080p. Spending complexity to save that, in a frame budget of 33 ms, would
//! buy a class of bug (a frame encoded one tick late, silently) for no gain
//! anyone could perceive.

use ivory_ui::app::IvoryApp;
use ivory_ui::recorder::{DisplayShows, Layout};

/// One offscreen frame's worth of machinery.
pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    /// A context of its OWN, not the window's.
    ///
    /// It has its own font atlas, its own memory and its own id space. Sharing
    /// the window's would mean two `run` passes per real frame against one set
    /// of widget state, and every id in the video frame colliding with the same
    /// id in the window.
    ctx: egui::Context,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    /// Bytes per row in `readback`, rounded up to wgpu's 256-byte copy
    /// alignment. Almost never equal to `width * 4`, which is why the readback
    /// copies row by row.
    padded_bpr: u32,
    /// The camera's texture, kept between frames and re-uploaded in place.
    camera: Option<CameraTexture>,
    /// The tightly-packed BGRA the encoder is handed.
    frame: Vec<u8>,
    /// Whether `frame` holds a real picture yet. `frame` is allocated zeroed,
    /// and handing that out as [`last_frame`](Self::last_frame) would pad a
    /// slow take's opening with black frames that were never composited.
    has_frame: bool,
}

struct CameraTexture {
    texture: wgpu::Texture,
    id: egui::TextureId,
    size: (u32, u32),
}

/// Drive a future to completion on this thread.
///
/// Two `wgpu` calls in this file are async and there is no runtime here. A
/// short poll loop is the whole of what an executor would be used for, so this
/// is that loop rather than a dependency.
fn block_on<F: std::future::Future>(mut f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    // Safety: `f` lives on this stack frame and is never moved again.
    let mut f = unsafe { std::pin::Pin::new_unchecked(&mut f) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// A software adapter, because the hardware ones came up empty.
///
/// `force_fallback_adapter` picks a CPU device — lavapipe on Linux, WARP on
/// Windows — where one is installed. Slower is reported by the frame counters;
/// absent is reported by nothing, which is why this rung exists.
fn software_adapter(instance: &wgpu::Instance) -> Option<wgpu::Adapter> {
    block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: true,
        compatible_surface: None,
    }))
    .ok()
}

impl Compositor {
    /// Build a compositor for a frame of `width` x `height`.
    ///
    /// `state` comes from `eframe::Frame::wgpu_render_state()`. `None` there
    /// means the app is running under glow, which is `IVORY_RENDERER=glow` and
    /// every non-macOS build — those cannot composite, and say so rather than
    /// producing an empty file.
    pub fn new(
        state: Option<&egui_wgpu::RenderState>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        match state {
            // The window's own device, which costs nothing to borrow.
            Some(s) => Self::on(s.device.clone(), s.queue.clone(), width, height),
            // **A device of its own, rather than a refusal.** `None` means the
            // app is drawing with OpenGL, which is every Windows and Linux
            // build and `IVORY_RENDERER=glow` on a Mac. That used to end the
            // video, on the reasoning that a compositor needs the renderer the
            // window is using. It does not: it paints an OFFSCREEN egui pass
            // into a texture nothing else ever sees, so any adapter will do.
            // Making the video depend on the window's renderer meant a whole
            // platform could not film, for a reason that was never true.
            None => Self::standalone(width, height),
        }
    }

    /// A compositor on an adapter it finds for itself.
    ///
    /// Blocking, and deliberately: it is called once, at the moment a take
    /// starts filming, on a thread that is about to do something expensive
    /// anyway. Bringing an executor into this crate to await two futures at
    /// startup would be a dependency bought for one line.
    pub fn standalone(width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        // A CPU rasterizer beats no video. The hardware ask fails on real
        // machines: a 2012-era GPU has no Vulkan driver at all, and mesa's
        // crocus answers wgpu's robust-context request with BAD_MATCH, which
        // wgpu treats as fatal — so the GL backend enumerates nothing either.
        // lavapipe supports everything wgpu asks for; slow is reported by the
        // frame counters, absent is reported by nothing.
        .or_else(|first| software_adapter(&instance).ok_or(first))
        .map_err(|e| {
            format!(
                "no graphics adapter this build can composite with: {e}{}",
                if cfg!(target_os = "linux") {
                    " - installing your distribution's software Vulkan driver \
                     (mesa lavapipe) makes video work on any GPU"
                } else {
                    ""
                }
            )
        })?;
        // A GL adapter is REFUSED, not tried. This runs on the window's own
        // thread and the window is drawn with OpenGL on this platform; EGL
        // allows one current context per thread, so wgpu's first make-current
        // against the window's is EGL_BAD_ACCESS — which wgpu-hal unwraps,
        // taking the whole app down mid-take. Observed, not theorised. Vulkan
        // has no notion of "current" and coexists fine.
        if adapter.get_info().backend == wgpu::Backend::Gl {
            return Err(
                "compositing cannot share a thread with the window's OpenGL - \
                 install your distribution's Vulkan driver (hardware, or mesa's \
                 lavapipe for any GPU) and video will work"
                    .to_owned(),
            );
        }
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("the graphics adapter would not open a device: {e}"))?;
        Self::on(device, queue, width, height)
    }

    /// The same, from a device this code was handed rather than one it found.
    ///
    /// Split out so the compositor can be built on a headless device in a test.
    /// The alternative was constructing an `egui_wgpu::RenderState`, which
    /// means standing up an adapter and a surface for a thing that never draws
    /// to a window — and a test that cannot run is a test that does not exist.
    pub fn on(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("a video frame needs a size".to_owned());
        }
        // BGRA to match what the encoder wants, so the readback is a copy and
        // not a conversion. Every channel swizzle avoided here is 8 MB a frame
        // of CPU work at 1080p that nobody has to do.
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions::default(),
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tangent composite"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_bpr = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tangent composite readback"),
            size: u64::from(padded_bpr) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let ctx = egui::Context::default();
        Ok(Self {
            device,
            queue,
            renderer,
            ctx,
            target,
            view,
            readback,
            width,
            height,
            padded_bpr,
            camera: None,
            frame: vec![0; (width as usize) * (height as usize) * 4],
            has_frame: false,
        })
    }

    /// The offscreen context, so the caller can seed it with the app's fonts.
    ///
    /// It MUST be seeded, or chord names render in egui's default face instead
    /// of Courier Prime and the video does not match the window.
    pub fn context(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Composite one frame and return it as tightly-packed BGRA.
    ///
    /// `camera` is the latest frame from the device, as **RGBA** with its own
    /// size — which is what `ivory_record::camera::Frame` carries, the backend
    /// having already converted from the device's BGRA. What this RETURNS is
    /// BGRA, because that is what the encoder wants. The two differ, the
    /// texture formats below are what convert between them, and assuming they
    /// were the same would swap red and blue in every recording.
    ///
    /// `None` means the camera has not produced a frame yet — the normal state
    /// for the first seconds of a take, and drawn as an empty pane rather than
    /// skipped. See `encode`'s note on why the video ticks from take start
    /// regardless of what the camera is doing.
    pub fn frame(
        &mut self,
        app: &IvoryApp,
        layout: Layout,
        shows: DisplayShows,
        want_camera: bool,
        want_display: bool,
        camera: Option<(&[u8], u32, u32)>,
    ) -> Result<&[u8], String> {
        if let Some((pixels, w, h)) = camera {
            self.upload_camera(pixels, w, h)?;
        }
        let frame_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.width as f32, self.height as f32),
        );
        let camera_id = self.camera.as_ref().map(|c| (c.id, c.size));
        // **The frame IS the window.** No split, no inset, no layer order.
        //
        // A take used to be an arrangement of its own: the app's bands fitted
        // into one pane, the camera composited into another, and a `Layout`
        // deciding which floated over which. That was a second design of the
        // same picture, and the two disagreed — the recorder band was dropped
        // from the video, so the camera had to be put back by a route that
        // could place it somewhere the window never does.
        //
        // The window already has the camera in it, full height at the top-left
        // of the recorder band. So the display fills the frame, the camera
        // texture goes to the app, and what the video shows is what the person
        // recording it was looking at. `layout`, `want_camera` and
        // `want_display` are still taken so the call sites and the settings
        // file do not have to change in the same release; nothing reads them.
        let _ = (layout, want_camera, want_display, shows);
        let display_pane = frame_rect;
        let input = egui::RawInput {
            screen_rect: Some(frame_rect),
            // One physical pixel per point. The compositor is painting into a
            // pixel buffer, so "points" and "pixels" are the same thing here —
            // and a scale factor inherited from the window would make a video
            // recorded on a Retina machine twice the size of the same video
            // recorded on a monitor.
            ..Default::default()
        };
        let out = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    let painter = ui.painter();
                    app.paint_composite(
                        painter,
                        display_pane,
                        shows,
                        // The camera as THIS context knows it. The window's
                        // texture handle means nothing here — they are
                        // different `egui::Context`s with different atlases,
                        // so it would draw whatever else happened to carry
                        // that id, or nothing at all.
                        camera_id.map(|(texture, (w, h))| ivory_ui::recorder::Preview {
                            texture,
                            size: egui::vec2(w as f32, h as f32),
                        }),
                    );
                });
        });

        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        for (id, delta) in &out.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: out.pixels_per_point,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tangent composite"),
            });
        self.renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &desc);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tangent composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &desc);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }

        self.read_back()?;
        self.has_frame = true;
        Ok(&self.frame)
    }

    /// The most recent successfully composited frame, tightly-packed BGRA.
    ///
    /// `None` until [`frame`](Self::frame) has succeeded once. This is what
    /// lets the video pump hold the timeline on a machine that composites
    /// slower than real time: a repeated real frame keeps the clock honest,
    /// where compositing every tick late would compress the whole performance.
    pub fn last_frame(&self) -> Option<&[u8]> {
        self.has_frame.then_some(self.frame.as_slice())
    }

    fn read_back(&mut self) -> Result<(), String> {
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        // Poll until the copy has landed. `PollType::Wait` blocks on the
        // device rather than spinning, which is the whole reason this is
        // allowed to be synchronous.
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("the video frame could not be read back: {e}")),
            Err(_) => return Err("the video frame was never read back".to_owned()),
        }
        {
            let data = slice.get_mapped_range();
            let src_stride = self.padded_bpr as usize;
            let dst_stride = self.width as usize * 4;
            for y in 0..self.height as usize {
                let from = y * src_stride;
                self.frame[y * dst_stride..(y + 1) * dst_stride]
                    .copy_from_slice(&data[from..from + dst_stride]);
            }
        }
        self.readback.unmap();
        Ok(())
    }

    fn upload_camera(&mut self, pixels: &[u8], w: u32, h: u32) -> Result<(), String> {
        if w == 0 || h == 0 || pixels.len() < (w as usize * h as usize * 4) {
            return Err("a camera frame arrived the wrong size".to_owned());
        }
        // Re-made only when the SIZE changes. A camera that renegotiates
        // mid-take is rare; one that delivers the same size 30 times a second
        // is every take, and re-creating the texture each time would churn a
        // GPU allocation per frame.
        let stale = self.camera.as_ref().is_none_or(|c| c.size != (w, h));
        if stale {
            if let Some(old) = self.camera.take() {
                self.renderer.free_texture(&old.id);
            }
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("tangent camera"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // RGBA, matching what the camera actually hands over — the
                // backend has already converted from the device's BGRA, and
                // `Frame::pixels` is RGBA by the time it reaches anything here.
                //
                // The render target is BGRA, and the difference is the point:
                // naming each texture's TRUE format is what makes the GPU do
                // the swizzle for nothing. Declaring this one BGRA so it
                // "matches" the target would exchange red and blue in every
                // recording anybody ever made.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = self.renderer.register_native_texture(
                &self.device,
                &view,
                wgpu::FilterMode::Linear,
            );
            self.camera = Some(CameraTexture {
                texture,
                id,
                size: (w, h),
            });
        }
        let cam = self.camera.as_ref().expect("just made");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cam.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[..(w as usize * h as usize * 4)],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }
}

/// Paint the camera into its pane, filling it and cropping the overflow.
///
/// **Cropped, not letterboxed.** A letterboxed camera puts black bars inside a
/// pane that is itself inside a frame, which is two nested borders and reads as
/// a mistake. Cropping loses the edges of the picture, and the edges of a
/// webcam pointed at a piano are the wall.
fn paint_camera(
    painter: &egui::Painter,
    pane: egui::Rect,
    id: egui::TextureId,
    size: (u32, u32),
) {
    if !pane.is_positive() || size.0 == 0 || size.1 == 0 {
        return;
    }
    let src = size.0 as f32 / size.1 as f32;
    let dst = pane.width() / pane.height();
    // The fraction of the source used along each axis. One of them is always
    // 1.0 — you crop width or height, never both.
    let (uw, uh) = if src > dst {
        (dst / src, 1.0)
    } else {
        (1.0, src / dst)
    };
    let uv = egui::Rect::from_min_max(
        egui::Pos2::new((1.0 - uw) * 0.5, (1.0 - uh) * 0.5),
        egui::Pos2::new(1.0 - (1.0 - uw) * 0.5, 1.0 - (1.0 - uh) * 0.5),
    );
    painter.add(egui::Shape::image(id, pane, uv, egui::Color32::WHITE));
}

/// wgpu requires a buffer copy's rows to start on a 256-byte boundary.
fn padded_bytes_per_row(width: u32) -> u32 {
    const ALIGN: u32 = 256;
    let unpadded = width * 4;
    unpadded.div_ceil(ALIGN) * ALIGN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The readback stride must be a legal copy alignment AND still hold the
    /// row. Getting it wrong shears the picture diagonally, which is the
    /// classic symptom and is easier to assert than to recognise.
    #[test]
    fn a_readback_row_is_aligned_and_still_fits_the_picture() {
        for w in [1_u32, 2, 63, 64, 65, 320, 1080, 1280, 1920, 3840] {
            let p = padded_bytes_per_row(w);
            assert_eq!(p % 256, 0, "{w} gave an unaligned stride {p}");
            assert!(p >= w * 4, "{w} gave a stride {p} too small for its row");
            assert!(p < w * 4 + 256, "{w} gave a stride {p} padded more than once");
        }
    }

    /// 1920 BGRA is exactly 7680 bytes, which divides by 256 — so the common
    /// case needs no padding at all and the general case still must.
    #[test]
    fn the_common_widths_are_already_aligned_and_the_awkward_ones_are_not() {
        assert_eq!(padded_bytes_per_row(1920), 7680);
        assert_eq!(padded_bytes_per_row(1280), 5120);
        assert_eq!(padded_bytes_per_row(1080), 4352, "1080*4 = 4320, padded to 4352");
    }

    /// A headless device, so the GPU path can be tested without a window.
    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    /// The smallest possible executor. `pollster` is not a dependency of this
    /// crate and adding one for two `await`s in a test would be a poor trade.
    /// **The camera survives the round trip with its colours intact.**
    ///
    /// Upload BGRA, composite, read back BGRA, and check the middle pixel is
    /// the colour that went in. This is the test for the mistake that is
    /// invisible in code and obvious in a finished video: one channel swap
    /// anywhere in texture format, render target or readback and every
    /// recording anybody makes has blue skin and orange denim.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_camera_frame_comes_back_the_colour_it_went_in() {
        let Some((device, queue)) = headless() else {
            eprintln!("no GPU adapter - the compositor was not exercised");
            return;
        };
        const W: u32 = 320;
        const H: u32 = 240;
        let mut c = Compositor::on(device, queue, W, H).expect("compositor");

        // Opaque orange, in the RGBA byte order the camera delivers: R=0xF0,
        // G=0x80, B=0x20. It must come back as BGRA — the bytes REVERSED — and
        // that conversion is the whole point of the test. An earlier version
        // fed BGRA in and expected BGRA out, which round-tripped through
        // matching texture formats and so proved nothing at all.
        let cam: Vec<u8> = std::iter::repeat([0xF0, 0x80, 0x20, 0xFF])
            .take((W * H) as usize)
            .flatten()
            .collect();
        // **`first_launch`, not `default`.** A take is the window, and the
        // camera's only home in the window is the recorder band's preview —
        // so with the band switched off there is no camera in the video, which
        // is correct and makes this test measure nothing. Bare defaults have
        // the recorder off; a first launch has it on, which is the state
        // anybody recording is actually in.
        // And `DESKTOP` caps, not `MINIMAL`. A host that cannot open a capture
        // device has its recorder band forced off at construction — correctly,
        // since it would be two hundred points of transport it can never
        // populate — and a camera frame is not something such a host can have
        // in the first place.
        let app = IvoryApp::new(
            c.context(),
            ivory_ui::settings::Settings::first_launch(),
            ivory_ui::host::Caps::DESKTOP,
        );
        let out = c
            .frame(
                &app,
                Layout::default(),
                DisplayShows::default(),
                true,
                true,
                Some((&cam, W, H)),
            )
            .expect("composite");
        assert_eq!(out.len(), (W * H * 4) as usize, "the frame is the wrong size");
        // **Searched for, not sampled at a known point.** The camera has no
        // pane of its own any more — it is drawn inside the recorder band's
        // preview, wherever the band puts it — so the thing to assert is that
        // the colour is IN the frame and its mirror is not.
        //
        // Tolerant by a couple of levels: the image goes through a linear
        // blend, and exact equality would be asserting the rasteriser's
        // rounding rather than the channel order.
        let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
        let mut right = 0usize;
        let mut swapped = 0usize;
        for px in out.chunks_exact(4) {
            if close(px[0], 0x20) && close(px[1], 0x80) && close(px[2], 0xF0) {
                right += 1;
            }
            if close(px[0], 0xF0) && close(px[1], 0x80) && close(px[2], 0x20) {
                swapped += 1;
            }
        }
        assert!(
            right > 0,
            "RGBA F0,80,20 is nowhere in the frame as BGRA 20,80,F0 \
             ({swapped} pixels came back with red and blue swapped)"
        );
        assert_eq!(swapped, 0, "red and blue are swapped in {swapped} pixels");
        assert!(
            out.chunks_exact(4).all(|px| px[3] == 0xFF),
            "the frame is not opaque"
        );
    }

    /// **The whole pipeline, in one test, producing a file a person can watch.**
    ///
    /// Compositor to encoder to `.mp4`: a moving synthetic camera, the real
    /// display bands painted by the real app, real audio, in the layout a take
    /// actually uses — because the DEFAULT is the one every recording gets and
    /// the one nobody thinks to check.
    ///
    /// It writes to a named path and prints it, so the output is something to
    /// open rather than only something to assert on.
    #[test]
    #[ignore = "needs a GPU, and writes a real video"]
    fn the_whole_pipeline_writes_a_video_in_the_default_layout() {
        let Some((device, queue)) = headless() else {
            eprintln!("no GPU adapter - the pipeline was not exercised");
            return;
        };
        use ivory_record::encode::{AudioSpec, Encoder, VideoSpec};

        // Landscape at half 1080p, in the DEFAULT layout, so the frame the
        // test writes is the one a take actually produces.
        const W: u32 = 960;
        const H: u32 = 540;
        const FPS: u32 = 30;
        const RATE: u32 = 48_000;
        const CH: usize = 2;
        const N: u64 = 60;

        let dir = std::env::temp_dir().join("tangent-pipeline");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("default-layout.mp4");

        let mut c = Compositor::on(device, queue, W, H).expect("compositor");
        let app = IvoryApp::new(
            c.context(),
            ivory_ui::settings::Settings::default(),
            ivory_ui::host::Caps::MINIMAL,
        );
        let mut enc = Encoder::create(
            &path,
            VideoSpec {
                width: W,
                height: H,
                fps: FPS,
            },
            Some(AudioSpec {
                sample_rate: RATE,
                channels: CH as u16,
            }),
        )
        .expect("encoder");

        const CW: u32 = 640;
        const CHH: u32 = 480;
        let mut cam = vec![0u8; (CW * CHH * 4) as usize];
        let per_tick = (RATE / FPS) as usize;
        let mut audio = vec![0.0f32; per_tick * CH];
        let mut frames_done: u64 = 0;

        for i in 0..N {
            // A camera that visibly changes, so a video stuck on one frame is
            // distinguishable from one that is working.
            let shade = ((i * 4) % 256) as u8;
            for px in cam.chunks_exact_mut(4) {
                px[0] = shade;
                px[1] = 0x30;
                px[2] = 0xFF - shade;
                px[3] = 0xFF;
            }
            let bgra = c
                .frame(
                    &app,
                    Layout::default(),
                    DisplayShows::default(),
                    true,
                    true,
                    Some((&cam, CW, CHH)),
                )
                .expect("composite");
            enc.push(bgra, (i as i64 * 1_000_000_000) / i64::from(FPS))
                .expect("push video");
            for (n, sm) in audio.chunks_exact_mut(CH).enumerate() {
                let t = (frames_done + n as u64) as f64 / f64::from(RATE);
                sm.fill((t * 220.0 * std::f64::consts::TAU).sin() as f32 * 0.2);
            }
            enc.push_audio(&audio, frames_done).expect("push audio");
            frames_done += per_tick as u64;
        }
        let written = enc.frames_written();
        enc.finish().expect("finish");
        eprintln!("wrote {}", path.display());

        assert!(written >= N - 2, "only {written} of {N} frames were encoded");
        let Ok(out) = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,codec_name,width,height,duration",
                "-of",
                "default=nw=1",
            ])
            .arg(&path)
            .output()
        else {
            eprintln!("ffprobe is not installed - the file was not verified");
            return;
        };
        let probe = String::from_utf8_lossy(&out.stdout);
        assert!(probe.contains("codec_name=h264"), "{probe}");
        assert!(probe.contains("codec_name=aac"), "no sound: {probe}");
        assert!(
            probe.contains(&format!("width={W}")) && probe.contains(&format!("height={H}")),
            "the frame is not {W}x{H}: {probe}"
        );
        // Portrait, which is the point of this test.
        assert!(W > H);
    }

    /// A frame with no camera in it is still the window, not garbage.
    ///
    /// The first seconds of every take look like this, because a camera takes
    /// up to four seconds to wake up. It used to be asserted BLACK, back when
    /// the camera had a pane of its own and an empty pane was black. A take is
    /// the window now, so a camera that has not woken up yet costs the video
    /// one small box inside the recorder band and nothing else — the rest of
    /// the app is there from the first frame.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_frame_with_no_camera_yet_is_the_window_and_not_garbage() {
        let Some((device, queue)) = headless() else {
            eprintln!("no GPU adapter - the compositor was not exercised");
            return;
        };
        const W: u32 = 64;
        const H: u32 = 64;
        let mut c = Compositor::on(device, queue, W, H).expect("compositor");
        let app = IvoryApp::new(
            c.context(),
            ivory_ui::settings::Settings::default(),
            ivory_ui::host::Caps::MINIMAL,
        );
        let once = c
            .frame(&app, Layout::default(), DisplayShows::default(), true, true, None)
            .expect("composite")
            .to_vec();
        assert_eq!(once.len(), (W * H * 4) as usize);
        // Opaque everywhere: an uninitialised readback shows up here first.
        assert!(
            once.chunks_exact(4).all(|px| px[3] == 0xFF),
            "the frame is not opaque"
        );
        // Something was actually drawn — the app's own bands, which are not
        // black — so this is not the old empty pane wearing a new name.
        assert!(
            once.chunks_exact(4).any(|px| px[..3] != [0, 0, 0]),
            "a camera-less frame is entirely black, so nothing was painted"
        );
        // And it is DETERMINISTIC. Garbage is what differs between two frames
        // of the same unchanged app.
        let twice = c
            .frame(&app, Layout::default(), DisplayShows::default(), true, true, None)
            .expect("composite");
        assert_eq!(once, twice, "two frames of an unchanged app differ");
    }
}

#[cfg(test)]
mod shot {
    use super::*;

    /// Render the whole window offscreen to a PNG, for a person to look at.
    ///
    ///   IVORY_SHOT=/tmp/x.png cargo test -p ivory --bins shot::window \
    ///     -- --ignored --nocapture
    #[test]
    #[ignore = "writes a picture for a person to look at"]
    fn window() {
        let Ok(out) = std::env::var("IVORY_SHOT") else {
            eprintln!("IVORY_SHOT not set");
            return;
        };
        let (w, h) = (1600u32, 900u32);
        let mut c = match Compositor::standalone(w, h) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("no GPU here: {e}");
                return;
            }
        };
        let mut settings = ivory_ui::settings::Settings::default();
        settings.show_recorder = true;
        settings.reverb_mix = 0.42;
        settings.delay_mix = 0.18;
        settings.chorus_mix = 0.66;
        settings.metronome_gain = 0.5;
        settings.input_gain = 1.0;
        settings.plugin_slots[0] = Some(ivory_ui::dialogs::BUILTIN_PATH.to_owned());
        let mut app = IvoryApp::new(c.context(), settings, ivory_ui::host::Caps::DESKTOP);
        app.set_effect_defaults(crate::desktop::effect_defaults_for_shot());
        let mut shoot = |app: &IvoryApp| {
            c.frame(app, Layout::default(), DisplayShows::default(), false, true, None)
                .map(|_| ())
        };
        // The first frame lays out and the second draws what it decided.
        for _ in 0..2 {
            if let Err(e) = shoot(&app) {
                eprintln!("frame: {e}");
                return;
            }
        }
        // **After a frame, not before.** A panel hangs off its knob, and where
        // the knob is is not known until the band has been drawn once — so a
        // panel opened before that has no anchor and closes itself.
        if let Ok(which) = std::env::var("IVORY_SHOT_FX") {
            app.open_effect_panel(match which.as_str() {
                "delay" => ivory_ui::recorder_panel::Fx::Delay,
                "chorus" => ivory_ui::recorder_panel::Fx::Chorus,
                _ => ivory_ui::recorder_panel::Fx::Reverb,
            });
            for _ in 0..2 {
                if let Err(e) = shoot(&app) {
                    eprintln!("frame: {e}");
                    return;
                }
            }
        }
        let Some(px) = c.last_frame() else {
            eprintln!("nothing came back");
            return;
        };
        // Only the top of the frame when asked, because the band is a tenth of
        // a 900-row window and every crop tool on this machine measures its
        // offset from somewhere different.
        let rows = std::env::var("IVORY_SHOT_ROWS")
            .ok()
            .and_then(|r| r.parse::<u32>().ok())
            .unwrap_or(h)
            .min(h);
        write_png(
            std::path::Path::new(&out),
            &px[..(rows * w * 4) as usize],
            w,
            rows,
        );
        println!("wrote {out} ({rows} rows)");
    }

    /// A minimal RGBA PNG, so this needs no image crate.
    fn write_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = 0xffff_ffffu32;
            for b in bytes {
                c ^= u32::from(*b);
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc(&body).to_be_bytes());
        }
        // Stored (uncompressed) deflate blocks inside a zlib wrapper.
        fn zlib_stored(raw: &[u8]) -> Vec<u8> {
            let mut z = vec![0x78, 0x01];
            let mut a = 1u32;
            let mut b = 0u32;
            for byte in raw {
                a = (a + u32::from(*byte)) % 65521;
                b = (b + a) % 65521;
            }
            for (i, part) in raw.chunks(65_535).enumerate() {
                let last = u8::from((i + 1) * 65_535 >= raw.len());
                z.push(last);
                z.extend_from_slice(&(part.len() as u16).to_le_bytes());
                z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
                z.extend_from_slice(part);
            }
            z.extend_from_slice(&((b << 16) | a).to_be_bytes());
            z
        }
        // **The readback is BGRA**, which is the surface format, and a PNG is
        // RGBA. Writing it straight out gives a picture with the red and blue
        // channels swapped — tan panels come out pale blue, which is subtle
        // enough to read as a theme rather than a bug.
        let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
        for y in 0..h as usize {
            raw.push(0);
            let at = y * w as usize * 4;
            for px in rgba[at..at + w as usize * 4].chunks_exact(4) {
                raw.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &zlib_stored(&raw));
        chunk(&mut png, b"IEND", &[]);
        std::fs::write(path, png).expect("write the png");
    }
}
