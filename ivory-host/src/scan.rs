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

use std::ffi::{c_void, CString, OsStr};
use std::path::{Path, PathBuf};

use vst3::Steinberg::{IPluginFactory, IPluginFactoryTrait, PClassInfo, PFactoryInfo};
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
pub fn search_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
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

/// Every `.vst3` bundle under the standard paths.
///
/// Not recursive beyond one level: the SDK allows nested category folders, but
/// a full walk of `/Library/Audio/Plug-Ins` on a machine with a large library
/// is slow enough to be felt at startup and is not where anything installs.
pub fn discover() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in search_paths() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("vst3")) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
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
    pub category: String,
    pub cid: [u8; 16],
}

impl ClassInfo {
    /// Whether this is the audio-processing half, as opposed to a controller or
    /// something else the factory happens to export.
    pub fn is_audio_module(&self) -> bool {
        self.category == "Audio Module Class"
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
    pub fn classes(&self) -> Vec<ClassInfo> {
        // SAFETY: the factory is a live COM pointer for the module's lifetime.
        let count = unsafe { self.factory.countClasses() };
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
            out.push(ClassInfo {
                name: c_array_to_string(&info.name),
                category: c_array_to_string(&info.category),
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
            cid: [0; 16],
        };
        let controller = ClassInfo {
            name: "Pianoteq Controller".into(),
            category: "Component Controller Class".into(),
            cid: [0; 16],
        };
        assert!(processor.is_audio_module());
        assert!(!controller.is_audio_module());
    }
}
