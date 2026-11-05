//! Per-user login registration. Never registers automatically or elevates privileges.
use anyhow::{Context, Result, ensure};
#[cfg(any(test, not(target_os = "windows")))]
use std::path::Path;
#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

const ID: &str = "dev.HostAgent.HostAgent";

pub trait StartupControl {
    fn is_enabled(&self) -> Result<bool>;
    fn set_enabled(&self, enabled: bool) -> Result<()>;
}

#[cfg(not(target_os = "windows"))]
pub struct NativeStartup {
    path: PathBuf,
    content: String,
}

#[cfg(target_os = "windows")]
pub struct NativeStartup {
    command: String,
}

impl NativeStartup {
    pub fn new() -> Result<Self> {
        let executable = std::env::current_exe()?.canonicalize()?;
        let path = executable
            .to_str()
            .context("Startup requires a UTF-8 executable path")?;
        ensure!(
            !path.chars().any(char::is_control),
            "Startup path contains control characters"
        );
        #[cfg(target_os = "macos")]
        {
            ensure!(
                !path.contains("/AppTranslocation/") && !path.starts_with("/Volumes/"),
                "Move Sidekick to Applications before enabling Start at login"
            );
            let dirs = directories::BaseDirs::new().context("No user directory")?;
            Ok(Self {
                path: dirs
                    .home_dir()
                    .join("Library/LaunchAgents")
                    .join(format!("{ID}.plist")),
                content: launch_agent(path),
            })
        }
        #[cfg(target_os = "linux")]
        {
            let dirs = directories::BaseDirs::new().context("No user config directory")?;
            Ok(Self {
                path: dirs
                    .config_dir()
                    .join("autostart")
                    .join(format!("{ID}.desktop")),
                content: desktop_entry(path),
            })
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                command: windows_command(path),
            })
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl StartupControl for NativeStartup {
    fn is_enabled(&self) -> Result<bool> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => Ok(content == self.content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    fn set_enabled(&self, enabled: bool) -> Result<()> {
        if enabled {
            atomic_write(&self.path, self.content.as_bytes())
        } else {
            match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl StartupControl for NativeStartup {
    fn is_enabled(&self) -> Result<bool> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        {
            Ok(key) => match key.get_value::<String, _>(ID) {
                Ok(value) => Ok(value == self.command),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error.into()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    fn set_enabled(&self, enabled: bool) -> Result<()> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
        if enabled {
            key.set_value(ID, &self.command)?;
        } else if let Err(error) = key.delete_value(ID) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        Ok(())
    }
}

#[cfg(any(test, not(target_os = "windows")))]
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("Startup path needs a parent")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(any(test, target_os = "windows"))]
fn windows_command(path: &str) -> String {
    format!("\"{path}\" --autostart")
}

#[cfg(any(test, target_os = "macos"))]
fn launch_agent(path: &str) -> String {
    let escaped = path
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{ID}</string>
<key>ProgramArguments</key><array><string>{escaped}</string><string>--autostart</string></array>
<key>RunAtLoad</key><true/>
<key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>
"#
    )
}

#[cfg(any(test, target_os = "linux"))]
fn desktop_entry(path: &str) -> String {
    // Exec quoting is parsed after .desktop string unescaping (two escaping levels).
    let quoted = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%")
        .replace('\\', "\\\\");
    format!(
        "[Desktop Entry]\nType=Application\nName=Sidekick\nExec=\"{quoted}\" --autostart\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_registration_is_valid_xml_and_preserves_special_paths() {
        let path = "/Applications/A & B <test>/Sidekick.app/Contents/MacOS/host-agent";
        let value = plist::Value::from_reader(std::io::Cursor::new(launch_agent(path))).unwrap();
        let dictionary = value.as_dictionary().unwrap();
        let arguments = dictionary["ProgramArguments"].as_array().unwrap();
        assert_eq!(arguments[0].as_string(), Some(path));
        assert_eq!(arguments[1].as_string(), Some("--autostart"));
        assert_eq!(dictionary["RunAtLoad"].as_boolean(), Some(true));
        assert!(!dictionary.contains_key("KeepAlive")); // Quit must stay quit for this login session.
    }
    #[test]
    fn windows_paths_are_quoted_and_have_only_the_startup_flag() {
        assert_eq!(
            windows_command(r"C:\Users\A B\Sidekick\host-agent.exe"),
            r#""C:\Users\A B\Sidekick\host-agent.exe" --autostart"#
        );
    }
    #[test]
    fn linux_exec_uses_both_desktop_escaping_levels() {
        let entry = desktop_entry("/home/a b/$cash%/back`tick/host-agent");
        let exec = entry
            .lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .unwrap();
        // First pass: .desktop string escaping. Second pass: quoted argument escaping.
        let desktop_unescaped = exec.replace("\\\\", "\\");
        let argument = desktop_unescaped
            .strip_prefix('"')
            .unwrap()
            .strip_suffix("\" --autostart")
            .unwrap();
        let decoded = argument
            .replace("\\$", "$")
            .replace("\\`", "`")
            .replace("%%", "%");
        assert_eq!(decoded, "/home/a b/$cash%/back`tick/host-agent");
    }
    #[test]
    fn atomic_registration_replaces_old_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("autostart/agent.desktop");
        atomic_write(&file, b"old path").unwrap();
        atomic_write(&file, b"new path").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new path");
    }
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn enabling_and_disabling_use_only_a_temporary_registration() {
        let dir = tempfile::tempdir().unwrap();
        let registration = NativeStartup {
            path: dir.path().join("login/agent"),
            content: "test".into(),
        };
        assert!(!registration.is_enabled().unwrap());
        registration.set_enabled(true).unwrap();
        assert!(registration.is_enabled().unwrap());
        registration.set_enabled(false).unwrap();
        registration.set_enabled(false).unwrap();
        assert!(!registration.is_enabled().unwrap());
    }
}
