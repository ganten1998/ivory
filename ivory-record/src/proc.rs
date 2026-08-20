//! One place that knows a windowed app must not let a child open a console.

use std::ffi::OsStr;
use std::process::Command;

/// A `Command` that will not put a console window on the screen.
///
/// **Windows only, and it is the whole reason this module exists.** Tangent is
/// built `#![windows_subsystem = "windows"]`, so the process has no console for
/// a child to inherit — and Windows hands a console-subsystem child one of its
/// own, with a visible window, unless told not to. ffmpeg is a console program.
/// So every video take, every mux at the end of one and every backing track
/// loaded put a console window on screen for as long as ffmpeg ran, which is
/// what a tester on Windows reported as intermittent white flashes while using
/// the app. Redirecting the child's output does not help: the console is
/// allocated at process start regardless of where its handles point.
///
/// `CREATE_NO_WINDOW` (`0x0800_0000`, `winbase.h`) is the documented answer. It
/// is spelled out rather than pulling in `windows-sys` for one integer, and it
/// is a no-op on a GUI-subsystem child.
///
/// Everywhere else this is `Command::new` and nothing more.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    /// **Every runtime spawn goes through `command`.**
    ///
    /// A source scan, because the failure it guards is invisible on the two
    /// platforms this is developed on: `Command::new` compiles, runs and
    /// behaves identically on macOS and Linux, and only puts a window on the
    /// screen on the one platform nobody here can see. A reviewer cannot spot
    /// it and a test that runs the code cannot either.
    ///
    /// Test code is exempt — a test has a console already — so each file is
    /// scanned only up to its own `#[cfg(test)]`.
    #[test]
    fn nothing_spawns_a_child_the_long_way_round() {
        let files: [(&str, &str); 4] = [
            ("audio.rs", include_str!("audio.rs")),
            ("decode.rs", include_str!("decode.rs")),
            ("encode.rs", include_str!("encode.rs")),
            ("encode/ffmpeg.rs", include_str!("encode/ffmpeg.rs")),
        ];
        for (name, src) in files {
            let runtime = src.split("#[cfg(test)]").next().unwrap_or("");
            assert!(
                !runtime.contains("Command::new"),
                "{name} spawns a child with Command::new; use crate::proc::command \
                 or a console window appears on Windows"
            );
        }
    }
}
