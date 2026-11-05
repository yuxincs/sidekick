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
impl CredentialStore for NativeStore {
    fn get(&self, id: &str) -> Result<String> {
        use windows_sys::Win32::Security::Credentials::*;
        let target: Vec<u16> = format!("Sidekick/{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut value = std::ptr::null_mut();
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut value) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let bytes = std::slice::from_raw_parts(
                (*value).CredentialBlob,
                (*value).CredentialBlobSize as usize,
            )
            .to_vec();
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
        let mut value: CREDENTIALW = unsafe { std::mem::zeroed() };
        value.Type = CRED_TYPE_GENERIC;
        value.TargetName = target.as_mut_ptr();
        value.CredentialBlobSize = bytes.len() as u32;
        value.CredentialBlob = bytes.as_mut_ptr();
        value.Persist = CRED_PERSIST_LOCAL_MACHINE;
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
        unsafe {
            if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
}
