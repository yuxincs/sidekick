#[cfg(not(target_os = "linux"))]
use anyhow::Result;
use host_agent_core::credentials::CredentialStore;

#[cfg(target_os = "linux")]
pub fn store(directory: std::path::PathBuf) -> std::sync::Arc<dyn CredentialStore> {
    std::sync::Arc::new(host_agent_core::credentials::FileCredentials(
        directory.join("credentials"),
    ))
}
#[cfg(not(target_os = "linux"))]
pub fn store(_directory: std::path::PathBuf) -> std::sync::Arc<dyn CredentialStore> {
    std::sync::Arc::new(NativeStore)
}

#[cfg(not(target_os = "linux"))]
struct NativeStore;
#[cfg(target_os = "macos")]
impl CredentialStore for NativeStore {
    fn get(&self, id: &str) -> Result<String> {
        Ok(String::from_utf8(
            security_framework::passwords::get_generic_password("dev.HostAgent.Sidekick", id)?,
        )?)
    }
    fn set(&self, id: &str, secret: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(
            "dev.HostAgent.Sidekick",
            id,
            secret.as_bytes(),
        )?;
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        security_framework::passwords::delete_generic_password("dev.HostAgent.Sidekick", id)?;
        Ok(())
    }
}
#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "Windows Credential Manager FFI")]
impl CredentialStore for NativeStore {
    fn get(&self, id: &str) -> Result<String> {
        use windows_sys::Win32::Security::Credentials::*;
        let target: Vec<u16> = format!("Sidekick/{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut value = std::ptr::null_mut();
        // SAFETY: target is null-terminated and value is a writable output pointer.
        // On success Windows returns an initialized CREDENTIALW allocation and a
        // readable blob of the stated length. Copy it before CredFree, exactly once.
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut value) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // An empty credential may use a null blob pointer, which must not be
            // passed to from_raw_parts even with a zero length.
            let bytes = if (*value).CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(
                    (*value).CredentialBlob,
                    (*value).CredentialBlobSize as usize,
                )
                .to_vec()
            };
            CredFree(value.cast());
            Ok(String::from_utf8(bytes)?)
        }
    }
    fn set(&self, id: &str, secret: &str) -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        let mut target: Vec<u16> = format!("Sidekick/{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut bytes = secret.as_bytes().to_vec();
        // SAFETY: CREDENTIALW contains only integers, raw pointers, and FILETIME;
        // all have valid all-zero representations. Required fields are set below.
        let mut value: CREDENTIALW = unsafe { std::mem::zeroed() };
        value.Type = CRED_TYPE_GENERIC;
        value.TargetName = target.as_mut_ptr();
        value.CredentialBlobSize = u32::try_from(bytes.len())?;
        value.CredentialBlob = bytes.as_mut_ptr();
        value.Persist = CRED_PERSIST_LOCAL_MACHINE;
        // SAFETY: value is initialized; target is null-terminated, and its mutable
        // blob buffer has the checked length. Both remain alive through CredWriteW,
        // which copies their data rather than retaining these pointers.
        unsafe {
            if CredWriteW(&value, 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        let target: Vec<u16> = format!("Sidekick/{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: target is a live, null-terminated UTF-16 buffer; the type and
        // reserved flags are valid. CredDeleteW does not retain the pointer.
        unsafe {
            if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
}
