use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Exposure {
    pub cpu: bool,
    pub memory: bool,
    pub shutdown: bool,
    pub audio: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub device_id: Uuid,
    pub exposure: Exposure,
    pub sample_interval_seconds: u64,
    #[serde(default)]
    pub enrollment: Option<Enrollment>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub url: String,
    pub credential_id: String,
    pub instance_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_id: Uuid::new_v4(),
            exposure: Exposure::default(),
            sample_interval_seconds: 5,
            enrollment: None,
        }
    }
}

impl Config {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let value: Self = serde_json::from_slice(&fs::read(path)?)
                .context("Invalid agent config; refusing to overwrite it")?;
            ensure!(
                (1..=3600).contains(&value.sample_interval_seconds),
                "Sample interval must be 1..3600 seconds"
            );
            Ok(value)
        } else {
            let value = Self::default();
            value.save(path)?;
            Ok(value)
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let parent = path.parent().context("Config needs a parent directory")?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.as_file().sync_all()?;
        file.persist(path)
            .context("Could not atomically save settings")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_private_and_identity_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let first = Config::load_or_create(&path).unwrap();
        assert_eq!(first.exposure, Exposure::default());
        let second = Config::load_or_create(&path).unwrap();
        assert_eq!(first.device_id, second.device_id);
        assert!(second.enrollment.is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn corrupt_config_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "broken").unwrap();
        assert!(Config::load_or_create(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
}
