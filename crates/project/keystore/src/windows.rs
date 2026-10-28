// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Windows Credential Manager: each key is a generic credential named
//! `service/account`, kept for this person on this computer
//! (`CRED_PERSIST_LOCAL_MACHINE`, not roamed to other computers). Control
//! Panel ▸ Credential Manager ▸ Windows Credentials lists them.

use crate::Fail;
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW};

/// A NUL-terminated wide string.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn target(service: &str, account: &str) -> Vec<u16> {
    wide(&format!("{service}/{account}"))
}

pub fn get(service: &str, account: &str) -> Result<Option<String>, Fail> {
    let name = target(service, account);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; on success
    // `cred` points at a credential Windows allocated, freed below.
    let ok = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
    if ok == 0 {
        // SAFETY: reads this thread's last error, set by CredReadW.
        let err = unsafe { GetLastError() };
        return if err == ERROR_NOT_FOUND { Ok(None) } else { Err(Fail::Unavailable(format!("Windows Credential Manager could not be read (error {err})"))) };
    }
    // SAFETY: `cred` is a valid credential; its blob is `CredentialBlobSize`
    // bytes long (or null when empty). Copied out before it is freed.
    let bytes = unsafe {
        let c = &*cred;
        let b = if c.CredentialBlob.is_null() { Vec::new() } else { std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec() };
        CredFree(cred as *const core::ffi::c_void);
        b
    };
    Ok(String::from_utf8(bytes).ok())
}

pub fn set(service: &str, account: &str, secret: &str) -> Result<(), Fail> {
    let bytes = secret.as_bytes();
    if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(Fail::Unavailable(format!("the key is longer than Windows Credential Manager keeps ({CRED_MAX_CREDENTIAL_BLOB_SIZE} bytes)")));
    }
    let mut name = target(service, account);
    let mut user = wide(account);
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        CredentialBlobSize: bytes.len() as u32,
        CredentialBlob: bytes.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` points into a buffer that outlives
    // the call; Windows copies what it keeps.
    let ok = unsafe { CredWriteW(&cred, 0) };
    if ok == 0 {
        // SAFETY: reads this thread's last error, set by CredWriteW.
        let err = unsafe { GetLastError() };
        return Err(Fail::Unavailable(format!("Windows Credential Manager did not save the key (error {err})")));
    }
    match get(service, account) {
        Ok(Some(back)) if back == secret => Ok(()),
        _ => Err(Fail::Unavailable("Windows Credential Manager did not keep the key".into())),
    }
}

pub fn remove(service: &str, account: &str) {
    let name = target(service, account);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe {
        CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0);
    }
}
