//! Tangent — MIDI keyboard monitor with chord detection (Rust port of the
//! PySide6 v1.1.0 app; UI parity per docs/spec/ui-spec.md).
#![windows_subsystem = "windows"]

mod desktop;
#[cfg(feature = "recorder")]
mod devices;
mod midi;
/// The recording session. Absent from a Minimal build, where `ivory-record` is
/// not linked at all — the exclusion the compiler enforces rather than a flag
/// anyone can flip (docs/RECORDER-PLAN.md §12a).
#[cfg(feature = "recorder")]
mod record;

use ivory_ui::settings::Settings;

/// On Windows, a windows-subsystem binary detaches from the console; reattach
/// so `-l` / `-p` output still prints when launched from a terminal.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

struct CliArgs {
    port: Option<String>,
}

fn print_usage_and_exit(code: i32) -> ! {
    let text = "usage: tangent [-h] [-p PORT] [-l]\n\
                \n\
                MIDI keyboard monitor with chord detection\n\
                \n\
                options:\n\
                  -h, --help            show this help message and exit\n\
                  -p PORT, --port PORT  MIDI input port name\n\
                  -l, --list            List available MIDI input ports\n";
    if code == 0 {
        print!("{text}");
    } else {
        eprint!("{text}");
    }
    std::process::exit(code);
}

fn parse_cli() -> CliArgs {
    let mut port = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-l" | "--list" => {
                midi::print_port_list();
                std::process::exit(0);
            }
            // Undocumented on purpose (absent from --help): a developer probe
            // for the recorder's raw MIDI tap, which the GUI cannot show yet.
            // It is how the timestamp, the channel, the continuous pedal sweep
            // and the release velocity get checked against a REAL keyboard,
            // which no unit test can do. See docs/RECORDER-PLAN.md step 1.
            #[cfg(feature = "recorder")]
            "--dump-midi" => {
                midi::dump_raw(args.next());
                std::process::exit(0);
            }
            // The other half of the same idea, for audio: record one take from
            // the default input with no GUI and report what landed on disk.
            // The only way to exercise the whole capture chain against real
            // hardware, and the way a missing microphone entitlement shows up
            // as an empty device list rather than as a mystery months later.
            #[cfg(feature = "recorder")]
            "--record-test" => {
                record::record_test(args.next());
                std::process::exit(0);
            }
            "-h" | "--help" => print_usage_and_exit(0),
            "-p" | "--port" => match args.next() {
                Some(value) => port = Some(value),
                None => {
                    eprintln!("tangent: error: argument -p/--port: expected one argument");
                    std::process::exit(2);
                }
            },
            other => {
                if let Some(value) = other.strip_prefix("--port=") {
                    port = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("-p") {
                    if !value.is_empty() {
                        port = Some(value.to_owned());
                    }
                } else {
                    eprintln!("tangent: error: unrecognized arguments: {other}");
                    print_usage_and_exit(2);
                }
            }
        }
    }
    CliArgs { port }
}

/// Minimal stderr logger. eframe probes for a desktop GL context first and only
/// falls back to GLES if that fails — but the first failure is a `log::warn!`,
/// so with no logger installed a startup crash reports the FALLBACK error and
/// hides the actual cause. (Seen under CrossOver: "extension to create ES
/// context with wgl is not present", which is the second failure, not the first.)
///
/// Quiet by default; `IVORY_LOG=debug ivory` turns it on. On Windows pair it
/// with a console — the binary is windows-subsystem and reattaches to the
/// parent, so running it from a shell shows this output.
struct StderrLogger(log::LevelFilter);

impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

fn install_logger() {
    let level = match std::env::var("IVORY_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        "off" => log::LevelFilter::Off,
        // Default: warnings only. Silent in normal use, but a GL probe failure
        // leaves a trail if the user ever runs it from a terminal.
        _ => log::LevelFilter::Warn,
    };
    let logger = Box::leak(Box::new(StderrLogger(level)));
    let _ = log::set_logger(logger).map(|()| log::set_max_level(level));
}

/// Top-level error surface (spec §2.3): any panic shows a critical "Tangent
/// Error" box with the message and a backtrace, then exits with code 1.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown error".to_owned()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let body =
            format!("Tangent encountered an error:\n\n{msg}\n  at {location}\n\n{backtrace}");
        eprintln!("{body}");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Tangent Error")
                .set_description(&body)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }));
        std::process::exit(1);
    }));
}

/// Single instance via a lock file (D-UI-2): same dialog + exit UX as the
/// Python QSharedMemory key "ivory-midi-monitor"; a crashed instance never
/// blocks relaunch (the OS releases the lock with the process).
fn acquire_single_instance() -> bool {
    let Some(home) = dirs::home_dir() else {
        return true; // no home dir: don't block launching
    };
    let dir = home.join(".config").join("ivory");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("ivory-midi-monitor.lock");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
    else {
        return true; // can't create the lock file: don't block launching
    };
    let lock: &'static mut fd_lock::RwLock<std::fs::File> =
        Box::leak(Box::new(fd_lock::RwLock::new(file)));
    match lock.try_write() {
        Ok(guard) => {
            // Hold the lock for the lifetime of the process.
            std::mem::forget(guard);
            true
        }
        Err(_) => false,
    }
}

fn main() {
    #[cfg(windows)]
    attach_console();

    let cli = parse_cli();
    install_logger();
    install_panic_hook();

    if !acquire_single_instance() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Tangent Already Running")
            .set_description("Tangent is already running.\n\nOnly one instance can run at a time.")
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(0);
    }

    // Settings are read before building the ViewportBuilder so the very first
    // frame is right-sized (DESIGN: fixed-size via Min+Max+Inner triple).
    let settings = Settings::load();
    let size = ivory_ui::app::initial_window_size(&settings);

    // The app icon MUST be set here. eframe applies its own embedded default
    // (the egui hexagon) whenever the viewport icon is None, and on macOS a
    // runtime icon overrides the .app bundle's CFBundleIconFile — so without
    // this the Dock showed the egui logo while Finder correctly showed Tangent's.
    // A decode failure just means no runtime icon: on macOS the bundle icon then
    // applies, so fall back silently rather than killing startup over artwork.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tangent")
        .with_app_id("ivory")
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_decorations(!settings.borderless_mode);

    // Reopen where it was last left. Monitors are not enumerable before the
    // window exists, so a position saved on a display that is now unplugged
    // could land off-screen; `IvoryApp` rescues that on the first frame, once
    // it can actually see the monitor.
    if let Some(pos) = settings.window_pos_for_use() {
        viewport = viewport.with_position(pos);
    }

    // macOS wants a rounded-squircle icon with padding around the art; every other
    // platform expects the plain full-bleed square. Both are the same artwork.
    #[cfg(target_os = "macos")]
    let icon_png: &[u8] = &include_bytes!("../../assets/ivory-macos.png")[..];
    #[cfg(not(target_os = "macos"))]
    let icon_png: &[u8] = &include_bytes!("../../assets/ivory.png")[..];

    match eframe::icon_data::from_png_bytes(icon_png) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(err) => eprintln!("could not decode assets/ivory.png as the app icon: {err}"),
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let port = cli.port;
    let result = eframe::run_native(
        "Tangent",
        options,
        Box::new(move |cc| Ok(Box::new(desktop::DesktopApp::new(cc, settings, port)))),
    );

    if let Err(err) = result {
        let body = format!("Tangent encountered an error:\n\n{err}");
        eprintln!("{body}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Tangent Error")
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(1);
    }
}
