//! Secret storage is separate from portable agent state and transport.
use anyhow::{Context, Result, ensure};
use std::{fs, io::Write, path::PathBuf};
use uuid::Uuid;

pub trait CredentialStore: Send + Sync {
    fn get(&self, id: &str) -> Result<String>;
    fn set(&self, id: &str, secret: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

/// Headless/Linux fallback. The desktop uses native stores on macOS and Windows.
pub struct FileCredentials(pub PathBuf);
impl FileCredentials {
    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(Uuid::parse_str(id).is_ok(), "Invalid credential identifier");
        Ok(self.0.join(id))
    }
}
impl CredentialStore for FileCredentials {
    fn get(&self, id: &str) -> Result<String> {
        fs::read_to_string(self.path(id)?).context("Credential is unavailable; pair again")
    }
    fn set(&self, id: &str, secret: &str) -> Result<()> {
        let path = self.path(id)?;
        fs::create_dir_all(&self.0)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700))?;
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.0)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(secret.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.path(id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
