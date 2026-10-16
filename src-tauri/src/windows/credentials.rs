//! API keys in Windows Credential Manager, where macOS keeps them in the
//! Keychain. Each key is a generic credential named after the Keychain
//! service it would have on a Mac, under the account "samlu".

use super::wide;
use std::ptr::null_mut;
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

const ACCOUNT: &str = "samlu";

pub fn read_secret(service: &str) -> Option<String> {
    let target = wide(service);
    let mut credential: *mut CREDENTIALW = null_mut();
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        return None;
    }
    let secret = unsafe {
        let stored = &*credential;
        let bytes = if stored.CredentialBlob.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(stored.CredentialBlob, stored.CredentialBlobSize as usize)
        };
        String::from_utf8_lossy(bytes).trim().to_string()
    };
    unsafe { CredFree(credential.cast()) };
    Some(secret)
}

pub fn write_secret(service: &str, secret: &str) -> Result<(), String> {
    let mut target = wide(service);
    let mut account = wide(ACCOUNT);
    let mut blob = secret.as_bytes().to_vec();
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target.as_mut_ptr();
    credential.UserName = account.as_mut_ptr();
    credential.CredentialBlobSize = blob.len() as u32;
    credential.CredentialBlob = blob.as_mut_ptr();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        return Err(format!(
            "Windows Credential Manager refused the key: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn delete_secret(service: &str) {
    let target = wide(service);
    unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_round_trip() {
        let service = format!("com.samlu.desktop.test.{}", std::process::id());
        assert_eq!(read_secret(&service), None);
        write_secret(&service, "sk-test-123").unwrap();
        assert_eq!(read_secret(&service).as_deref(), Some("sk-test-123"));
        write_secret(&service, "sk-test-456").unwrap();
        assert_eq!(read_secret(&service).as_deref(), Some("sk-test-456"));
        delete_secret(&service);
        assert_eq!(read_secret(&service), None);
    }
}
