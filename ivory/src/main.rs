//! Ivory — MIDI keyboard monitor with chord detection (Rust port of the
//! PySide6 v1.1.0 app; UI parity per docs/spec/ui-spec.md).
#![windows_subsystem = "windows"]

mod app;
mod chord_strip;
mod dialogs;
mod fonts;
mod menu;
mod midi;
mod piano;
mod settings;
mod theme;

use settings::Settings;

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
    let text = "usage: ivory [-h] [-p PORT] [-l]\n\
                \n\
                PySide6 MIDI keyboard monitor with chord detection\n\
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
            "-h" | "--help" => print_usage_and_exit(0),
            "-p" | "--port" => match args.next() {
                Some(value) => port = Some(value),
                None => {
                    eprintln!("ivory: error: argument -p/--port: expected one argument");
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
                    eprintln!("ivory: error: unrecognized arguments: {other}");
                    print_usage_and_exit(2);
                }
            }
        }
    }
    CliArgs { port }
}

/// Top-level error surface (spec §2.3): any panic shows a critical "Ivory
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
        let body = format!("Ivory encountered an error:\n\n{msg}\n  at {location}\n\n{backtrace}");
        eprintln!("{body}");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Ivory Error")
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
    install_panic_hook();

    if !acquire_single_instance() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Ivory Already Running")
            .set_description("Ivory is already running.\n\nOnly one instance can run at a time.")
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(0);
    }

    // Settings are read before building the ViewportBuilder so the very first
    // frame is right-sized (DESIGN: fixed-size via Min+Max+Inner triple).
    let settings = Settings::load();
    let size = app::initial_window_size(&settings);

    // The app icon MUST be set here. eframe applies its own embedded default
    // (the egui hexagon) whenever the viewport icon is None, and on macOS a
    // runtime icon overrides the .app bundle's CFBundleIconFile — so without
    // this the Dock showed the egui logo while Finder correctly showed Ivory's.
    // A decode failure just means no runtime icon: on macOS the bundle icon then
    // applies, so fall back silently rather than killing startup over artwork.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Ivory")
        .with_app_id("ivory")
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_decorations(!settings.borderless_mode);

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
        "Ivory",
        options,
        Box::new(move |cc| Ok(Box::new(app::IvoryApp::new(cc, settings, port)))),
    );

    if let Err(err) = result {
        let body = format!("Ivory encountered an error:\n\n{err}");
        eprintln!("{body}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Ivory Error")
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(1);
    }
}
