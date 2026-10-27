// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Files and connections only this user can reach: what the agent socket,
//! its Windows token file, the traces and the hot-swap copies are made with.
//!
//! * Files are created `0600` (folders `0700`) on Unix, and on Windows with
//!   a protected DACL that gives the owner alone full access.
//! * A Unix socket connection is accepted only from a process of this same
//!   user (`SO_PEERCRED` on Linux, `getpeereid` elsewhere), on top of the
//!   socket file's `0600`.

use std::fs::File;
use std::io;
use std::path::Path;

/// A folder only this user can list or enter (made if it is not there).
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        // An existing folder of ours keeps what it has, except group and others.
        let mode = std::fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode & 0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// A new file only this user can read or write; fails if it is there.
pub fn create_new(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
    }
    #[cfg(windows)]
    {
        windows::create(path, windows::CREATE_NEW, windows::GENERIC_WRITE)
    }
}

/// A file to append to, made user-only if it is new.
pub fn append(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(path)?;
        // A file someone made wider than that is narrowed before it is written.
        if f.metadata()?.permissions().mode() & 0o077 != 0 {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(f)
    }
    #[cfg(windows)]
    {
        match windows::create(path, windows::CREATE_NEW, windows::FILE_APPEND_DATA) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => std::fs::OpenOptions::new().append(true).open(path),
            r => r,
        }
    }
}

/// Whether the process at the other end of `stream` is this user's.
#[cfg(unix)]
pub fn same_user(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let me = unsafe { libc::geteuid() };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut libc::ucred as *mut libc::c_void, &mut len) };
        r == 0 && cred.uid == me
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
        let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        r == 0 && uid == me
    }
}

#[cfg(windows)]
mod windows {
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;

    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    pub const FILE_APPEND_DATA: u32 = 0x0004;
    pub const CREATE_NEW: u32 = 1;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const SDDL_REVISION_1: u32 = 1;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Protected DACL, one entry: the owner, full access (no inheritance
    /// from the folder, so nobody else is let in by it).
    pub fn create(path: &Path, disposition: u32, access: u32) -> io::Result<File> {
        let sddl = wide(std::ffi::OsStr::new("D:P(A;;FA;;;OW)"));
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd as *mut _ as *mut _, std::ptr::null_mut()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let name = wide(path.as_os_str());
        let h = unsafe { CreateFileW(name.as_ptr(), access, FILE_SHARE_READ, &sa, disposition, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut()) };
        let err = io::Error::last_os_error();
        unsafe { LocalFree(sd as _) };
        if h == INVALID_HANDLE_VALUE {
            return Err(err);
        }
        Ok(unsafe { File::from_raw_handle(h as _) })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn files_are_this_users_alone() {
        let dir = std::env::temp_dir().join(format!("zornov-local-only-{}", std::process::id()));
        private_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let p = dir.join("a.jsonl");
        drop(append(&p).unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(create_new(&p).is_err(), "never over a file that is there");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_connection_from_this_user_is_this_users() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(same_user(&a));
    }
}
