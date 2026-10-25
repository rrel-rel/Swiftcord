//! The account token, kept in Windows Credential Manager (encrypted for the current user by Windows).

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

const TARGET: &str = "Swiftcord/token";

pub fn load_token() -> Option<String> {
    let target = HSTRING::from(TARGET);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(&target, CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let token = String::from_utf8(blob.to_vec()).ok();
        CredFree(cred as *const _);
        token.filter(|t| !t.is_empty())
    }
}

pub fn save_token(token: &str) -> windows::core::Result<()> {
    let mut target: Vec<u16> = TARGET.encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = "Swiftcord".encode_utf16().chain([0]).collect();
    let mut blob = token.as_bytes().to_vec();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }
}

pub fn delete_token() {
    let target = HSTRING::from(TARGET);
    unsafe {
        let _ = CredDeleteW(&target, CRED_TYPE_GENERIC, None);
    }
}
