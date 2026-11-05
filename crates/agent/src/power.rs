use anyhow::{Result, bail};
use host_agent_core::service::PowerControl;
use std::process::Command;

pub struct HostPower;

impl PowerControl for HostPower {
    fn shutdown(&self) -> Result<()> {
        platform::shutdown()
    }
}

fn run(mut command: Command) -> Result<()> {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    // Keep native permission prompts from pinning a worker forever.
    let mut errors = tempfile::tempfile()?;
    let mut child = command
        .stdout(Stdio::null())
        .stderr(errors.try_clone()?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                use std::io::{Read, Seek};
                errors.rewind()?;
                let mut detail = String::new();
                errors.take(4096).read_to_string(&mut detail)?;
                bail!("OS refused shutdown ({status}): {}", detail.trim());
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Shutdown outcome unknown: OS request timed out; not retried");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    pub fn shutdown() -> Result<()> {
        let root = std::env::var_os("SystemRoot")
            .ok_or_else(|| anyhow::anyhow!("SystemRoot is missing"))?;
        let mut command =
            Command::new(std::path::PathBuf::from(root).join("System32/shutdown.exe"));
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        command.args(["/s", "/t", "0"]);
        run(command)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    pub fn shutdown() -> Result<()> {
        let mut command = Command::new("/usr/bin/osascript");
        command.args(["-e", "tell application \"System Events\" to shut down"]);
        run(command)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    pub fn shutdown() -> Result<()> {
        let mut command = Command::new("/usr/bin/systemctl");
        command.args(["--no-ask-password", "poweroff"]);
        run(command)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
compile_error!("Supported platforms: Windows, macOS, Linux");
