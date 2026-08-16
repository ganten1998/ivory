//! The take directory: what a take is called, where it lives, how it is created
//! without ever overwriting another take, and the sidecar that says what
//! happened.
//!
//! A take is one press of Record to one press of Stop, and the directory it
//! produces **is** the deliverable (`docs/RECORDER-PLAN.md` §9). Nothing here
//! touches a device, so all of it is testable against `std::env::temp_dir()` in
//! a few milliseconds.
//!
//! # Why a naming scheme needs a hundred lines of rules
//!
//! Every rule below is here because a real recorder shipped without it.
//!
//! 1. **`YYYY-MM-DD_HHMMSS`, so ASCII sort equals chronological sort.** Any
//!    other field order (`15-08-2026`, `Aug15`) sorts a practice session into
//!    nonsense the moment it crosses a month boundary, and a pianist's folder
//!    holds hundreds of takes.
//! 2. **No colon.** Illegal in a Windows filename, and on macOS the Finder still
//!    swaps `/` and `:` at the presentation layer — so a name containing either
//!    displays as the other and the string in Terminal does not match the string
//!    on screen.
//! 3. **No space.** These paths get pasted into shells, drag targets and
//!    `ffmpeg` command lines, and an unquoted space splits one argument into two.
//! 4. **No leading `.` and no leading `-`.** The first hides the folder; the
//!    second is read as an option by every CLI tool ever written, so `rm -rf
//!    -take` is a syntax error at best.
//! 5. **No trailing `.` or space.** Windows *silently* strips both during path
//!    normalisation, so `take.` is created as `take`, and code that then reopens
//!    the string it asked for gets a path that does not exist. Silent is the
//!    problem: there is no error to catch.
//! 6. **No reserved device names.** `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`,
//!    `LPT0`-`LPT9`, case-insensitive, **and with any extension** — `NUL.txt` is
//!    still the null device. Opening one succeeds and writes into a device.
//! 7. **Truncate on a character boundary.** `&slug[..40]` panics — not
//!    truncates, *panics* — when byte 40 lands inside a multi-byte character, so
//!    a Japanese take name is a crash rather than a long name.
//!
//! # Creation is two calls and neither one is the other's `_all`
//!
//! ```text
//! fs::create_dir_all(root)?;          // ~/Movies/Tangent: absent on a fresh machine
//! fs::create_dir(root.join(name))?;   // the take: MUST be create_dir, MUST NOT be _all
//! ```
//!
//! The root does not exist on a new install (`~/Movies` does; `~/Movies/Tangent`
//! does not), and `create_dir` on a path whose parent is missing returns
//! `NotFound`, not `AlreadyExists` — so a design with only the second call fails
//! the very first take on every machine with an error naming the *take* folder
//! and hiding the real cause.
//!
//! The take folder stays `create_dir` because its `AlreadyExists` error **is**
//! the collision detector, and it is atomic. `Path::exists()` followed by
//! `create_dir_all` is TOCTOU *and* wrong even single-threaded, because
//! `create_dir_all` succeeds on a directory that is already there and the second
//! take then writes its files over the first take's.
//!
//! Retries are `-2`, `-3` … `-99`, and only on `AlreadyExists`. Retrying on
//! `NotFound` turns one missing parent into ninety-nine identical failures and a
//! final error message about a collision that never happened.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clock::{Nanos, RateFit, SourceClock, Timeline};

// ───────────────────────────────────────────────────────────────────────────
// Wall-clock time
// ───────────────────────────────────────────────────────────────────────────

/// The wall-clock reading a take is named after.
///
/// **This is not the timebase.** `clock::Nanos` is a monotonic coordinate that
/// answers "how far into the take"; this answers "what did the kitchen clock
/// say", which is the only thing a folder name can usefully carry. They are
/// different clocks on purpose: `SystemTime` can be stepped by NTP mid-take and
/// must never reach the sync arithmetic, while `Instant` has no epoch and cannot
/// name a folder.
///
/// # The local-time trap, stated because there is no way to hide it
///
/// This crate has no `chrono`, no `time` and no `libc`, and **std has no
/// local-time API at all**. So the UTC offset is a parameter: the caller
/// supplies it and [`from_unix`](WallTime::from_unix) does the rest. That is not
/// only a dependency workaround — `time::UtcOffset::current_local_offset`
/// deliberately *fails* in a multi-threaded process, because the only portable
/// way to ask is `localtime_r`, which reads the environment while another thread
/// may be in `setenv`. Tangent is multi-threaded by the time a take starts.
///
/// Passing `0` is honest and gives UTC; passing the wrong sign gives a folder
/// dated tomorrow, which is the failure to watch for in review.
///
/// Build these with [`from_unix`](WallTime::from_unix). The calendar fields are
/// a derived view of `unix_seconds` and setting one by hand desynchronises them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Seconds since the Unix epoch, UTC. Negative before 1970.
    pub unix_seconds: i64,
    /// Seconds east of UTC that were applied to produce the fields above.
    pub utc_offset_seconds: i32,
    /// Sub-second part, nanoseconds. Zero unless set with [`WallTime::with_nanos`].
    ///
    /// Carried because BWF's `TimeReference` is a **sample count** since
    /// midnight, so a take that starts at 12:00:00.5 is 24000 samples further
    /// into the day than one starting at 12:00:00.0 at 48 kHz. Dropping the
    /// fraction would put every take up to a second early in a REAPER timeline.
    pub nanos: u32,
}

impl WallTime {
    /// Break a Unix timestamp into local calendar fields.
    ///
    /// The civil-from-days conversion is Howard Hinnant's, which is exact for
    /// every year in `i64` and — the part that matters — handles negative days
    /// correctly. The obvious `secs / 86400` truncates towards zero, so every
    /// instant on 1969-12-31 comes out as 1970-01-01 and the clock appears to run
    /// backwards across the epoch. A machine whose RTC battery has died boots
    /// into 1970 and lands exactly there.
    pub fn from_unix(unix_seconds: i64, utc_offset_seconds: i32) -> Self {
        let local = unix_seconds + utc_offset_seconds as i64;
        let days = div_floor(local, 86_400);
        let secs_of_day = local - days * 86_400;

        // Shift the epoch to 0000-03-01 so that the leap day is the last day of
        // the year and the whole 400-year cycle becomes one table-free formula.
        let z = days + 719_468;
        let era = div_floor(z, 146_097);
        let doe = z - era * 146_097; // [0, 146096]
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11], March-based
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);

        Self {
            year,
            month,
            day,
            hour: (secs_of_day / 3_600) as u32,
            minute: (secs_of_day / 60 % 60) as u32,
            second: (secs_of_day % 60) as u32,
            unix_seconds,
            utc_offset_seconds,
            nanos: 0,
        }
    }

    /// Attach a sub-second part. See the field's docs for why it matters.
    pub fn with_nanos(mut self, nanos: u32) -> Self {
        self.nanos = nanos.min(999_999_999);
        self
    }

    /// Seconds elapsed since local midnight.
    pub fn seconds_since_midnight(&self) -> u64 {
        self.hour as u64 * 3_600 + self.minute as u64 * 60 + self.second as u64
    }

    /// Samples elapsed since local midnight, at `rate`.
    ///
    /// This is BWF's `TimeReference` exactly: a 64-bit sample count that lets
    /// REAPER, Pro Tools, Nuendo and Pyramix drop the file at its source
    /// position with no dragging. It is the single highest-leverage interop
    /// decision in the whole recorder, and it costs 602 bytes.
    pub fn samples_since_midnight(&self, rate: u32) -> u64 {
        self.seconds_since_midnight() * rate as u64
            + self.nanos as u64 * rate as u64 / 1_000_000_000
    }

    /// `YYYY-MM-DD`, BWF's `OriginationDate` field.
    pub fn date_string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `HH:MM:SS`, BWF's `OriginationTime` field.
    pub fn time_string(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// Now, in UTC. See the type's docs for why local time is not the default.
    pub fn now_utc() -> Self {
        Self::from_unix(unix_now_seconds(), 0)
    }

    /// Now, at a UTC offset the caller has obtained from the platform.
    pub fn now_at_offset(utc_offset_seconds: i32) -> Self {
        Self::from_unix(unix_now_seconds(), utc_offset_seconds)
    }

    /// `YYYY-MM-DD_HHMMSS` — the fixed-width, colon-free, space-free prefix
    /// every take name starts with.
    pub fn folder_prefix(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2026-08-15T14:32:07+02:00`, or `...Z` at offset zero.
    ///
    /// The offset is written out rather than assumed, because a sync report that
    /// says `14:32:07` with no zone cannot be lined up against a log from any
    /// other machine — and lining up logs is the entire reason this field exists.
    pub fn iso8601(&self) -> String {
        let date = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        if self.utc_offset_seconds == 0 {
            return format!("{date}Z");
        }
        let sign = if self.utc_offset_seconds < 0 { '-' } else { '+' };
        let mag = self.utc_offset_seconds.unsigned_abs();
        format!("{date}{sign}{:02}:{:02}", mag / 3_600, mag / 60 % 60)
    }
}

/// Euclidean division: rounds towards negative infinity, unlike `/`.
fn div_floor(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }
}

/// Seconds since the Unix epoch, negative if the machine thinks it is 1969.
///
/// `duration_since(UNIX_EPOCH).expect(..)` is the idiom everyone writes and it
/// panics on exactly that machine. A recorder that panics because a coin cell
/// died is worse than one that names a folder `1969-12-31_235959`.
pub fn unix_now_seconds() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// Nanoseconds since the Unix epoch, saturating rather than wrapping.
///
/// `as_nanos() as i64` wraps in the year 2262. Saturating there is meaningless
/// either way; wrapping produces a *negative* timestamp, which is the kind of
/// number that gets stored and then confuses somebody in 2262.
pub fn unix_now_nanos() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Naming
// ───────────────────────────────────────────────────────────────────────────

/// Characters that cannot appear in a Windows filename, plus the two that
/// macOS's Finder swaps at the presentation layer.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows device names. Reserved as a whole component **and** as the stem of
/// one, so `NUL`, `nul`, `NUL.txt` and `nul.wav` all open the null device.
const RESERVED: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// How much of a user's typed name survives, measured in **UTF-16 code units**.
///
/// Not characters, and the difference is not pedantry. `MAX_PATH` counts UTF-16
/// units; an emoji is one `char` and *two* units, so a forty-emoji take name
/// budgeted at forty would occupy eighty and every path reservation below would
/// be short by half. Counting units also makes truncation char-boundary-safe by
/// construction, because a unit boundary that is not a char boundary is never
/// reached: the loop adds whole characters.
///
/// For an all-BMP name — every Latin, Cyrillic, Greek, Hebrew, Arabic, Chinese,
/// Japanese and Korean name a pianist will type — this is exactly forty
/// characters.
pub const MAX_SLUG_UNITS: usize = 40;

/// `YYYY-MM-DD_HHMMSS`.
const PREFIX_UNITS: usize = 17;

/// `-99`, the longest collision suffix [`Take::create`] can append.
const COLLISION_UNITS: usize = 3;

/// The longest take *name* this module can produce: prefix, `_`, slug, `-99`.
pub const MAX_TAKE_NAME_UNITS: usize = PREFIX_UNITS + 1 + MAX_SLUG_UNITS + COLLISION_UNITS;

/// Turn something a user typed into something safe to be a path component.
///
/// Returns `None` when nothing usable survives — an empty box, a string of
/// slashes, or a reserved device name. The caller then produces a
/// timestamp-only folder, and the Recorder band's live grey preview of the
/// resulting name (`RECORDER-PLAN.md` §5) is what stops that being silent.
///
/// The order of operations is load-bearing and is documented step by step in the
/// body, because two of the steps exist only to undo damage done by the others.
pub fn sanitise_slug(raw: &str) -> Option<String> {
    // 1. Delete the characters that cannot be in a name at all. Deleted, not
    //    substituted: `a<b` becomes `ab`, because turning every illegal byte
    //    into `-` makes `C:\Users\me` read as `C--Users-me`, which is noise.
    //
    //    A control character that is also WHITESPACE is the exception and is
    //    left for step 3 to collapse. Tab and newline arrive constantly, because
    //    a take name gets pasted out of a tracklist or a spreadsheet cell, and
    //    deleting them welds two words together: `Chopin<TAB>Nocturne` would
    //    become `ChopinNocturne` rather than `Chopin-Nocturne`.
    let stripped: String = raw
        .chars()
        .filter(|c| !FORBIDDEN.contains(c) && (!c.is_control() || c.is_whitespace()))
        .collect();

    // 2. Reject a reserved device name BEFORE the collapse below eats the dot.
    //    `NUL.txt` is reserved; `NUL-txt`, which is what step 3 would make of
    //    it, is not. Checking only afterwards therefore passes the exact case
    //    the rule was written for.
    if is_reserved(&stripped) {
        return None;
    }

    // 3. Collapse every run of non-alphanumerics to one separator. A run that
    //    was entirely underscores stays an underscore — `nocturne_v2` is a name
    //    somebody typed on purpose and `nocturne-v2` is a small betrayal —
    //    everything else, including whitespace, becomes `-`.
    let mut collapsed = String::with_capacity(stripped.len());
    let mut run_start: Option<bool> = None; // Some(all_underscores_so_far)
    for c in stripped.chars() {
        if c.is_alphanumeric() {
            if let Some(all_underscore) = run_start.take() {
                collapsed.push(if all_underscore { '_' } else { '-' });
            }
            collapsed.push(c);
        } else {
            let all_underscore = run_start.unwrap_or(true) && c == '_';
            run_start = Some(all_underscore);
        }
    }
    // A trailing run is deliberately not flushed: it would only be trimmed
    // again in step 5.

    // 4. Trim leading separators. The plan asks for the leading `.` (a hidden
    //    folder); step 3 has already turned that into `-`, which is worse —
    //    every CLI tool in existence parses a leading `-` as an option.
    let trimmed = collapsed.trim_start_matches(['-', '_']);

    // 5. Truncate, counting UTF-16 units, adding whole characters only. Then
    //    trim trailing separators AGAIN, because the cut can land immediately
    //    after one and re-create the Windows trailing-character bug that step 3
    //    was supposed to have made impossible.
    let mut out = String::new();
    let mut units = 0usize;
    for c in trimmed.chars() {
        let w = c.len_utf16();
        if units + w > MAX_SLUG_UNITS {
            break;
        }
        units += w;
        out.push(c);
    }
    // `.` and ` ` cannot survive step 3. They are trimmed anyway so that a
    // future edit loosening step 3 cannot silently resurrect the Windows
    // trailing-dot bug, which has no error to catch.
    let out = out.trim_end_matches(['-', '_', '.', ' ']);

    // 6. Sanitisation can MANUFACTURE a reserved name: `--CON--` is not
    //    reserved, and trims down to `CON`, which is. So the check runs on the
    //    result as well as on the input.
    if out.is_empty() || is_reserved(out) {
        return None;
    }
    Some(out.to_string())
}

/// Is this a Windows device name, ignoring case, extension, and the trailing
/// dots and spaces Windows itself strips before deciding?
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let stem = stem.trim_matches([' ', '.']);
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

/// `2026-08-15_143207_nocturne`, or `2026-08-15_143207` when the slug does not
/// survive sanitisation.
///
/// The timestamp alone guarantees uniqueness, so the typed name never has to be
/// unique. That is what makes "type `nocturne` once, press Record five times"
/// produce five adjacent folders and **no overwrite dialog ever**.
pub fn folder_name(at: &WallTime, slug: Option<&str>) -> String {
    match slug.and_then(sanitise_slug) {
        Some(s) => format!("{}_{s}", at.folder_prefix()),
        None => at.folder_prefix(),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Roots
// ───────────────────────────────────────────────────────────────────────────

/// Path length reserved for everything Tangent appends below the chosen root.
///
/// The number is `RECORDER-PLAN.md` §9's. The true worst case is
/// [`worst_case_relative_units`], which is 136 — six more than this — and the
/// gap is covered on purpose by [`PATH_LIMIT`] being 250 rather than Windows'
/// 259 usable characters. The check below therefore admits roots up to 120
/// units, and 120 + 136 = 256 still fits. That reconciliation is written down
/// because the two constants look inconsistent until you do the arithmetic, and
/// somebody would eventually "fix" one of them.
pub const PATH_BUDGET: usize = 130;

/// Longest root Tangent will accept, plus the budget. Below `MAX_PATH` (260,
/// including the terminating NUL, so 259 usable) with room to spare.
pub const PATH_LIMIT: usize = 250;

/// Longest root, in UTF-16 units.
pub const MAX_ROOT_UNITS: usize = PATH_LIMIT - PATH_BUDGET;

/// `"-display"` + `".mov"`: the longest tail a media file adds to a take name.
const LONGEST_MEDIA_TAIL: usize = 12;

/// The longest path, in UTF-16 units, that a take can add below its root:
/// separator, take folder, separator, media file.
pub const fn worst_case_relative_units() -> usize {
    1 + MAX_TAKE_NAME_UNITS + 1 + MAX_TAKE_NAME_UNITS + LONGEST_MEDIA_TAIL
}

/// Where takes go when the user has not chosen: `~/Movies/Tangent`,
/// `%USERPROFILE%\Videos\Tangent`, `$XDG_VIDEOS_DIR/Tangent`.
///
/// **Never the Desktop and never the bundle directory.** A recorder that fills
/// somebody's Desktop with folders is remembered for that, and a bundle-relative
/// path is read-only under `/Applications`, is inside the signed seal, and
/// vanishes on update.
///
/// `None` only when the platform's home variable is unset, which on a desktop
/// session means something is badly wrong; the caller must then insist on a
/// chosen folder rather than guessing.
///
/// There is no `dirs` crate here, so this reads the environment directly. The
/// consequence on Windows is real and belongs in the integrator's notes:
/// `FOLDERID_Videos` (which the plan asks for) is the *correct* API because a
/// user can relocate Videos to another drive, and `%USERPROFILE%\Videos` is only
/// the default. This returns the default and is wrong for a relocated folder.
pub fn default_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(PathBuf::from(drive).join(path))
            })?;
        Some(home.join("Videos").join("Tangent"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from(std::env::var_os("HOME")?).join("Movies/Tangent"))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let videos = std::env::var_os("XDG_VIDEOS_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let config = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"));
                let text = fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
                xdg_videos_dir_from(&text, &home)
            })
            .unwrap_or_else(|| home.join("Videos"));
        Some(videos.join("Tangent"))
    }
}

/// Pull `XDG_VIDEOS_DIR` out of the contents of `~/.config/user-dirs.dirs`.
///
/// **`XDG_VIDEOS_DIR` is almost never an environment variable.** It lives in
/// that file, and `xdg-user-dirs-update` writes it localised: a French desktop
/// has `$HOME/Vidéos`, a German one `$HOME/Videos` — which happens to match, and
/// is exactly why testing in English hides the bug. Code that reads only the env
/// var creates a second, English `~/Videos` sitting next to the user's real
/// video folder, and the user's file manager shows neither as the other.
///
/// Public rather than `cfg`-gated so that it is compiled and tested on every
/// platform; a parser that only exists on Linux is a parser only Linux users
/// discover is broken.
pub fn xdg_videos_dir_from(contents: &str, home: &Path) -> Option<PathBuf> {
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(value) = line.strip_prefix("XDG_VIDEOS_DIR=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        // The file's own format uses `$HOME/...`; it is not shell-expanded for
        // us, and a literal `$HOME` directory is not what anyone meant.
        let path = match value.strip_prefix("$HOME/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(value),
        };
        if path.as_os_str().is_empty() {
            return None;
        }
        return Some(path);
    }
    None
}

/// Refuse a root that leaves too little room, **at the moment it is chosen**.
///
/// This is the whole point of the check: `MAX_PATH` is discovered at the moment
/// a file is created, which is the moment a twenty-minute take is being
/// finalised. Discovering it there means the take is already unrecoverable.
/// Discovering it in the folder picker costs the user one click.
pub fn validate_root(root: &Path) -> Result<(), TakeError> {
    let units = path_units(root);
    if units + PATH_BUDGET > PATH_LIMIT {
        return Err(TakeError::RootTooLong {
            units,
            max: MAX_ROOT_UNITS,
        });
    }
    Ok(())
}

/// Validate the root and create it, at Change-folder time.
///
/// `create_dir_all`, because on a fresh machine neither `~/Movies/Tangent` nor —
/// on a minimal Linux install — its parent exists. A permissions failure surfaces
/// here, while the user is still looking at a file picker.
pub fn prepare_root(root: &Path) -> Result<(), TakeError> {
    validate_root(root)?;
    fs::create_dir_all(root).map_err(|source| TakeError::Root {
        path: root.to_path_buf(),
        source,
    })
}

/// Length of a path in UTF-16 code units, which is what `MAX_PATH` counts.
///
/// `to_string_lossy` replaces unpaired surrogates with U+FFFD, one unit each, so
/// the count stays right on Windows. On Unix a non-UTF-8 byte becomes one unit
/// where it occupied one byte, which under-counts — and Unix has no `MAX_PATH`,
/// so nothing depends on it.
fn path_units(path: &Path) -> usize {
    path.to_string_lossy().encode_utf16().count()
}

// ───────────────────────────────────────────────────────────────────────────
// Creation
// ───────────────────────────────────────────────────────────────────────────

/// How many names are tried before giving up: the base plus `-2` … `-99`.
pub const MAX_COLLISION_ATTEMPTS: u32 = 99;

/// The sidecar manifest. A constant name, unlike the media files.
pub const MANIFEST_NAME: &str = "take.json";

/// The plain-text event log for support. Constant for the same reason.
pub const LOG_NAME: &str = "take.log";

/// Written, `sync_all`ed and renamed over [`MANIFEST_NAME`].
const MANIFEST_TMP_NAME: &str = "take.json.tmp";

/// A created take directory: the one thing a take press produces.
#[derive(Debug, Clone)]
pub struct Take {
    dir: PathBuf,
    name: String,
}

impl Take {
    /// Create the root if needed, then create exactly one new take directory.
    ///
    /// The second call is `create_dir`, never `create_dir_all`, and that is the
    /// entire collision story: it is atomic, and its `AlreadyExists` is the only
    /// evidence of a clash that cannot be raced. Real causes are not exotic —
    /// NTP stepping backwards, the DST fall-back hour, a crash-recovery
    /// re-import, a scripted test, and a user hammering Record after a 0.4 s
    /// take.
    ///
    /// `create_dir_all(root)` runs on every take, not just the first, because a
    /// root chosen ten minutes ago can have been deleted since — emptying the
    /// Trash and a cleanup tool both do it — and re-creating it is idempotent
    /// and free next to the take itself.
    pub fn create(root: &Path, at: &WallTime, slug: Option<&str>) -> Result<Self, TakeError> {
        prepare_root(root)?;
        let (dir, name) = create_unique(root, &folder_name(at, slug))?;
        Ok(Self { dir, name })
    }

    /// The directory. Everything a take writes goes inside it and nothing goes
    /// outside it.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `2026-08-15_143207_nocturne`, possibly with a `-2` collision suffix.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `{take name}{suffix}.{ext}` inside the take directory.
    ///
    /// **Media files carry the take name; the sidecar and the log do not.**
    /// Constant names like `video.mp4` are nicer to script and wrong here,
    /// because these files leave their folder constantly — dragged into a
    /// timeline, attached to an email, uploaded. Ten browser tabs called
    /// `video.mp4` is the failure mode. `take.json` and `take.log` keep constant
    /// names because they are only ever read in place.
    pub fn media(&self, suffix: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{}{suffix}.{ext}", self.name))
    }

    /// `{take name}.wav`.
    pub fn wav(&self) -> PathBuf {
        self.media("", "wav")
    }

    /// `{take name}.mid`.
    pub fn midi(&self) -> PathBuf {
        self.media("", "mid")
    }

    /// `{dir}/take.json`.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST_NAME)
    }

    /// `{dir}/take.log`.
    pub fn log_path(&self) -> PathBuf {
        self.dir.join(LOG_NAME)
    }
}

/// Try `base`, then `base-2` … `base-99`, and stop on anything that is not a
/// collision.
///
/// Deliberately does **not** create the root, so that the "parent is missing"
/// path can be proven to fail on the first attempt rather than after
/// ninety-nine. That is not a hypothetical: `NotFound` is exactly what
/// `create_dir` returns for a missing parent, and a loop that treats every error
/// as a collision converts one clear error into ninety-nine syscalls and a final
/// message about a name clash that never happened.
fn create_unique(root: &Path, base: &str) -> Result<(PathBuf, String), TakeError> {
    for n in 1..=MAX_COLLISION_ATTEMPTS {
        let name = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        let dir = root.join(&name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((dir, name)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(TakeError::Create { path: dir, source }),
        }
    }
    Err(TakeError::Collision {
        base: base.to_string(),
        tried: MAX_COLLISION_ATTEMPTS,
    })
}

/// Everything that can stop a take before it starts.
#[derive(Debug)]
pub enum TakeError {
    /// The chosen folder leaves too little room under `MAX_PATH`.
    RootTooLong { units: usize, max: usize },
    /// The destination root could not be created.
    Root { path: PathBuf, source: io::Error },
    /// The take directory could not be created, for a reason that is not a
    /// collision.
    Create { path: PathBuf, source: io::Error },
    /// Ninety-nine names in the same second were all taken.
    Collision { base: String, tried: u32 },
}

impl fmt::Display for TakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootTooLong { units, max } => write!(
                f,
                "that folder's path is {units} characters and the longest usable \
                 here is {max}; recordings would exceed the system path limit \
                 partway through a take. Choose a folder closer to the top of \
                 the drive."
            ),
            Self::Root { path, source } => {
                write!(f, "could not create {}: {source}", path.display())
            }
            Self::Create { path, source } => {
                write!(f, "could not create take folder {}: {source}", path.display())
            }
            Self::Collision { base, tried } => write!(
                f,
                "{tried} folders named {base} already exist; refusing to guess \
                 further rather than risk writing over one"
            ),
        }
    }
}

impl std::error::Error for TakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Root { source, .. } | Self::Create { source, .. } => Some(source),
            _ => None,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// JSON
// ───────────────────────────────────────────────────────────────────────────

/// A JSON value, hand-rolled because there is no `serde` in this crate.
///
/// A tree rather than string concatenation, because the manifest carries strings
/// this process did not author — a take name the user typed, a plugin name and
/// preset read out of somebody else's binary, file names from the filesystem —
/// and one unescaped quote in any of them makes the whole sidecar unparseable.
/// The sidecar is, in the plan's words, "the only thing that will let a user's
/// 'it's out of sync' report be debugged", so it failing to parse is the failure
/// that matters.
///
/// Objects are a `Vec` and not a map so that field order is authored, stable and
/// diffable: two takes' manifests should differ only where the takes did.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn s(v: impl Into<String>) -> Self {
        Self::Str(v.into())
    }

    pub fn obj(pairs: Vec<(&str, Json)>) -> Self {
        Self::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn opt_str(v: Option<&str>) -> Self {
        v.map_or(Self::Null, Self::s)
    }

    pub fn opt_num(v: Option<f64>) -> Self {
        v.map_or(Self::Null, Self::Num)
    }

    pub fn opt_int(v: Option<i64>) -> Self {
        v.map_or(Self::Null, Self::Int)
    }

    /// Pretty, two-space indented, with a trailing newline.
    ///
    /// Pretty rather than compact because the intended reader is a human in a
    /// text editor trying to explain why their video is out of sync, and because
    /// the crash detector below matches one exact line of this layout.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_into(&self, out: &mut String, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => out.push_str(&i.to_string()),
            // JSON has no NaN and no Infinity. `format!("{}", f64::NAN)` writes
            // the bare word `NaN`, which no parser accepts — so one un-fitted
            // rate would take the entire sidecar down with it. A missing number
            // is `null`, which is true and readable.
            Self::Num(n) => {
                if n.is_finite() {
                    out.push_str(&format!("{n}"));
                } else {
                    out.push_str("null");
                }
            }
            Self::Str(s) => push_escaped(out, s),
            Self::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write_into(out, depth + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push(']');
            }
            Self::Obj(pairs) => {
                if pairs.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    indent(out, depth + 1);
                    push_escaped(out, k);
                    out.push_str(": ");
                    v.write_into(out, depth + 1);
                    if i + 1 < pairs.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Escape per RFC 8259 §7.
///
/// The two-character escapes are not optional decoration: a raw newline or tab
/// inside a JSON string is a *parse error*, not merely ugly. Everything else
/// below 0x20 goes out as `\u00XX`, and 0x7F (DEL) with it — legal unescaped,
/// but it renders as nothing at all, and an invisible character in a file name
/// is how a support thread runs for a week.
fn push_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ───────────────────────────────────────────────────────────────────────────
// The manifest
// ───────────────────────────────────────────────────────────────────────────

/// Which of the two timelines in `clock.rs` produced this take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClockKind {
    /// An audio input device was the rate master and `epsilon` was measured.
    Fitted,
    /// Nothing was being sampled by a crystal Tangent does not control, so
    /// `epsilon` is zero **by definition rather than by measurement**. Reporting
    /// this separately is the whole point: 0 ppm from a real fit and 0 ppm from
    /// no fit at all look identical in a number and mean opposite things.
    #[default]
    Synthetic,
}

impl ClockKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fitted => "fitted",
            Self::Synthetic => "synthetic",
        }
    }
}

/// Where a source's latency figure came from.
///
/// §3a demands this field by name. A constant A/V offset is precisely the
/// complaint a sync report without it cannot explain, because "40 ms" tells you
/// nothing about whether the OS said so, a calibration measured it, or nobody
/// knew and the code used zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LatencySource {
    /// The OS reported it (cpal's CoreAudio device + safety offset, say).
    OsReported,
    /// The user ran the sharp-attack calibration in §3a.
    Measured,
    /// Nobody knew. This is the honest default for a UVC webcam, whose true
    /// figure is 20-150 ms.
    #[default]
    AssumedZero,
}

impl LatencySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::OsReported => "os_reported",
            Self::Measured => "measured",
            Self::AssumedZero => "assumed_zero",
        }
    }
}

/// One capture source's clock report.
#[derive(Debug, Clone, Default)]
pub struct SourceReport {
    pub name: String,
    /// The timebase instant this source's stamp zero maps to: its anchor.
    pub anchor_ns: Option<Nanos>,
    /// Measured rate in Hz, where the source has one.
    pub fitted_rate: Option<f64>,
    pub latency_ns: Nanos,
    pub latency_source: LatencySource,
    pub observations: u64,
    /// Peak-to-peak spread of observed offsets: this source's delivery jitter.
    pub jitter_ns: Option<Nanos>,
}

impl SourceReport {
    /// Read everything a [`SourceClock`] knows about itself.
    ///
    /// The anchor is `to_timebase(0)` — where stamp zero lands — which is the
    /// offset with the latency already applied, i.e. the number actually used to
    /// place this source's events. Reporting the raw offset instead would report
    /// a number no event was ever converted with.
    pub fn from_clock(name: impl Into<String>, clock: &SourceClock) -> Self {
        Self {
            name: name.into(),
            anchor_ns: clock.to_timebase(0),
            fitted_rate: None,
            latency_ns: clock.latency(),
            latency_source: LatencySource::AssumedZero,
            observations: clock.observations(),
            jitter_ns: clock.jitter_ns(),
        }
    }

    /// Attach the measured rate from this source's [`RateFit`].
    pub fn with_fit(mut self, fit: &RateFit) -> Self {
        self.fitted_rate = fit.true_rate();
        self
    }

    fn to_json(&self) -> Json {
        Json::obj(vec![
            ("name", Json::s(self.name.clone())),
            ("anchor_ns", Json::opt_int(self.anchor_ns)),
            ("fitted_rate_hz", Json::opt_num(self.fitted_rate)),
            ("latency_ns", Json::Int(self.latency_ns)),
            ("latency_source", Json::s(self.latency_source.as_str())),
            ("observations", Json::Int(self.observations as i64)),
            ("jitter_ns", Json::opt_int(self.jitter_ns)),
        ])
    }
}

/// The audio half of the manifest.
#[derive(Debug, Clone)]
pub struct AudioReport {
    pub nominal_rate: f64,
    /// Measured against the host clock. `None` on a synthetic clock.
    pub true_rate: Option<f64>,
    /// `(R_true / R_nominal - 1) * 1e6`. Zero and meaningless when the clock is
    /// synthetic, which is why [`ClockKind`] is reported beside it.
    pub epsilon_ppm: f64,
    pub channels: u16,
    pub bits: u16,
    pub frames: u64,
    /// Peak level over the whole take. `-inf` for digital silence, which is why
    /// the JSON writer has to handle a non-finite number.
    pub peak_dbfs: f64,
    /// A latch, not a level: it survives the take and is reported after Stop,
    /// because a clip six seconds in is invisible by the time the meter is read.
    pub clipped: bool,
}

impl Default for AudioReport {
    fn default() -> Self {
        Self {
            nominal_rate: 48_000.0,
            true_rate: None,
            epsilon_ppm: 0.0,
            channels: 2,
            bits: 24,
            frames: 0,
            peak_dbfs: f64::NEG_INFINITY,
            clipped: false,
        }
    }
}

/// The video half, including the two fields §7 adds so that a sidecar can say
/// what the muxed audio actually is — Linux ships LPCM in a `.mov` because there
/// is no permissively-licensed AAC encoder, and a report that cannot say so
/// cannot explain a file size either.
#[derive(Debug, Clone, Default)]
pub struct VideoReport {
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// What the nominal frame rate implies over the take's duration.
    pub frames_expected: u64,
    /// What the camera actually delivered. Every USB webcam is VFR in practice;
    /// the gap between these two is the number that explains a stutter.
    pub frames_received: u64,
}

impl VideoReport {
    fn to_json(&self) -> Json {
        Json::obj(vec![
            ("container", Json::s(self.container.clone())),
            ("video_codec", Json::s(self.video_codec.clone())),
            ("audio_codec", Json::s(self.audio_codec.clone())),
            ("width", Json::Int(i64::from(self.width))),
            ("height", Json::Int(i64::from(self.height))),
            ("fps", Json::Num(self.fps)),
            ("frames_expected", Json::Int(self.frames_expected as i64)),
            ("frames_received", Json::Int(self.frames_received as i64)),
        ])
    }
}

/// The MIDI half. `tempo_bpm` is the mark written into the SMF, and it is
/// reported because it is an *assertion* the file makes rather than something
/// measured — importers routinely discard it, and then nobody can tell what the
/// file claimed.
#[derive(Debug, Clone)]
pub struct MidiReport {
    pub tempo_bpm: f64,
    pub ppq: u16,
    pub events: usize,
}

impl Default for MidiReport {
    fn default() -> Self {
        Self {
            tempo_bpm: 120.0,
            ppq: crate::smf::PPQ,
            events: 0,
        }
    }
}

/// The hosted instrument, when there was one.
#[derive(Debug, Clone, Default)]
pub struct PluginReport {
    pub name: String,
    pub preset: Option<String>,
}

/// `take.json`: the manifest and the sync report.
///
/// # `complete` is the crash detector
///
/// It is written `false` at take **start** and flipped at clean stop. Nothing
/// else in the design can tell a finished take from one whose process died
/// mid-file, and the recovery path in §9 — "the next launch sees
/// `complete: false`, finalises what is there and offers it" — has no other
/// input. A manifest written only at stop would be absent exactly when it is
/// needed.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub take: String,
    pub complete: bool,
    /// Set by [`abort`](Manifest::abort). `"audio_device_lost"` is the one §4
    /// names.
    pub aborted_reason: Option<String>,
    pub started: WallTime,
    /// `T0` in the recorder's monotonic timebase, so a log line can be lined up
    /// against the manifest without going through the wall clock.
    pub t0_monotonic_ns: Nanos,
    pub duration_seconds: f64,
    pub clock: ClockKind,
    pub audio: AudioReport,
    pub video: Option<VideoReport>,
    pub midi: MidiReport,
    pub sources: Vec<SourceReport>,
    pub plugin: Option<PluginReport>,
    /// Names of the media files in this directory, in the order they were
    /// written.
    pub files: Vec<String>,
}

impl Manifest {
    /// The manifest as it exists at take start: incomplete, and honest about it.
    pub fn starting(take: &str, started: WallTime, t0: Nanos) -> Self {
        Self {
            take: take.to_string(),
            complete: false,
            aborted_reason: None,
            started,
            t0_monotonic_ns: t0,
            duration_seconds: 0.0,
            clock: ClockKind::Synthetic,
            audio: AudioReport::default(),
            video: None,
            midi: MidiReport::default(),
            sources: Vec::new(),
            plugin: None,
            files: Vec::new(),
        }
    }

    /// Copy the finished [`Timeline`] into the report.
    ///
    /// Deliberately the only way these five fields get set, so that
    /// `clock: "synthetic"` and `epsilon_ppm: 0` can never disagree — a
    /// hand-filled manifest reporting a fitted clock with no fit behind it would
    /// make a broken pipeline look healthy.
    pub fn apply_timeline(&mut self, timeline: &Timeline) {
        self.t0_monotonic_ns = timeline.t0();
        self.duration_seconds = timeline.duration_seconds();
        self.clock = if timeline.is_synthetic() {
            ClockKind::Synthetic
        } else {
            ClockKind::Fitted
        };
        self.audio.nominal_rate = timeline.nominal_rate();
        self.audio.epsilon_ppm = timeline.epsilon_ppm();
        self.audio.true_rate = (!timeline.is_synthetic())
            .then(|| timeline.nominal_rate() * (1.0 + timeline.epsilon()));
    }

    /// Clean stop: flip the crash detector.
    pub fn finish(&mut self) {
        self.complete = true;
        self.aborted_reason = None;
    }

    /// Stopped by something going wrong. `complete` stays `false` on purpose —
    /// the take is real and every byte captured is kept (§4), but it is not a
    /// take that ran to a clean stop and the recovery path must still look at it.
    pub fn abort(&mut self, reason: impl Into<String>) {
        self.complete = false;
        self.aborted_reason = Some(reason.into());
    }

    /// The manifest as a JSON tree, ready to render or to extend.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            (
                "tangent",
                Json::obj(vec![
                    ("version", Json::s(env!("CARGO_PKG_VERSION"))),
                    ("manifest", Json::Int(1)),
                ]),
            ),
            ("take", Json::s(self.take.clone())),
            ("complete", Json::Bool(self.complete)),
            ("aborted_reason", Json::opt_str(self.aborted_reason.as_deref())),
            // The contract, stated in the file because every other tool trims
            // leading silence and then the MIDI does not line up: WAV sample 0,
            // MIDI tick 0 and video frame 0 are the same instant.
            ("zero_is", Json::s("take_start")),
            (
                "started",
                Json::obj(vec![
                    ("iso8601", Json::s(self.started.iso8601())),
                    ("unix_seconds", Json::Int(self.started.unix_seconds)),
                    (
                        "utc_offset_seconds",
                        Json::Int(i64::from(self.started.utc_offset_seconds)),
                    ),
                    ("monotonic_ns", Json::Int(self.t0_monotonic_ns)),
                ]),
            ),
            ("duration_seconds", Json::Num(self.duration_seconds)),
            ("clock", Json::s(self.clock.as_str())),
            (
                "audio",
                Json::obj(vec![
                    ("nominal_rate_hz", Json::Num(self.audio.nominal_rate)),
                    ("true_rate_hz", Json::opt_num(self.audio.true_rate)),
                    ("epsilon_ppm", Json::Num(self.audio.epsilon_ppm)),
                    ("channels", Json::Int(i64::from(self.audio.channels))),
                    ("bits", Json::Int(i64::from(self.audio.bits))),
                    ("frames", Json::Int(self.audio.frames as i64)),
                    ("peak_dbfs", Json::Num(self.audio.peak_dbfs)),
                    ("clipped", Json::Bool(self.audio.clipped)),
                ]),
            ),
            (
                "video",
                self.video.as_ref().map_or(Json::Null, VideoReport::to_json),
            ),
            (
                "midi",
                Json::obj(vec![
                    ("tempo_bpm", Json::Num(self.midi.tempo_bpm)),
                    ("ppq", Json::Int(i64::from(self.midi.ppq))),
                    ("events", Json::Int(self.midi.events as i64)),
                ]),
            ),
            (
                "sources",
                Json::Arr(self.sources.iter().map(SourceReport::to_json).collect()),
            ),
            (
                "plugin",
                self.plugin.as_ref().map_or(Json::Null, |p| {
                    Json::obj(vec![
                        ("name", Json::s(p.name.clone())),
                        ("preset", Json::opt_str(p.preset.as_deref())),
                    ])
                }),
            ),
            (
                "files",
                Json::Arr(self.files.iter().cloned().map(Json::Str).collect()),
            ),
        ])
    }

    /// Render the manifest.
    pub fn render(&self) -> String {
        self.to_json().render()
    }

    /// Write `take_dir/take.json`, atomically.
    ///
    /// Temp file, `sync_all`, then `rename`. Not a plain truncating write: this
    /// file is rewritten at clean stop, and a crash *during* that rewrite would
    /// leave a truncated sidecar — destroying the crash detector at precisely
    /// the moment a crash detector is wanted. `rename` over an existing file is
    /// atomic on every filesystem this ships on, so a reader sees the old
    /// contents or the new ones and never half of either.
    ///
    /// What this does **not** do is fsync the containing directory, because std
    /// has no portable way to. The bounded consequence: after a power cut the
    /// entry may be missing entirely, so the recovery scan must treat *no*
    /// `take.json` exactly as it treats `complete: false` — which
    /// [`is_complete`] already does.
    pub fn write(&self, take_dir: &Path) -> io::Result<()> {
        let tmp = take_dir.join(MANIFEST_TMP_NAME);
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(self.render().as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, take_dir.join(MANIFEST_NAME))
    }
}

/// Did this take reach a clean stop?
///
/// `false` for a missing, unreadable or truncated `take.json`, which is the
/// answer that makes the recovery path safe: every one of those means the
/// process did not finish writing, and offering to finalise a take that was
/// already finalised costs nothing while skipping one that was not loses it.
///
/// It matches a line of our own rendering rather than parsing JSON, and that is
/// sound for a reason worth writing down: [`push_escaped`] escapes every `"`
/// inside a string, so the needle's opening quote can only ever line up with a
/// real key. A take named `"complete": true` renders as `\"complete\": true`,
/// which does not match — there is a backslash where the needle wants a quote.
pub fn is_complete(take_dir: &Path) -> bool {
    fs::read_to_string(take_dir.join(MANIFEST_NAME))
        .is_ok_and(|s| s.contains("\n  \"complete\": true"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ── scratch directories ─────────────────────────────────────────────

    /// A temp directory that removes itself, including when a test panics.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ivory-take-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(unix: i64) -> WallTime {
        WallTime::from_unix(unix, 0)
    }

    // ── the calendar ────────────────────────────────────────────────────

    #[test]
    fn unix_seconds_become_the_calendar_date_the_system_agrees_with() {
        // Every row checked against `date -u -r <secs>` on this machine, so a
        // wrong leap-year rule or a wrong epoch shift fails here rather than
        // naming one folder in four hundred years incorrectly.
        for (secs, iso) in [
            (0i64, "1970-01-01T00:00:00Z"),
            (1_000_000_000, "2001-09-09T01:46:40Z"),
            (1_786_804_327, "2026-08-15T14:32:07Z"),
            // 2000 is a leap year (divisible by 400), 2100 is not (divisible by
            // 100 but not 400). Getting the second wrong is the classic bug and
            // it does not bite for seventy-four years.
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_709_164_800, "2024-02-29T00:00:00Z"),
            (4_102_444_800, "2100-01-01T00:00:00Z"),
        ] {
            assert_eq!(at(secs).iso8601(), iso, "at unix {secs}");
        }
    }

    #[test]
    fn a_clock_stuck_before_the_epoch_does_not_run_backwards() {
        // Truncating division would put every instant on 1969-12-31 into 1970,
        // so the folder for a machine whose RTC battery died would be dated
        // *after* the one recorded a second later.
        assert_eq!(at(-1).iso8601(), "1969-12-31T23:59:59Z");
        assert_eq!(at(-14_182_940).iso8601(), "1969-07-20T20:17:40Z");
        assert_eq!(at(-2_208_988_800).iso8601(), "1900-01-01T00:00:00Z");
        assert!(
            at(-1).folder_prefix() < at(0).folder_prefix(),
            "an earlier instant must sort earlier even across the epoch"
        );
    }

    #[test]
    fn the_offset_shifts_the_wall_clock_and_is_written_into_the_timestamp() {
        let utc = WallTime::from_unix(1_786_804_327, 0);
        let berlin = WallTime::from_unix(1_786_804_327, 2 * 3_600);
        let denver = WallTime::from_unix(1_786_804_327, -6 * 3_600 - 1_800);
        assert_eq!(utc.iso8601(), "2026-08-15T14:32:07Z");
        assert_eq!(berlin.iso8601(), "2026-08-15T16:32:07+02:00");
        assert_eq!(denver.iso8601(), "2026-08-15T08:02:07-06:30");
        assert_eq!(berlin.folder_prefix(), "2026-08-15_163207");
        assert_eq!(
            berlin.unix_seconds, utc.unix_seconds,
            "the offset renames the instant, it does not move it"
        );
    }

    // ── naming ──────────────────────────────────────────────────────────

    #[test]
    fn a_take_folder_sorts_chronologically_as_plain_ascii() {
        let mut names: Vec<String> = [1_786_804_327i64, 1_735_689_600, 1_786_804_328, 0]
            .iter()
            .map(|s| folder_name(&at(*s), Some("nocturne")))
            .collect();
        let chronological = vec![
            folder_name(&at(0), Some("nocturne")),
            folder_name(&at(1_735_689_600), Some("nocturne")),
            folder_name(&at(1_786_804_327), Some("nocturne")),
            folder_name(&at(1_786_804_328), Some("nocturne")),
        ];
        names.sort();
        assert_eq!(names, chronological, "ASCII sort must be chronological sort");
    }

    #[test]
    fn a_folder_name_never_contains_a_colon_or_a_space() {
        // A colon is illegal on Windows and is displayed as `/` by the macOS
        // Finder; a space splits an unquoted shell argument in two.
        let name = folder_name(&at(1_786_804_327), Some("Chopin Nocturne Op. 9 No. 2"));
        assert!(!name.contains(':'), "{name}");
        assert!(!name.contains(' '), "{name}");
        assert_eq!(name, "2026-08-15_143207_Chopin-Nocturne-Op-9-No-2");
    }

    #[test]
    fn a_reserved_device_name_never_becomes_a_folder() {
        for raw in [
            "CON", "con", "Con", "NUL", "nul.txt", "NUL.WAV", "aux", "PRN", "com0", "COM9", "lpt3",
            "LPT0", "con.", "CON ", "con .txt",
        ] {
            assert_eq!(
                sanitise_slug(raw),
                None,
                "{raw:?} opens a Windows device, not a file"
            );
        }
        // Only the exact stem is reserved. A pianist practising for a console
        // game soundtrack keeps their name.
        assert_eq!(sanitise_slug("console"), Some("console".to_string()));
        assert_eq!(sanitise_slug("com10"), Some("com10".to_string()));
        assert_eq!(sanitise_slug("my-con"), Some("my-con".to_string()));
    }

    #[test]
    fn sanitisation_can_manufacture_a_reserved_name_and_the_second_check_catches_it() {
        // `--CON--` is not reserved as typed, so an implementation that checks
        // only the input passes it through, trims it to `CON`, and creates a
        // folder that is the console device on Windows.
        assert_eq!(sanitise_slug("--CON--"), None);
        assert_eq!(sanitise_slug("  nul  "), None);
        assert_eq!(sanitise_slug("...aux..."), None);
    }

    #[test]
    fn a_trailing_dot_or_space_is_removed_because_windows_removes_it_silently() {
        // Windows strips trailing dots and spaces during path normalisation, so
        // `nocturne.` is created as `nocturne` and reopening the string you
        // asked for fails. There is no error to catch, which is why this is a
        // naming rule rather than error handling.
        for raw in ["nocturne.", "nocturne ", "nocturne...  ", "nocturne-", "nocturne_"] {
            let s = sanitise_slug(raw).expect("something must survive");
            assert_eq!(s, "nocturne", "from {raw:?}");
        }
    }

    #[test]
    fn a_leading_dot_never_produces_a_hidden_or_flag_shaped_folder() {
        let s = sanitise_slug(".nocturne").expect("survives");
        assert_eq!(s, "nocturne");
        assert!(!s.starts_with('.'), "a hidden take folder is a lost take");
        assert!(
            !sanitise_slug("--rf").unwrap().starts_with('-'),
            "a leading dash is parsed as an option by every CLI tool"
        );
        let name = folder_name(&at(0), Some(".hidden"));
        assert!(!name.starts_with('.') && !name.starts_with('-'), "{name}");
    }

    #[test]
    fn forbidden_characters_are_deleted_rather_than_each_becoming_a_dash() {
        // `C:\Users\me` mapped one-for-one reads `C--Users-me`; deleted it reads
        // `CUsers-me`, which is at least a name.
        assert_eq!(sanitise_slug("a<b>c").as_deref(), Some("abc"));
        assert_eq!(sanitise_slug("Bach/Gould").as_deref(), Some("BachGould"));
        assert_eq!(sanitise_slug("q\"uote").as_deref(), Some("quote"));
        assert_eq!(sanitise_slug("tab\there").as_deref(), Some("tab-here"));
        assert_eq!(sanitise_slug("nul\u{0}byte").as_deref(), Some("nulbyte"));
    }

    #[test]
    fn runs_collapse_to_one_separator_and_typed_underscores_survive() {
        assert_eq!(sanitise_slug("a   ---  b").as_deref(), Some("a-b"));
        assert_eq!(sanitise_slug("nocturne_v2").as_deref(), Some("nocturne_v2"));
        assert_eq!(sanitise_slug("a__b").as_deref(), Some("a_b"));
        assert_eq!(sanitise_slug("a_-_b").as_deref(), Some("a-b"));
    }

    #[test]
    fn truncation_lands_on_a_character_boundary_rather_than_panicking() {
        // `&s[..40]` does not truncate a multi-byte string, it PANICS. Sixty
        // CJK characters put byte 40 inside a character.
        let long = "\u{845b}".repeat(60);
        let s = sanitise_slug(&long).expect("survives");
        assert_eq!(s.chars().count(), MAX_SLUG_UNITS);
        assert!(s.chars().all(|c| c == '\u{845b}'));

        // U+20000 is a CJK Extension B ideograph: a perfectly ordinary letter
        // that a rare character in a piece's title really is written with, one
        // `char`, and TWO UTF-16 units. MAX_PATH counts units, so budgeting it
        // as one would leave every path reservation short by half.
        let astral = "\u{20000}".repeat(60);
        let a = sanitise_slug(&astral).expect("survives");
        assert_eq!(
            a.encode_utf16().count(),
            MAX_SLUG_UNITS,
            "a non-BMP name must be budgeted in the units MAX_PATH counts"
        );
        assert_eq!(a.chars().count(), MAX_SLUG_UNITS / 2);
    }

    #[test]
    fn truncating_onto_a_separator_does_not_leave_a_trailing_dash() {
        // 39 characters then a separator: the cut lands right after the dash and
        // re-creates the trailing-character bug the earlier trim removed.
        let raw = format!("{}-tail", "x".repeat(39));
        let s = sanitise_slug(&raw).expect("survives");
        assert_eq!(s, "x".repeat(39));
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn a_slug_with_nothing_usable_in_it_leaves_a_timestamp_only_folder() {
        for raw in [
            "", "   ", "///", "...", "-", "\u{0}\u{1}",
            // A pure-emoji name. Emoji are symbols rather than letters, so they
            // collapse away like any other punctuation, and a path that gets
            // pasted into a shell is better off without them. The name is not
            // lost silently: §5's band shows the resulting folder as live grey
            // text under the field.
            "\u{1f3b9}\u{1f3b9}",
        ] {
            assert_eq!(sanitise_slug(raw), None, "{raw:?}");
        }
        assert_eq!(
            sanitise_slug("nocturne \u{1f3b9}").as_deref(),
            Some("nocturne"),
            "an emoji beside a real name costs the emoji, not the name"
        );
        assert_eq!(folder_name(&at(0), Some("///")), "1970-01-01_000000");
        assert_eq!(folder_name(&at(0), None), "1970-01-01_000000");
    }

    // ── roots and path budget ───────────────────────────────────────────

    #[test]
    fn the_longest_take_this_module_can_name_still_fits_under_max_path() {
        // The reason PATH_BUDGET (130) and the true worst case (136) are allowed
        // to disagree: PATH_LIMIT is 250 rather than the 259 usable characters
        // MAX_PATH permits, and the nine characters of slack absorb the six.
        // Raise MAX_SLUG_UNITS or add a longer media suffix and this fails.
        let worst = MAX_ROOT_UNITS + worst_case_relative_units();
        assert!(worst < 260, "worst case path is {worst} units");
        // The directory itself is checked against the lower limit that
        // CreateDirectory enforces, which reserves 12 characters for an 8.3 name.
        let deepest_dir = MAX_ROOT_UNITS + 1 + MAX_TAKE_NAME_UNITS;
        assert!(deepest_dir < 248, "deepest take directory is {deepest_dir}");
    }

    #[test]
    fn a_root_too_long_is_refused_when_it_is_chosen_not_after_a_take() {
        let ok = PathBuf::from(format!("/{}", "a".repeat(MAX_ROOT_UNITS - 1)));
        assert!(validate_root(&ok).is_ok(), "a root at the limit is usable");

        let too_long = PathBuf::from(format!("/{}", "a".repeat(MAX_ROOT_UNITS)));
        match validate_root(&too_long) {
            Err(TakeError::RootTooLong { units, max }) => {
                assert_eq!(units, MAX_ROOT_UNITS + 1);
                assert_eq!(max, MAX_ROOT_UNITS);
            }
            other => panic!("expected RootTooLong, got {other:?}"),
        }
    }

    #[test]
    fn the_default_root_is_never_the_desktop_and_never_inside_a_bundle() {
        let Some(root) = default_root() else {
            return; // no HOME in this environment; nothing to assert
        };
        assert!(root.ends_with("Tangent"), "{}", root.display());
        for bad in ["Desktop", "Contents", "MacOS", "Resources"] {
            assert!(
                !root.components().any(|c| c.as_os_str() == bad),
                "{} must not contain {bad}",
                root.display()
            );
        }
    }

    #[test]
    fn the_videos_folder_comes_from_user_dirs_not_from_an_english_guess() {
        // XDG_VIDEOS_DIR is almost never exported; reading only the env var
        // creates an English `~/Videos` beside the user's real localised one.
        let home = Path::new("/home/aline");
        let file = "# generated\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n\
                    XDG_VIDEOS_DIR=\"$HOME/Vidéos\"\n";
        assert_eq!(
            xdg_videos_dir_from(file, home),
            Some(PathBuf::from("/home/aline/Vidéos"))
        );
        assert_eq!(
            xdg_videos_dir_from("XDG_VIDEOS_DIR=\"/media/big/vids\"\n", home),
            Some(PathBuf::from("/media/big/vids"))
        );
        assert_eq!(
            xdg_videos_dir_from("#XDG_VIDEOS_DIR=\"$HOME/nope\"\n", home),
            None,
            "a commented-out line is not a setting"
        );
        assert_eq!(xdg_videos_dir_from("XDG_MUSIC_DIR=\"$HOME/M\"\n", home), None);
    }

    // ── creation ────────────────────────────────────────────────────────

    #[test]
    fn creating_a_take_also_creates_a_root_that_has_never_existed() {
        // ~/Movies/Tangent does not exist on a fresh machine, and on a minimal
        // Linux install neither does its parent. Without create_dir_all the very
        // first take on every install fails with an error naming the take folder.
        let s = Scratch::new("fresh");
        let root = s.path().join("never").join("existed").join("Tangent");
        let take = Take::create(&root, &at(1_786_804_327), Some("nocturne")).expect("create");
        assert!(take.dir().is_dir());
        assert_eq!(take.name(), "2026-08-15_143207_nocturne");
    }

    #[test]
    fn a_collision_takes_the_next_suffix_instead_of_writing_over_a_take() {
        let s = Scratch::new("collide");
        let when = at(1_786_804_327);
        let first = Take::create(s.path(), &when, Some("nocturne")).expect("first");
        fs::write(first.wav(), b"first take audio").expect("write");

        let second = Take::create(s.path(), &when, Some("nocturne")).expect("second");
        assert_eq!(first.name(), "2026-08-15_143207_nocturne");
        assert_eq!(second.name(), "2026-08-15_143207_nocturne-2");
        assert_ne!(first.dir(), second.dir());
        assert_eq!(
            fs::read(first.wav()).expect("still there"),
            b"first take audio",
            "create_dir_all would have succeeded here and the second take would \
             have written over the first"
        );

        let third = Take::create(s.path(), &when, Some("nocturne")).expect("third");
        assert_eq!(third.name(), "2026-08-15_143207_nocturne-3");
    }

    #[test]
    fn ninety_nine_collisions_fail_loudly_rather_than_silently_reusing_one() {
        let s = Scratch::new("exhaust");
        let when = at(0);
        for _ in 0..MAX_COLLISION_ATTEMPTS {
            Take::create(s.path(), &when, None).expect("within the retry budget");
        }
        match Take::create(s.path(), &when, None) {
            Err(TakeError::Collision { tried, .. }) => assert_eq!(tried, MAX_COLLISION_ATTEMPTS),
            other => panic!("expected Collision, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_parent_fails_at_once_instead_of_retrying_ninety_nine_times() {
        // create_dir returns NotFound, not AlreadyExists, for a missing parent.
        // A loop that retries on every error turns one clear cause into
        // ninety-nine syscalls and a final message about a name clash that never
        // happened.
        let s = Scratch::new("noparent");
        let absent = s.path().join("does-not-exist");
        match create_unique(&absent, "2026-08-15_143207") {
            Err(TakeError::Create { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected an immediate Create/NotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_root_that_cannot_be_created_is_reported_as_a_root_failure() {
        // The root's parent is a regular file, so create_dir_all cannot succeed.
        // The user needs to be told about the folder they chose, not about a
        // take name they never saw.
        let s = Scratch::new("notadir");
        let file = s.path().join("this-is-a-file");
        fs::write(&file, b"x").expect("write");
        match Take::create(&file.join("Tangent"), &at(0), None) {
            Err(TakeError::Root { path, .. }) => assert!(path.ends_with("Tangent")),
            other => panic!("expected Root, got {other:?}"),
        }
    }

    #[test]
    fn media_files_carry_the_take_name_and_the_sidecar_does_not() {
        let s = Scratch::new("names");
        let take = Take::create(s.path(), &at(1_786_804_327), Some("nocturne")).expect("create");
        let n = "2026-08-15_143207_nocturne";
        assert!(take.wav().ends_with(format!("{n}.wav")));
        assert!(take.midi().ends_with(format!("{n}.mid")));
        assert!(take.media("-camera", "mp4").ends_with(format!("{n}-camera.mp4")));
        // Ten browser tabs called `video.mp4` is the failure this prevents; the
        // sidecar is exempt because it is only ever read in place.
        assert!(take.manifest_path().ends_with("take.json"));
        assert!(take.log_path().ends_with("take.log"));
    }

    // ── JSON ────────────────────────────────────────────────────────────

    #[test]
    fn json_escapes_quotes_backslashes_and_control_characters() {
        // These strings do not come from the sanitiser. `plugin.name` and
        // `preset` are read out of somebody else's binary and `files` comes off
        // the filesystem, so one unescaped quote takes the whole sync report
        // down with it.
        let v = Json::obj(vec![(
            "take",
            Json::s("a\"b\\c\nd\te\u{1}f\u{7f}g\u{8}h\u{c}i"),
        )]);
        assert_eq!(
            v.render(),
            "{\n  \"take\": \"a\\\"b\\\\c\\nd\\te\\u0001f\\u007fg\\bh\\fi\"\n}\n"
        );
    }

    #[test]
    fn a_non_finite_number_becomes_null_rather_than_invalid_json() {
        // Digital silence is -inf dBFS and an un-fitted rate is NaN. Rust writes
        // those as the bare words `-inf` and `NaN`, which no JSON parser accepts
        // — so one silent take would make its own sync report unreadable.
        let v = Json::obj(vec![
            ("peak_dbfs", Json::Num(f64::NEG_INFINITY)),
            ("rate", Json::Num(f64::NAN)),
            ("real", Json::Num(48_000.5)),
        ]);
        let out = v.render();
        assert!(!out.contains("inf") && !out.contains("NaN"), "{out}");
        assert_eq!(out.matches("null").count(), 2, "{out}");
        assert!(out.contains("48000.5"), "{out}");
    }

    #[test]
    fn empty_containers_render_as_valid_json() {
        assert_eq!(Json::Arr(vec![]).render(), "[]\n");
        assert_eq!(Json::Obj(vec![]).render(), "{}\n");
    }

    // ── the manifest ────────────────────────────────────────────────────

    fn manifest() -> Manifest {
        Manifest::starting(
            "2026-08-15_143207_nocturne",
            WallTime::from_unix(1_786_804_327, 0),
            1_234_567_890,
        )
    }

    #[test]
    fn the_manifest_written_at_take_start_says_it_is_incomplete() {
        let s = Scratch::new("manifest");
        let m = manifest();
        assert!(!m.complete, "a take in progress is never complete");
        m.write(s.path()).expect("write");
        assert!(
            !is_complete(s.path()),
            "the crash detector must read false while the take is running"
        );
    }

    #[test]
    fn a_clean_stop_flips_the_crash_detector_and_an_abort_does_not() {
        let s = Scratch::new("flip");
        let mut m = manifest();
        m.write(s.path()).expect("start");

        m.finish();
        m.write(s.path()).expect("stop");
        assert!(is_complete(s.path()));

        m.abort("audio_device_lost");
        m.write(s.path()).expect("abort");
        assert!(
            !is_complete(s.path()),
            "a device that vanished mid-take did not produce a clean stop"
        );
        assert!(m.render().contains("audio_device_lost"));
    }

    #[test]
    fn a_directory_with_no_manifest_at_all_reads_as_incomplete() {
        // A power cut can lose the directory entry itself, and the recovery scan
        // must treat that exactly as it treats `complete: false` — offering to
        // finalise a finished take costs nothing, skipping an unfinished one
        // loses it.
        let s = Scratch::new("nomanifest");
        assert!(!is_complete(s.path()));
        fs::write(s.path().join(MANIFEST_NAME), b"{ truncated").expect("write");
        assert!(!is_complete(s.path()));
    }

    #[test]
    fn a_take_name_impersonating_the_complete_flag_does_not_fool_the_detector() {
        let s = Scratch::new("impersonate");
        let mut m = manifest();
        m.take = "\n  \"complete\": true".to_string();
        m.write(s.path()).expect("write");
        assert!(
            !is_complete(s.path()),
            "escaping means the needle's quote can only line up with a real key"
        );
    }

    #[test]
    fn writing_the_manifest_leaves_no_temp_file_behind() {
        let s = Scratch::new("atomic");
        manifest().write(s.path()).expect("write");
        assert!(s.path().join(MANIFEST_NAME).is_file());
        assert!(
            !s.path().join(MANIFEST_TMP_NAME).exists(),
            "the temp file must be renamed away, not left in the take"
        );
    }

    #[test]
    fn the_manifest_reports_a_synthetic_clock_rather_than_a_zero_ppm_measurement() {
        // 0 ppm from a real fit and 0 ppm from no fit at all are the same number
        // and opposite facts. Without the clock field a pipeline that never
        // fitted anything looks perfectly healthy.
        let mut m = manifest();
        m.apply_timeline(&Timeline::synthetic(0, 1_000_000_000, 48_000.0));
        assert_eq!(m.clock, ClockKind::Synthetic);
        assert_eq!(m.audio.true_rate, None);
        assert!(m.render().contains("\"clock\": \"synthetic\""));

        let mut fit = RateFit::new();
        for k in 0..2_000u64 {
            let sample = k * 512;
            fit.push(sample, (sample as f64 / 48_002.4 * 1e9) as Nanos);
        }
        m.apply_timeline(&Timeline::from_fit(0, 1_200_000_000_000, 48_000.0, &fit));
        assert_eq!(m.clock, ClockKind::Fitted);
        assert!((m.audio.epsilon_ppm - 50.0).abs() < 0.5, "{}", m.audio.epsilon_ppm);
        assert!((m.audio.true_rate.expect("fitted") - 48_002.4).abs() < 0.1);
        assert!((m.duration_seconds - 1_200.0).abs() < 0.2);
    }

    #[test]
    fn the_manifest_carries_the_fields_a_sync_complaint_cannot_be_debugged_without() {
        let mut m = manifest();
        let mut clock = SourceClock::new(crate::clock::MIDIR_SCALE_NS, 2_000_000_000);
        clock.observe(1_000, 1_000_000);
        clock.set_latency(40_000_000);
        m.sources
            .push(SourceReport::from_clock("midi", &clock));
        m.video = Some(VideoReport {
            container: "mov".to_string(),
            video_codec: "h264".to_string(),
            audio_codec: "lpcm".to_string(),
            frames_expected: 36_000,
            frames_received: 35_997,
            ..VideoReport::default()
        });
        m.plugin = Some(PluginReport {
            name: "Pianoteq 8".to_string(),
            preset: Some("D4 Classical".to_string()),
        });
        m.files.push("2026-08-15_143207_nocturne.wav".to_string());
        let out = m.render();

        for field in [
            "\"zero_is\": \"take_start\"",
            "\"complete\": false",
            "\"epsilon_ppm\"",
            "\"container\": \"mov\"",
            "\"audio_codec\": \"lpcm\"",
            "\"latency_source\": \"assumed_zero\"",
            "\"latency_ns\": 40000000",
            "\"frames_expected\": 36000",
            "\"frames_received\": 35997",
            "\"monotonic_ns\": 1234567890",
            "\"iso8601\": \"2026-08-15T14:32:07Z\"",
            "\"tempo_bpm\": 120",
            "\"ppq\": 960",
            "Pianoteq 8",
        ] {
            assert!(out.contains(field), "manifest is missing {field}:\n{out}");
        }
    }

    #[test]
    fn a_sources_fit_is_reported_as_a_measured_rate() {
        let mut fit = RateFit::new();
        for k in 0..1_000u64 {
            let sample = k * 512;
            fit.push(sample, (sample as f64 / 44_101.0 * 1e9) as Nanos);
        }
        let clock = SourceClock::new(1.0, 0);
        let report = SourceReport::from_clock("audio-in", &clock).with_fit(&fit);
        assert!(
            (report.fitted_rate.expect("fitted") - 44_101.0).abs() < 1.0,
            "{:?}",
            report.fitted_rate
        );
        assert_eq!(
            report.anchor_ns, None,
            "a clock with no observations has no anchor and must not invent one"
        );
    }
}
