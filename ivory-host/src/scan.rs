//! Finding VST3 bundles on disk and reading what they claim to be.
//!
//! This is the first half of hosting: locate the modules, open them, and ask
//! the factory what classes it exports. Instantiating a component and pulling
//! audio comes next; nothing here creates a processor.
//!
//! # The bundle shape, which is not obvious
//!
//! A `.vst3` is a **directory**, not a file, on every platform since VST 3.6.10
//! — Steinberg made the Windows and Linux layouts match macOS's. The binary
//! lives at:
//!
//! ```text
//! macOS    Foo.vst3/Contents/MacOS/Foo
//! Windows  Foo.vst3/Contents/x86_64-win/Foo.vst3
//! Linux    Foo.vst3/Contents/x86_64-linux/Foo.so
//! ```
//!
//! and the executable inside has **no extension on macOS**, which is why
//! globbing for a file rather than reading `Contents` finds nothing.
//!
//! # The macOS entry points, which are the actual trap
//!
//! On Windows and Linux you `dlopen` and call `GetPluginFactory`. On macOS that
//! is **not sufficient**: the bundle must first be initialised with
//! `bundleEntry(CFBundleRef)`, and plugins are entitled to do real work there —
//! Pianoteq and the Arturia plugins both locate their sample and preset
//! directories relative to the bundle. Skipping it gets you a factory pointer
//! that segfaults later rather than an error now.
//!
//! Symmetrically, `bundleExit()` must be called before unloading, and **not
//! calling it is safer than calling it at the wrong time**: a plugin that has
//! spawned threads in `bundleEntry` will use-after-free if the library is
//! unmapped from under them. `Module` therefore does the pairing itself and
//! deliberately leaks on the failure path.

use std::collections::HashSet;
use std::ffi::{c_void, CString, OsStr};
use std::path::{Path, PathBuf};

use vst3::Steinberg::{
    IPluginFactory, IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo,
    PClassInfo2, PFactoryInfo,
};
use vst3::ComPtr;

/// Where a VST3 bundle's loadable binary lives inside it, per platform.
#[cfg(target_os = "macos")]
const CONTENTS_SUBDIR: &str = "MacOS";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const CONTENTS_SUBDIR: &str = "x86_64-win";
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const CONTENTS_SUBDIR: &str = "arm64-win";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const CONTENTS_SUBDIR: &str = "x86_64-linux";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const CONTENTS_SUBDIR: &str = "aarch64-linux";

/// The standard search paths, in the order the SDK specifies.
///
/// User-local first, so a user's own build shadows a system one — which is what
/// every host does and what anyone debugging a plugin expects.
///
/// `VST3_PATH` comes first of all when it is set. It is the SDK's own documented
/// override, every serious host honours it, and it is the one thing somebody
/// with plugins in an unusual place will already have tried.
pub fn search_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(env) = std::env::var_os("VST3_PATH") {
        out.extend(std::env::split_paths(&env));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(&home).join("Library/Audio/Plug-Ins/VST3"));
        }
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        out.push(PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(PathBuf::from(local).join("Programs/Common/VST3"));
        }
        if let Some(pf) = std::env::var_os("CommonProgramFiles") {
            out.push(PathBuf::from(pf).join("VST3"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(&home).join(".vst3"));
        }
        out.push(PathBuf::from("/usr/lib/vst3"));
        out.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    out
}

/// How deep to walk under a search path.
///
/// **Not one level, which is what this used to do.** The SDK allows vendor and
/// category folders under the VST3 directory and the big installers all use
/// them — Steinberg, Native Instruments, Waves, Applied Acoustics, Kilohearts.
/// On the machine this was found on, 112 of 160 installed plugins were at the
/// top level and the other 48 — HALion Sonic, Groove Agent, Lounge Lizard —
/// simply did not exist as far as this app was concerned, with nothing on
/// screen to suggest they were being skipped.
///
/// Three is deep enough for `VST3/Vendor/Category/Thing.vst3` and shallow
/// enough that it cannot turn into a walk of somebody's home directory if they
/// add one as a custom folder.
const MAX_DEPTH: usize = 3;

/// A cap on how many directories one scan will open.
///
/// The depth limit bounds the SHAPE of the walk and this bounds its SIZE, which
/// are different failures: a custom folder pointed at a network volume or a
/// build tree can be three levels deep and still contain tens of thousands of
/// directories, and a picker that takes a minute to open is a hung app.
const MAX_DIRS: usize = 4_000;

/// Every `.vst3` bundle under the standard paths, plus any the user added.
///
/// Recursive, bounded, symlink-safe and deduplicated — see [`MAX_DEPTH`],
/// [`MAX_DIRS`], and the `seen` set, which holds canonical paths so that a
/// folder reachable two ways (a symlink into the system directory is the common
/// one) is walked once and listed once.
pub fn discover_in(extra: &[PathBuf]) -> Vec<PathBuf> {
    // The user's own folders first, so a plugin they pointed at deliberately
    // shadows a copy of the same thing in a system directory.
    let mut roots: Vec<PathBuf> = extra.to_vec();
    roots.extend(search_paths());

    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut opened = 0_usize;
    let mut queue: Vec<(PathBuf, usize)> = roots.into_iter().map(|d| (d, 0)).collect();

    while let Some((dir, depth)) = queue.pop() {
        if opened >= MAX_DIRS {
            break;
        }
        // Canonicalised, so a symlink loop terminates and a directory reachable
        // by two names is not scanned twice. A path that will not canonicalise
        // does not exist, which is the normal case for a search path on a
        // machine that has never had that vendor's installer run.
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        if !seen.insert(real.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&real) else {
            continue;
        };
        opened += 1;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("vst3")) {
                // A bundle is a directory and is NOT descended into: the
                // binary inside it is not another plugin, and some bundles
                // carry a `Resources` tree deep enough to matter.
                out.push(path);
                continue;
            }
            // `is_dir` follows symlinks, which is what we want: people symlink
            // their plugin folders, and refusing to follow one would be the
            // same bug this whole change is about.
            if depth + 1 <= MAX_DEPTH && path.is_dir() {
                queue.push((path, depth + 1));
            }
        }
    }

    // By canonical path, so the same bundle reached through a symlinked folder
    // is one entry. Sorted by the path shown, so the picker is stable.
    let mut unique: HashSet<PathBuf> = HashSet::new();
    out.retain(|p| unique.insert(p.canonicalize().unwrap_or_else(|_| p.clone())));
    out.sort();
    out
}

/// Every `.vst3` bundle under the standard paths.
pub fn discover() -> Vec<PathBuf> {
    discover_in(&[])
}

/// The binary inside a bundle, or the bundle itself if it is already a plain
/// shared library (which some Linux builds still are).
pub fn binary_in_bundle(bundle: &Path) -> Option<PathBuf> {
    if bundle.is_file() {
        return Some(bundle.to_path_buf());
    }
    let dir = bundle.join("Contents").join(CONTENTS_SUBDIR);
    let entries = std::fs::read_dir(dir).ok()?;
    // The executable is named after the bundle but not reliably so — some
    // plugins ship a differently-cased or renamed binary. Take the first real
    // file and ignore the `_CodeSignature` and `Resources` siblings.
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
}

/// What a plugin class advertises about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInfo {
    pub name: String,
    /// `"Audio Module Class"` for a processor, `"Component Controller Class"`
    /// for its editor half. An instrument is the former.
    ///
    /// **It does not say what KIND of processor.** A synth and a reverb are
    /// both "Audio Module Class"; see [`sub_categories`](Self::sub_categories).
    pub category: String,
    /// The `|`-separated list a plugin uses to say what it is —
    /// `"Instrument|Synth"`, `"Fx|Reverb"`, `"Fx|Instrument"` for the rare
    /// thing that is both.
    ///
    /// **Empty when the factory is too old to be asked.** `subCategories` is a
    /// `PClassInfo2` field, reachable only through `IPluginFactory2`, and a
    /// factory that does not implement it tells us nothing — which must read as
    /// "unknown" rather than as "not an instrument". See [`kind`](Self::kind).
    pub sub_categories: String,
    pub cid: [u8; 16],
}

/// What a plugin is for, as far as it will say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// It makes sound from notes. This is what a slot can play.
    Instrument,
    /// It changes sound it is given. A slot feeds it nothing, so it would sit
    /// there silent — which is exactly the complaint this enum exists to answer.
    Effect,
    /// It did not say, and nothing may be assumed. Treated as an instrument,
    /// because that is what every build before this one did with everything.
    Unknown,
}

impl ClassInfo {
    /// Whether this is the audio-processing half, as opposed to a controller or
    /// something else the factory happens to export.
    pub fn is_audio_module(&self) -> bool {
        self.category == "Audio Module Class"
    }

    /// What the plugin says it is for.
    ///
    /// **Instrument wins a tie.** A handful of plugins declare `Fx|Instrument`
    /// — samplers with an audio input, mostly — and those genuinely do play
    /// notes, so refusing them would be a regression dressed as a fix.
    pub fn kind(&self) -> Kind {
        let has = |needle: &str| {
            self.sub_categories
                .split('|')
                .any(|part| part.trim().eq_ignore_ascii_case(needle))
        };
        if has("Instrument") {
            Kind::Instrument
        } else if has("Fx") {
            Kind::Effect
        } else {
            Kind::Unknown
        }
    }
}

/// A loaded VST3 module: the dynamic library plus its factory.
///
/// Deliberately not `Clone` and deliberately not `Send`. The SDK requires that
/// the factory be used from the thread that initialised the bundle, and a
/// second `Module` for the same path would call `bundleEntry` twice.
pub struct Module {
    path: PathBuf,
    /// Kept alive for as long as the factory is. Dropping this unmaps the
    /// library, and every COM pointer into it becomes a dangling function
    /// table.
    _lib: Library,
    factory: ComPtr<IPluginFactory>,
    vendor: String,
    url: String,
}

/// Hand-written rather than derived: `ComPtr` and the raw handles inside
/// `Library` are not `Debug`, and printing a function-table address would be
/// noise. What is useful in a log is which bundle, from whom, and what it
/// exports.
impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module")
            .field("path", &self.path)
            .field("vendor", &self.vendor)
            .field("classes", &self.classes().len())
            .finish()
    }
}

impl Module {
    /// Open a bundle and get its factory.
    ///
    /// # Safety of the whole operation
    ///
    /// This loads and executes arbitrary third-party code in this process. A
    /// crash inside a plugin is a crash of Tangent, and takes the user's take
    /// with it. That is a known and accepted cost for now — see
    /// `docs/RECORDER-PLAN.md` §8 on moving hosting out of process — but it is
    /// the reason nothing here should ever run while a take is recording.
    pub fn open(bundle: &Path) -> Result<Self, String> {
        let binary = binary_in_bundle(bundle)
            .ok_or_else(|| format!("no loadable binary inside {}", bundle.display()))?;

        let lib = Library::open(&binary, bundle)?;

        // SAFETY: the symbol is the VST3 ABI's single required export. A module
        // without it is not a VST3, which is what the error says.
        let get_factory: unsafe extern "system" fn() -> *mut IPluginFactory =
            unsafe { lib.symbol(b"GetPluginFactory\0") }
                .ok_or_else(|| format!("{} exports no GetPluginFactory", binary.display()))?;

        // SAFETY: calling the plugin's own factory entry point, after
        // bundleEntry has succeeded.
        let raw = unsafe { get_factory() };
        if raw.is_null() {
            return Err(format!("{} returned a null factory", binary.display()));
        }
        // SAFETY: the factory is returned with a reference already added, which
        // `from_raw` takes ownership of rather than adding a second.
        let factory = unsafe { ComPtr::from_raw(raw) }
            .ok_or_else(|| format!("{} returned a null factory", binary.display()))?;

        let mut info = PFactoryInfo {
            vendor: [0; 64],
            url: [0; 256],
            email: [0; 128],
            flags: 0,
        };
        // SAFETY: `info` is a valid, fully-initialised out-parameter.
        unsafe {
            factory.getFactoryInfo(&mut info);
        }

        Ok(Self {
            path: bundle.to_path_buf(),
            _lib: lib,
            factory,
            vendor: c_array_to_string(&info.vendor),
            url: c_array_to_string(&info.url),
        })
    }

    /// The factory, for . Crate-visible rather than public: a
    /// raw COM pointer is not something a caller outside this crate should be
    /// holding, and everything they need is on  and .
    pub(crate) fn factory(&self) -> &ComPtr<IPluginFactory> {
        &self.factory
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Every class the factory exports.
    ///
    /// **Asked twice, and the second question is optional.** `getClassInfo`
    /// answers on every factory ever shipped and does not say whether a class
    /// is a synth or a reverb; `getClassInfo2` says, and exists only on
    /// `IPluginFactory2`. So the cheap one is authoritative for identity and
    /// the richer one is consulted for kind when it is there — a factory that
    /// is too old simply leaves `sub_categories` empty, which reads as
    /// "unknown" and behaves exactly as this app did before.
    pub fn classes(&self) -> Vec<ClassInfo> {
        // SAFETY: the factory is a live COM pointer for the module's lifetime.
        let count = unsafe { self.factory.countClasses() };
        // Queried once for the whole enumeration rather than per class: it is a
        // `QueryInterface` and the answer cannot change between two classes of
        // one factory.
        let richer = self.factory.cast::<IPluginFactory2>();
        let mut out = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count {
            let mut info = PClassInfo {
                cid: [0; 16],
                cardinality: 0,
                category: [0; 32],
                name: [0; 64],
            };
            // SAFETY: `i` is in range and `info` is a valid out-parameter.
            let result = unsafe { self.factory.getClassInfo(i, &mut info) };
            if result != vst3::Steinberg::kResultOk {
                continue;
            }
            let sub_categories = richer
                .as_ref()
                .and_then(|f| {
                    let mut two = PClassInfo2 {
                        cid: [0; 16],
                        cardinality: 0,
                        category: [0; 32],
                        name: [0; 64],
                        classFlags: 0,
                        subCategories: [0; 128],
                        vendor: [0; 64],
                        version: [0; 64],
                        sdkVersion: [0; 64],
                    };
                    // SAFETY: same index, and `two` is a valid out-parameter.
                    // A factory that implements the interface but refuses the
                    // call is treated as one that never had it.
                    let ok = unsafe { f.getClassInfo2(i, &mut two) };
                    (ok == vst3::Steinberg::kResultOk)
                        .then(|| c_array_to_string(&two.subCategories))
                })
                .unwrap_or_default();
            out.push(ClassInfo {
                name: c_array_to_string(&info.name),
                category: c_array_to_string(&info.category),
                sub_categories,
                // A plain reinterpretation of sixteen bytes whose only
                // difference is signedness — and `c_char` rather than `i8`
                // because that signedness is the thing that varies by target.
                cid: unsafe {
                    std::mem::transmute::<[std::os::raw::c_char; 16], [u8; 16]>(info.cid)
                },
            });
        }
        out
    }

    /// The audio-processing classes, which is what an instrument is.
    pub fn audio_modules(&self) -> Vec<ClassInfo> {
        self.classes().into_iter().filter(ClassInfo::is_audio_module).collect()
    }
}

/// Fixed-size C string fields in the SDK's structs are `char8` (i8) arrays that
/// are NOT guaranteed to be NUL-terminated when full.
/// A NUL-terminated C string out of a fixed-size field.
///
/// `&[c_char]` and not `&[i8]`: `char` is signed on x86 and unsigned on
/// aarch64, so an `i8` signature type-checks on half the targets and fails to
/// compile on the other half. The bytes are the same either way.
fn c_array_to_string(raw: &[std::os::raw::c_char]) -> String {
    let bytes: Vec<u8> = raw.iter().map(|c| *c as u8).collect();
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

// ── The platform loader ──────────────────────────────────────────────────────

/// A dynamically loaded module, with the macOS bundle dance done correctly.
struct Library {
    handle: *mut c_void,
    #[cfg(target_os = "macos")]
    bundle: *mut c_void,
    /// Whether `bundleEntry` succeeded and `bundleExit` is therefore owed.
    entered: bool,
}

impl Library {
    fn open(binary: &Path, #[allow(unused)] bundle_path: &Path) -> Result<Self, String> {
        let c_path = CString::new(binary.as_os_str().as_encoded_bytes())
            .map_err(|_| "path contains a NUL".to_string())?;

        // SAFETY: `c_path` is a valid NUL-terminated path. RTLD_LOCAL keeps the
        // plugin's symbols out of the global namespace, which matters when two
        // plugins built against different versions of the same framework are
        // loaded at once — the classic cause of "loading plugin B breaks plugin
        // A" in every host that gets this wrong.
        let handle = load(&c_path, binary)?;

        let mut lib = Self {
            handle,
            #[cfg(target_os = "macos")]
            bundle: std::ptr::null_mut(),
            entered: false,
        };

        #[cfg(target_os = "macos")]
        {
            // bundleEntry takes the CFBundleRef of the .vst3 itself. Plugins use
            // it to find their resources; Pianoteq and Arturia both do.
            let cf_bundle = unsafe { cfbundle_for(bundle_path) }?;
            lib.bundle = cf_bundle;
            // SAFETY: freshly dlopen'd handle; the symbol is optional in the
            // sense that a badly-built bundle may lack it, which is an error.
            let entry: unsafe extern "system" fn(*mut c_void) -> bool =
                unsafe { lib.symbol(b"bundleEntry\0") }
                    .ok_or_else(|| "bundle exports no bundleEntry".to_string())?;
            // SAFETY: handing the plugin its own CFBundleRef, which is what the
            // ABI asks for.
            if !unsafe { entry(cf_bundle) } {
                return Err("bundleEntry returned false".to_string());
            }
            lib.entered = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Windows: InitDll. Linux: ModuleEntry. Both are optional in
            // practice — many plugins omit them — so a missing symbol is not an
            // error, unlike macOS's bundleEntry.
            #[cfg(target_os = "windows")]
            let name: &[u8] = b"InitDll\0";
            #[cfg(target_os = "linux")]
            let name: &[u8] = b"ModuleEntry\0";
            // SAFETY: freshly loaded handle.
            if let Some(entry) = unsafe { lib.symbol::<unsafe extern "system" fn() -> bool>(name) }
            {
                // SAFETY: the ABI's initialiser, called once.
                unsafe { entry() };
                lib.entered = true;
            }
        }

        Ok(lib)
    }

    /// # Safety
    /// The caller must name a symbol whose real signature is `T`.
    unsafe fn symbol<T: Copy>(&self, name: &[u8]) -> Option<T> {
        debug_assert_eq!(name.last(), Some(&0), "symbol name must be NUL-terminated");
        debug_assert_eq!(
            size_of::<T>(),
            size_of::<*mut c_void>(),
            "T must be pointer-sized"
        );
        let sym = unsafe { find_symbol(self.handle, name.as_ptr().cast()) };
        if sym.is_null() {
            return None;
        }
        Some(unsafe { *(&sym as *const *mut c_void).cast::<T>() })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // DELIBERATELY LEAKED. Calling bundleExit and dlclose is the tidy thing
        // and it is not the safe thing: plugins spawn threads and register
        // atexit handlers in bundleEntry, and unmapping the library from under
        // a thread that is still running is a use-after-free with a stack trace
        // pointing into someone else's code. Every mature host either keeps
        // modules loaded for the process lifetime or unloads them only through
        // a carefully sequenced teardown. A recorder loads a piano once and
        // keeps it; leaking a few hundred KB of mapping is the correct trade.
        //
        // Written as an explicit no-op with this comment rather than by omitting
        // the impl, so that nobody "fixes" the missing cleanup later.
        let _ = self.entered;
    }
}

// ── Loading a shared library, per platform ──────────────────────────────────
//
// Split because `dlopen` does not exist on Windows, and `cargo check` will not
// tell you: a check never links, so this compiled cleanly for the Windows
// target for as long as it existed and failed only at `cargo xwin build` with
// three undefined symbols. That is the argument for building the release
// artifact rather than trusting a check, and it is why `build-cross.sh` has
// both.

/// Open the module. The handle is opaque and only [`find_symbol`] reads it.
#[cfg(not(windows))]
fn load(c_path: &CString, _binary: &Path) -> Result<*mut c_void, String> {
    // Imported here rather than at the top of the file: the Windows loader
    // below has no use for it, and an unconditional import is a warning there.
    use std::ffi::CStr;

    // Minimal dlfcn bindings. `libloading` would do this too, but it is a
    // dependency for three symbols and it does not know about CFBundle.
    const RTLD_NOW: i32 = 2;
    #[cfg(target_os = "macos")]
    const RTLD_LOCAL: i32 = 4;
    #[cfg(not(target_os = "macos"))]
    const RTLD_LOCAL: i32 = 0;

    // `c_char`, not `i8`: these are C's own signatures, and C's `char` is
    // signed on x86 and unsigned on aarch64. Declaring them `i8` makes the
    // extern block itself fail to compile on ARM Linux.
    extern "C" {
        #[link_name = "dlopen"]
        fn libc_dlopen(path: *const std::os::raw::c_char, flags: i32) -> *mut c_void;
        #[link_name = "dlerror"]
        fn libc_dlerror() -> *const std::os::raw::c_char;
    }

    // SAFETY: `c_path` is a NUL-terminated path that outlives the call.
    let handle = unsafe { libc_dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
    if handle.is_null() {
        // SAFETY: dlerror is valid immediately after a failed dlopen.
        let err = unsafe {
            let e = libc_dlerror();
            if e.is_null() {
                "unknown error".to_string()
            } else {
                CStr::from_ptr(e).to_string_lossy().into_owned()
            }
        };
        return Err(format!("dlopen failed: {err}"));
    }
    Ok(handle)
}

/// `LoadLibraryW`, and the path has to be UTF-16.
///
/// The `W` form rather than `A`: plugin directories live under the user's
/// profile, and a user whose name is not representable in the system code page
/// cannot load a plugin at all through the ANSI entry point.
#[cfg(windows)]
fn load(_c_path: &CString, binary: &Path) -> Result<*mut c_void, String> {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn LoadLibraryW(path: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    let mut wide: Vec<u16> = binary.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err("path contains a NUL".to_string());
    }
    wide.push(0);
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
    if handle.is_null() {
        // SAFETY: no FFI call happens between the failure and this read.
        let code = unsafe { GetLastError() };
        // 126 is ERROR_MOD_NOT_FOUND, which on a plugin almost always means a
        // DEPENDENCY is missing rather than the plugin itself — the message
        // says so because "module not found" pointing at a file that plainly
        // exists is the most confusing error in Windows plugin hosting.
        let hint = if code == 126 {
            " (the plugin or one of its dependent DLLs could not be found)"
        } else {
            ""
        };
        return Err(format!("LoadLibrary failed: error {code}{hint}"));
    }
    Ok(handle)
}

/// Look up an exported symbol. `null` when it is absent.
///
/// # Safety
/// `handle` must be a live handle from [`load`] and `name` a NUL-terminated
/// symbol name.
#[cfg(not(windows))]
unsafe fn find_symbol(handle: *mut c_void, name: *const i8) -> *mut c_void {
    extern "C" {
        #[link_name = "dlsym"]
        fn libc_dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
    }
    // SAFETY: the caller's contract is this function's.
    unsafe { libc_dlsym(handle, name) }
}

/// # Safety
/// As above.
#[cfg(windows)]
unsafe fn find_symbol(handle: *mut c_void, name: *const i8) -> *mut c_void {
    extern "system" {
        fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    }
    // SAFETY: the caller's contract is this function's.
    unsafe { GetProcAddress(handle, name) }
}

#[cfg(target_os = "macos")]
mod cf {
    use std::ffi::c_void;
    #[repr(C)]
    pub struct __CFString(c_void);
    // The framework must be named explicitly. Rust links libSystem but not
    // CoreFoundation, and the failure is a linker error naming `_CFBundleCreate`
    // rather than anything that points at a missing `#[link]`.
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFAllocatorDefault: *const c_void;
        pub fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            num_bytes: isize,
            encoding: u32,
            is_external: bool,
        ) -> *mut __CFString;
        pub fn CFURLCreateWithFileSystemPath(
            alloc: *const c_void,
            path: *mut __CFString,
            style: isize,
            is_directory: bool,
        ) -> *mut c_void;
        pub fn CFBundleCreate(alloc: *const c_void, url: *mut c_void) -> *mut c_void;
        pub fn CFRelease(cf: *const c_void);
    }
    pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    pub const K_CF_URL_POSIX_PATH_STYLE: isize = 0;
}

/// # Safety
/// `path` must be a real directory on disk.
#[cfg(target_os = "macos")]
unsafe fn cfbundle_for(path: &Path) -> Result<*mut c_void, String> {
    use cf::*;
    let bytes = path.as_os_str().as_encoded_bytes();
    // SAFETY: `bytes` is a valid UTF-8 byte range for the lifetime of the call.
    let s = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            bytes.as_ptr(),
            bytes.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
            false,
        )
    };
    if s.is_null() {
        return Err("could not make a CFString from the bundle path".into());
    }
    // SAFETY: `s` is a live CFString; the URL takes its own reference.
    let url = unsafe {
        CFURLCreateWithFileSystemPath(kCFAllocatorDefault, s, K_CF_URL_POSIX_PATH_STYLE, true)
    };
    // SAFETY: we own `s` and are done with it.
    unsafe { CFRelease(s.cast()) };
    if url.is_null() {
        return Err("could not make a CFURL from the bundle path".into());
    }
    // SAFETY: `url` is live.
    let bundle = unsafe { CFBundleCreate(kCFAllocatorDefault, url) };
    // SAFETY: we own `url` and are done with it.
    unsafe { CFRelease(url) };
    if bundle.is_null() {
        return Err("CFBundleCreate failed".into());
    }
    // Intentionally NOT released: the plugin holds it for its lifetime, and the
    // module is never unloaded (see Library::drop).
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_paths_are_the_standard_ones() {
        let paths = search_paths();
        assert!(!paths.is_empty());
        #[cfg(target_os = "macos")]
        assert!(
            paths.iter().any(|p| p.ends_with("Library/Audio/Plug-Ins/VST3")),
            "the SDK's standard macOS location must be searched: {paths:?}"
        );
    }

    #[test]
    fn user_paths_come_before_system_paths() {
        // A user's own build of a plugin must shadow a system install, which is
        // what every host does and what anyone debugging a plugin expects.
        let paths = search_paths();
        if paths.len() >= 2 {
            let first = paths[0].to_string_lossy().to_string();
            assert!(
                first.contains("Users") || first.contains("HOME") || first.starts_with('/'),
                "unexpected first search path: {first}"
            );
        }
    }

    #[test]
    fn a_missing_bundle_is_an_error_not_a_panic() {
        let err = Module::open(Path::new("/nonexistent/Nope.vst3")).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn a_directory_that_is_not_a_bundle_is_an_error_not_a_panic() {
        let err = Module::open(Path::new("/tmp")).unwrap_err();
        assert!(err.contains("no loadable binary"), "{err}");
    }

    #[test]
    fn c_arrays_stop_at_the_nul_and_survive_a_full_field() {
        assert_eq!(c_array_to_string(&[0x41, 0x42, 0, 0x43]), "AB");
        assert_eq!(c_array_to_string(&[0x41; 4]), "AAAA", "unterminated is fine");
        assert_eq!(c_array_to_string(&[]), "");
        assert_eq!(c_array_to_string(&[0x20, 0x41, 0x20, 0]), "A", "trimmed");
    }

    #[test]
    fn an_audio_module_is_distinguished_from_a_controller() {
        let processor = ClassInfo {
            name: "Pianoteq".into(),
            category: "Audio Module Class".into(),
            sub_categories: "Instrument|Piano".into(),
            cid: [0; 16],
        };
        let controller = ClassInfo {
            name: "Pianoteq Controller".into(),
            category: "Component Controller Class".into(),
            sub_categories: String::new(),
            cid: [0; 16],
        };
        assert!(processor.is_audio_module());
        assert!(!controller.is_audio_module());
    }

    fn class_of(sub: &str) -> ClassInfo {
        ClassInfo {
            name: "Something".into(),
            category: "Audio Module Class".into(),
            sub_categories: sub.into(),
            cid: [0; 16],
        }
    }

    /// **A synth and a reverb are both "Audio Module Class".**
    ///
    /// Which is why loading Pro-R into an instrument slot produced silence and
    /// no explanation: nothing the app could see said the two were different.
    /// `subCategories` says, and this is the reading of it.
    #[test]
    fn a_plugin_says_whether_it_plays_notes_or_changes_them() {
        use Kind::*;
        for (sub, want) in [
            ("Instrument|Synth", Instrument),
            ("Instrument", Instrument),
            ("Fx|Reverb", Effect),
            ("Fx|Delay|Stereo", Effect),
            // **Instrument wins a tie.** Samplers with an audio input declare
            // both, and they genuinely do play notes: refusing one would be a
            // regression wearing a fix's clothes.
            ("Fx|Instrument", Instrument),
            ("Instrument|Fx", Instrument),
            // Case and spacing are a plugin's own business.
            ("fx|reverb", Effect),
            (" Fx | Reverb ", Effect),
            // **Silence is not a no.** A factory too old to be asked tells us
            // nothing, and nothing must not read as "not an instrument" — that
            // would refuse to load working plugins that have always worked.
            ("", Unknown),
            ("Spatial|Ambisonics", Unknown),
        ] {
            assert_eq!(
                class_of(sub).kind(),
                want,
                "{sub:?} was read as the wrong kind"
            );
        }
    }

    /// **The scan finds what is actually installed, not what is at the top.**
    ///
    /// The SDK allows vendor and category folders under the VST3 directory and
    /// every large installer uses them. A one-level scan found 112 of the 160
    /// plugins on the machine this was written on and said nothing about the
    /// other 48 — they simply were not offered, which reads as "this host does
    /// not support my plugin" rather than as a bug.
    ///
    /// Also pins the three things that make a recursive scan safe: it stops at
    /// `MAX_DEPTH`, it does not walk INTO a bundle, and a folder reachable two
    /// ways is listed once.
    #[test]
    fn the_scan_finds_nested_bundles_without_walking_the_whole_disk() {
        let root = std::env::temp_dir().join("tangent-scan-test");
        let _ = std::fs::remove_dir_all(&root);
        let mk = |p: &Path| std::fs::create_dir_all(p).expect("mkdir");

        // One at the top, one under a vendor folder, one under a vendor AND a
        // category folder — the three shapes that exist in the wild.
        mk(&root.join("Top.vst3/Contents"));
        mk(&root.join("Vendor/Nested.vst3/Contents"));
        mk(&root.join("Vendor/Category/Deep.vst3/Contents"));
        // Past the depth limit: present on disk, deliberately not found.
        mk(&root.join("a/b/c/d/TooDeep.vst3/Contents"));
        // A directory that is not a bundle and holds nothing.
        mk(&root.join("Vendor/Presets"));
        // And something inside a bundle that looks exactly like a bundle: the
        // walk must not descend into a `.vst3` at all.
        mk(&root.join("Top.vst3/Contents/Inner.vst3"));

        let found = discover_in(&[root.clone()]);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();

        assert!(names.contains(&"Top.vst3".to_owned()), "{names:?}");
        assert!(names.contains(&"Nested.vst3".to_owned()), "{names:?}");
        assert!(names.contains(&"Deep.vst3".to_owned()), "{names:?}");
        assert!(
            !names.contains(&"Inner.vst3".to_owned()),
            "the walk went inside a bundle: {names:?}"
        );
        assert!(
            !names.contains(&"TooDeep.vst3".to_owned()),
            "the walk went past MAX_DEPTH: {names:?}"
        );

        // The same folder twice is not the same plugin twice. This is the case
        // a symlinked plugin directory produces, and a picker with every
        // instrument listed twice is worse than one missing them.
        let twice = discover_in(&[root.clone(), root.clone()]);
        assert_eq!(twice.len(), found.len(), "duplicates: {twice:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A search path that does not exist is the normal case — most machines
    /// have never had most vendors' installers run — and it must not be an
    /// error, a panic, or a reason to stop looking in the others.
    #[test]
    fn a_missing_folder_is_skipped_rather_than_fatal() {
        let nowhere = std::env::temp_dir().join("tangent-scan-does-not-exist");
        let _ = std::fs::remove_dir_all(&nowhere);
        let found = discover_in(&[nowhere]);
        // Whatever is really installed on the machine running this, plus
        // nothing from the folder that is not there.
        assert_eq!(found, discover());
    }
}
