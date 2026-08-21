//! Why does wgpu find no hardware adapter on this machine, and can it be made to?
//!
//! `composite.rs` falls back to lavapipe because `request_adapter` returns a
//! CPU adapter — on a GPU that has a working hardware OpenGL 4.2 driver
//! (crocus) which the app's own *window* is already using. This asks wgpu
//! directly, with its internal log turned up, so the answer comes from wgpu
//! rather than from reading wgpu.
//!
//!   cargo run -p ivory --example glprobe
//!   cargo run -p ivory --example glprobe -- trace     # full wgpu-hal trace
//!
//! Linux is the only place this question means anything; elsewhere the window
//! renderer and the compositor already agree.

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn main() {
    eprintln!("glprobe asks why wgpu sees no hardware adapter on Linux; there is nothing to ask here.");
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    /// A logger in twenty lines rather than a dependency.
    ///
    /// `ivory` takes `log` but not `env_logger`, and this example exists to
    /// read wgpu-hal's own `log::debug!` lines — which are the only place the
    /// EGL negotiation is described. Adding a dev-dependency to see them would
    /// be a crate bought for one example.
    struct Stderr(log::LevelFilter);

    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= self.0
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("  [{:<5} {}] {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }

    fn describe(a: &wgpu::Adapter) -> String {
        let i = a.get_info();
        let soft = i.device_type == wgpu::DeviceType::Cpu
            || i.name.contains("llvmpipe")
            || i.name.contains("lavapipe")
            || i.name.contains("softpipe");
        format!(
            "{:<44} {:?}/{:?}  {}  [{} {}]",
            i.name,
            i.backend,
            i.device_type,
            if soft { "SOFTWARE" } else { "** HARDWARE **" },
            i.driver,
            i.driver_info
        )
    }

    pub fn main() {
        let trace = std::env::args().any(|a| a == "trace");
        let level = if trace {
            log::LevelFilter::Trace
        } else {
            log::LevelFilter::Debug
        };
        log::set_boxed_logger(Box::new(Stderr(level))).ok();
        log::set_max_level(level);

        // 1. What the app actually does today.
        println!("== what composite.rs asks for (all backends, HighPerformance) ==");
        let inst = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        match pollster_lite(inst.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })) {
            Ok(a) => println!("   -> {}", describe(&a)),
            Err(e) => println!("   -> no adapter: {e:?}"),
        }
        println!();

        // 2. Each backend on its own, enumerated rather than requested, so a
        //    backend that produces nothing is visibly producing nothing rather
        //    than being out-ranked by lavapipe.
        for (label, backends) in [
            ("VULKAN", wgpu::Backends::VULKAN),
            ("GL / EGL", wgpu::Backends::GL),
        ] {
            println!("== {label}, enumerated ==");
            let inst = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                ..Default::default()
            });
            let list = inst.enumerate_adapters(backends);
            if list.is_empty() {
                println!("   (none)");
            }
            for a in &list {
                println!("   {}", describe(a));
            }
            println!();
        }

        // 3. The GL backend with every knob this machine might need.
        //    Gen7 caps at GLES 3.0, so Automatic (which asks for 3.2 first)
        //    may be the thing that fails rather than robustness.
        for (label, minor) in [
            ("Automatic", wgpu::Gles3MinorVersion::Automatic),
            ("Version0 (Gen7 ceiling)", wgpu::Gles3MinorVersion::Version0),
        ] {
            println!("== GL / EGL, gles_minor_version = {label} ==");
            let inst = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::GL,
                flags: wgpu::InstanceFlags::empty(),
                backend_options: wgpu::BackendOptions {
                    gl: wgpu::GlBackendOptions {
                        gles_minor_version: minor,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            });
            let list = inst.enumerate_adapters(wgpu::Backends::GL);
            if list.is_empty() {
                println!("   (none)");
            }
            for a in &list {
                println!("   {}", describe(a));
            }
            println!();
        }
    }

    /// `composite.rs` has its own `block_on`; this example only ever awaits a
    /// future that is already resolved by the time it is polled, so a full
    /// executor would be a dependency bought for one call.
    fn pollster_lite<F: std::future::Future>(mut f: F) -> F::Output {
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }
        let waker = Waker::from(Arc::new(Noop));
        let mut cx = Context::from_waker(&waker);
        // SAFETY: `f` is owned here and never moved again.
        let mut f = unsafe { std::pin::Pin::new_unchecked(&mut f) };
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn main() {
    imp::main();
}
