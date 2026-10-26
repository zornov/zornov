// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Native crashes: a signal handler on Unix, an exception filter on Windows.

use std::path::Path;

#[cfg(unix)]
mod imp {
    use super::Path;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Built before any crash; the handler only reads them and calls
    // async-signal-safe functions (open, write, backtrace, raise).
    static mut PATH: Option<CString> = None;
    static mut HEADER: Vec<u8> = Vec::new();
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    const SIGNALS: [libc::c_int; 6] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT, libc::SIGTRAP];

    fn name(sig: libc::c_int) -> &'static [u8] {
        match sig {
            libc::SIGSEGV => b"SIGSEGV (invalid memory access)",
            libc::SIGBUS => b"SIGBUS (bad memory access)",
            libc::SIGILL => b"SIGILL (illegal instruction)",
            libc::SIGFPE => b"SIGFPE (arithmetic error)",
            libc::SIGABRT => b"SIGABRT (abort)",
            libc::SIGTRAP => b"SIGTRAP (trap)",
            _ => b"signal",
        }
    }

    unsafe fn put(fd: libc::c_int, bytes: &[u8]) {
        unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    }

    fn hex(mut v: usize, out: &mut [u8; 18]) -> &[u8] {
        out[0] = b'0';
        out[1] = b'x';
        let mut i = 18;
        loop {
            i -= 1;
            out[i] = b"0123456789abcdef"[v & 15];
            v >>= 4;
            if v == 0 || i == 2 {
                break;
            }
        }
        // Digits sit at the end; move them after "0x".
        let n = 18 - i;
        out.copy_within(i..18, 2);
        &out[..2 + n]
    }

    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        unsafe {
            // A crash inside the handler goes straight to the default action.
            if ACTIVE.swap(true, Ordering::SeqCst) {
                libc::signal(sig, libc::SIG_DFL);
                return;
            }
            #[allow(static_mut_refs)]
            if let Some(path) = PATH.as_ref() {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600 as libc::c_uint);
                if fd >= 0 {
                    put(fd, &HEADER);
                    put(fd, b"---\nsignal: ");
                    put(fd, name(sig));
                    put(fd, b"\nfault_address: ");
                    let mut buf = [0u8; 18];
                    let addr = if info.is_null() { 0 } else { (*info).si_addr() as usize };
                    put(fd, hex(addr, &mut buf));
                    put(fd, b"\n\nbacktrace (addresses; symbolicate with the matching build):\n");
                    let mut frames = [std::ptr::null_mut::<libc::c_void>(); 64];
                    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
                    {
                        let n = libc::backtrace(frames.as_mut_ptr(), 64);
                        libc::backtrace_symbols_fd(frames.as_ptr(), n, fd);
                    }
                    let _ = &mut frames;
                    libc::close(fd);
                }
            }
            // Default action: the system's own crash report and exit status.
            libc::signal(sig, libc::SIG_DFL);
            if sig == libc::SIGABRT {
                libc::raise(sig);
            }
        }
    }

    pub fn install(dir: &Path, header: &str) {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let file = dir.join(format!("crash-native-{}.txt", std::process::id()));
        let Ok(path) = CString::new(file.as_os_str().as_encoded_bytes()) else { return };
        unsafe {
            PATH = Some(path);
            HEADER = header.as_bytes().to_vec();
            for sig in SIGNALS {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = handler as *const () as usize;
                // On the thread's alternate stack (std sets one up), so a
                // stack overflow can still be reported.
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    }

    /// Crashes on purpose, for tests: `kind` is `segv` or `abort`.
    pub fn crash_for_test(kind: &str) -> ! {
        unsafe {
            match kind {
                "abort" => libc::abort(),
                _ => std::ptr::write_volatile(std::ptr::null_mut::<u8>().wrapping_add(8), 1),
            }
        }
        std::process::abort()
    }
}

#[cfg(windows)]
mod imp {
    use super::Path;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};

    static HEADER: OnceLock<String> = OnceLock::new();

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        let (code, addr) = unsafe {
            let rec = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref());
            (rec.map_or(0, |r| r.ExceptionCode), rec.map_or(0, |r| r.ExceptionAddress as usize))
        };
        let bt = std::backtrace::Backtrace::force_capture();
        let body = format!("{}---\nsignal: exception {code:#010x}\nfault_address: {addr:#x}\n\nbacktrace:\n{bt}\n", HEADER.get().map_or("", String::as_str));
        let _ = std::fs::create_dir_all(crate::dir());
        let _ = std::fs::write(crate::dir().join(format!("crash-native-{}.txt", std::process::id())), crate::scrub(&body));
        0 // EXCEPTION_CONTINUE_SEARCH: Windows then ends the program as usual
    }

    pub fn install(_dir: &Path, header: &str) {
        let _ = HEADER.set(header.to_owned());
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }

    pub fn crash_for_test(_kind: &str) -> ! {
        unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u8>().wrapping_add(8), 1) };
        std::process::abort()
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::Path;
    pub fn install(_: &Path, _: &str) {}
    pub fn crash_for_test(_: &str) -> ! {
        std::process::abort()
    }
}

pub use imp::install;

/// Crashes the process on purpose (`segv` or `abort`), to test the handler.
pub fn crash_for_test(kind: &str) -> ! {
    imp::crash_for_test(kind)
}
