// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hot-swapped tools (`agent-dev` builds only): `crates/dev/agent-hot`,
//! rebuilt on its own in seconds and loaded into the running window by
//! `zornov dev swap`, so a fix reaches the agent inside without a restart.
//!
//! What may be loaded is fixed when this build is made, never taken from a
//! request or a trace: the library at `target/debug/` of the checkout this
//! binary was built from, a regular file of this user's that nobody else
//! can write, whose SHA-256 is the one the swap request names (the hash of
//! what `zornov dev swap` just built). The bytes that were hashed are
//! written to a private copy (`0600` in a `0700` folder) and that copy is
//! loaded, so the file cannot change between the check and the load.
//! Nothing is fetched from anywhere.
//!
//! A loaded library can only take over tools the app already lists and
//! reword arguments before the app's own tool sees them; it never gets the
//! project. Libraries are never unloaded (a call in flight keeps working),
//! the newest one answers.
//!
//! Without `agent-dev` this file is two functions that do nothing, and
//! `Session::run` pays nothing for them.

use super::Content;
use serde_json::Value;
use std::borrow::Cow;

#[cfg(not(feature = "agent-dev"))]
#[inline(always)]
pub fn call(_tool: &str, _args: &Value) -> Option<Result<Vec<Content>, String>> {
    None
}

#[cfg(not(feature = "agent-dev"))]
#[inline(always)]
pub fn normalise<'a>(_tool: &str, args: &'a Value) -> Cow<'a, Value> {
    Cow::Borrowed(args)
}

#[cfg(feature = "agent-dev")]
pub use dev::*;

#[cfg(feature = "agent-dev")]
mod dev {
    use super::*;
    use serde_json::json;
    use std::ffi::{CStr, CString, c_char};
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    type AbiFn = unsafe extern "C" fn() -> u32;
    type ToolsFn = unsafe extern "C" fn() -> *mut c_char;
    type CallFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
    type FreeFn = unsafe extern "C" fn(*mut c_char);

    /// The ABI `crates/dev/agent-hot` speaks.
    const ABI: u32 = 1;
    /// Larger than this is not a small tools library.
    const MAX_BYTES: u64 = 64 << 20;

    struct Lib {
        sha256: String,
        tools: Vec<String>,
        call: CallFn,
        normalise: Option<CallFn>,
        free: FreeFn,
        loaded: std::time::SystemTime,
    }

    static CURRENT: RwLock<Option<Arc<Lib>>> = RwLock::new(None);

    fn current() -> Option<Arc<Lib>> {
        CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The checkout this binary was built from.
    pub fn repo_root() -> Option<PathBuf> {
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()?).ok()
    }

    fn file_name() -> &'static str {
        if cfg!(windows) {
            "zornov_agent_hot.dll"
        } else if cfg!(target_os = "macos") {
            "libzornov_agent_hot.dylib"
        } else {
            "libzornov_agent_hot.so"
        }
    }

    /// The one library this build will load.
    pub fn library_path() -> Option<PathBuf> {
        Some(repo_root()?.join("target").join("debug").join(file_name()))
    }

    /// Whether the loaded library answers `tool`.
    pub fn serves(tool: &str) -> bool {
        current().is_some_and(|l| l.tools.iter().any(|t| t == tool))
    }

    /// What is loaded.
    pub fn status() -> Value {
        match current() {
            Some(l) => json!({"loaded": true, "sha256": l.sha256, "tools": l.tools, "normalises": l.normalise.is_some(), "since": l.loaded.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())}),
            None => json!({"loaded": false, "library": library_path()}),
        }
    }

    /// Checks and loads the library whose SHA-256 is `want` (lower-case hex).
    pub fn load(want: &str) -> Result<Value, String> {
        let want = want.trim().to_ascii_lowercase();
        if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("swap needs the library's SHA-256 (64 hex digits)".into());
        }
        let root = repo_root().ok_or("this build does not know its checkout")?;
        let path = library_path().ok_or("no library path")?;
        let real = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e} (run cargo build -p zornov-agent-hot)", path.display()))?;
        // No symbolic link out of the checkout.
        if !real.starts_with(&root) || real.file_name() != path.file_name() {
            return Err(format!("{} is not inside {}", real.display(), root.display()));
        }
        let meta = std::fs::symlink_metadata(&real).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return Err(format!("{} is not a library file", real.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if meta.uid() != unsafe { libc::geteuid() } || meta.permissions().mode() & 0o022 != 0 {
                return Err(format!("{} must be yours and writable by you alone", real.display()));
            }
        }
        let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
        let got = studio_update::feed::sha256_hex(&bytes);
        if got != want {
            return Err(format!("the library's SHA-256 is {got}, not {want}: it changed since it was built, so it is not loaded"));
        }
        // The hashed bytes, in a private copy, are what gets loaded.
        let dir = studio_paths::data_dir().join("agent-dev").join("hot");
        super::super::local_only::private_dir(&dir).map_err(|e| e.to_string())?;
        let copy = dir.join(format!("{}-{}", &got[..16], file_name()));
        if !copy.exists() {
            use std::io::Write;
            super::super::local_only::create_new(&copy).and_then(|mut f| f.write_all(&bytes)).map_err(|e| format!("{}: {e}", copy.display()))?;
        } else if studio_update::feed::sha256_file(&copy).map_err(|e| e.to_string())? != got {
            return Err(format!("{} is not the copy it should be; remove it and swap again", copy.display()));
        }
        let lib = unsafe { open(&copy)? };
        let tools = {
            let p = unsafe { (lib.tools)() };
            let v = take(p, lib.free).unwrap_or_default();
            let known: Vec<String> = super::super::mcp::tools().iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect();
            let list: Vec<String> = v.as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)).collect();
            if let Some(bad) = list.iter().find(|t| !known.contains(t)) {
                return Err(format!("the library takes over \"{bad}\", which is not a tool of this app; it may only replace tools the app has"));
            }
            list
        };
        let l = Lib { sha256: got.clone(), tools: tools.clone(), call: lib.call, normalise: lib.normalise, free: lib.free, loaded: std::time::SystemTime::now() };
        *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(l));
        Ok(json!({"loaded": true, "sha256": got, "tools": tools, "normalises": lib.normalise.is_some()}))
    }

    struct Symbols {
        tools: ToolsFn,
        call: CallFn,
        normalise: Option<CallFn>,
        free: FreeFn,
    }

    /// # Safety
    /// Runs the library's initialisers: only for the checked copy.
    #[cfg(unix)]
    unsafe fn open(path: &std::path::Path) -> Result<Symbols, String> {
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            let why = unsafe { libc::dlerror() };
            return Err(if why.is_null() { "the library did not load".into() } else { unsafe { CStr::from_ptr(why) }.to_string_lossy().into_owned() });
        }
        let sym = |name: &str| -> Option<*mut libc::c_void> {
            let n = CString::new(name).ok()?;
            let p = unsafe { libc::dlsym(h, n.as_ptr()) };
            (!p.is_null()).then_some(p)
        };
        let need = |name: &str| sym(name).ok_or_else(|| format!("the library has no {name}"));
        unsafe {
            let abi: AbiFn = std::mem::transmute(need("zornov_hot_abi")?);
            if abi() != ABI {
                return Err(format!("the library speaks ABI {}, this build {ABI}", abi()));
            }
            Ok(Symbols {
                tools: std::mem::transmute(need("zornov_hot_tools")?),
                call: std::mem::transmute(need("zornov_hot_call")?),
                normalise: sym("zornov_hot_normalise").map(|p| std::mem::transmute::<*mut libc::c_void, CallFn>(p)),
                free: std::mem::transmute(need("zornov_hot_free")?),
            })
        }
    }

    /// # Safety
    /// As the Unix one.
    #[cfg(windows)]
    unsafe fn open(path: &std::path::Path) -> Result<Symbols, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let w: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let h = unsafe { LoadLibraryW(w.as_ptr()) };
        if h.is_null() {
            return Err(format!("the library did not load: {}", std::io::Error::last_os_error()));
        }
        let sym = |name: &str| {
            let n = CString::new(name).ok()?;
            unsafe { GetProcAddress(h, n.as_ptr() as *const u8) }
        };
        let need = |name: &str| sym(name).ok_or_else(|| format!("the library has no {name}"));
        unsafe {
            let abi: AbiFn = std::mem::transmute(need("zornov_hot_abi")?);
            if abi() != ABI {
                return Err(format!("the library speaks ABI {}, this build {ABI}", abi()));
            }
            Ok(Symbols {
                tools: std::mem::transmute(need("zornov_hot_tools")?),
                call: std::mem::transmute(need("zornov_hot_call")?),
                normalise: sym("zornov_hot_normalise").map(|p| std::mem::transmute::<_, CallFn>(p)),
                free: std::mem::transmute(need("zornov_hot_free")?),
            })
        }
    }

    /// A string the library returned, as JSON, freed.
    fn take(p: *mut c_char, free: FreeFn) -> Option<Value> {
        if p.is_null() {
            return None;
        }
        let v = serde_json::from_str(&unsafe { CStr::from_ptr(p) }.to_string_lossy()).ok();
        unsafe { free(p) };
        v
    }

    /// The loaded library's answer to `tool`, when it takes that tool over.
    pub fn call(tool: &str, args: &Value) -> Option<Result<Vec<Content>, String>> {
        let l = current().filter(|l| l.tools.iter().any(|t| t == tool))?;
        let (t, a) = (CString::new(tool).ok()?, CString::new(args.to_string()).ok()?);
        let v = take(unsafe { (l.call)(t.as_ptr(), a.as_ptr()) }, l.free)?;
        if v["handled"] != json!(true) {
            return None;
        }
        Some(if v["ok"] == json!(true) { Ok(vec![Content::Text(v["text"].as_str().unwrap_or("{}").to_owned())]) } else { Err(v["error"].as_str().unwrap_or("the hot-swapped tool failed").to_owned()) })
    }

    /// The arguments as the loaded library rewords them (as they are without one).
    pub fn normalise<'a>(tool: &str, args: &'a Value) -> Cow<'a, Value> {
        let Some(l) = current() else { return Cow::Borrowed(args) };
        let Some(f) = l.normalise else { return Cow::Borrowed(args) };
        let (Ok(t), Ok(a)) = (CString::new(tool), CString::new(args.to_string())) else { return Cow::Borrowed(args) };
        match take(unsafe { f(t.as_ptr(), a.as_ptr()) }, l.free) {
            Some(v) if v.is_object() => Cow::Owned(v),
            _ => Cow::Borrowed(args),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_the_hash_that_was_built_is_loaded() {
            assert!(load("not a hash").is_err());
            // A hash of nothing in particular never matches a real build.
            let e = load(&"0".repeat(64)).unwrap_err();
            assert!(e.contains("SHA-256") || e.contains("cargo build"), "{e}");
            assert!(!serves("feedback"));
        }
    }
}
