use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Read, Seek},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
static SELECTED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[derive(Clone, Copy)]
enum Backend {
    PipeWire,
    Pulse,
}
impl Backend {
    fn current() -> Result<Self> {
        match SELECTED.load(std::sync::atomic::Ordering::Acquire) {
            1 => Ok(Self::PipeWire),
            2 => Ok(Self::Pulse),
            _ => Self::detect(),
        }
    }
    fn detect() -> Result<Self> {
        // A desktop can run PipeWire for video while PulseAudio still owns audio.
        // Prefer a live PulseAudio-compatible server, including pipewire-pulse.
        let pulse = Path::new("/usr/bin/pactl").is_file();
        if pulse && Self::Pulse.run(&["info"]).is_ok() {
            Ok(Self::Pulse)
        } else if Path::new("/usr/bin/wpctl").is_file() && Path::new("/usr/bin/pw-dump").is_file() {
            Ok(Self::PipeWire)
        } else if pulse {
            Ok(Self::Pulse)
        } else {
            bail!("Audio subscriptions need wpctl + pw-dump (PipeWire) or pactl (PulseAudio)")
        }
    }
    fn run(&self, args: &[&str]) -> Result<String> {
        let executable = match self {
            Self::PipeWire => "/usr/bin/wpctl",
            Self::Pulse => "/usr/bin/pactl",
        };
        let mut output = tempfile::tempfile()?;
        let errors = tempfile::tempfile()?;
        let mut child = Command::new(executable)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(errors)
            .spawn()
            .context("Could not start audio control")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(
                    status.success(),
                    "Audio control failed ({status}); check your desktop audio session"
                );
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Audio request timed out; outcome unknown and not retried");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        output.rewind()?;
        let mut text = String::new();
        output.take(4096).read_to_string(&mut text)?;
        Ok(text)
    }
}
pub fn muted() -> Result<bool> {
    let backend = Backend::current()?;
    let args = match backend {
        Backend::PipeWire => ["get-volume", "@DEFAULT_AUDIO_SINK@"],
        Backend::Pulse => ["get-sink-mute", "@DEFAULT_SINK@"],
    };
    super::linux_events::parse_mute(&backend.run(&args)?, matches!(backend, Backend::PipeWire))
}
pub fn set_muted(muted: bool) -> Result<()> {
    let backend = Backend::current()?;
    let args = match backend {
        Backend::PipeWire => [
            "set-mute",
            "@DEFAULT_AUDIO_SINK@",
            if muted { "1" } else { "0" },
        ],
        Backend::Pulse => [
            "set-sink-mute",
            "@DEFAULT_SINK@",
            if muted { "1" } else { "0" },
        ],
    };
    backend.run(&args)?;
    Ok(())
}

use super::linux_events::{JsonArrays, PipewireEvents, pulse_changed};
use host_agent_core::audio::{AudioNotifier, AudioSubscription, audio_channel};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Child,
};
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn subscribe() -> Result<AudioSubscription> {
    let backend = Backend::detect()?;
    SELECTED.store(
        match backend {
            Backend::PipeWire => 1,
            Backend::Pulse => 2,
        },
        std::sync::atomic::Ordering::Release,
    );
    let (program, args): (&str, &[&str]) = match backend {
        Backend::PipeWire => ("/usr/bin/pw-dump", &["--monitor", "--no-colors"]),
        Backend::Pulse => ("/usr/bin/pactl", &["subscribe"]),
    };
    let child = tokio::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let (source, events) = audio_channel();
    let task = tokio::spawn(async move {
        let _ = monitor(child, matches!(backend, Backend::PipeWire), source).await;
    });
    Ok(AudioSubscription::new(events, Task(task)))
}
async fn monitor(mut child: Child, pipewire: bool, source: AudioNotifier) -> Result<()> {
    let stdout = child.stdout.take().context("No audio monitor output")?;
    if pipewire {
        let mut stdout = stdout;
        let mut framing = JsonArrays::default();
        let mut events = PipewireEvents::default();
        let mut buffer = [0u8; 8192];
        loop {
            let count = stdout.read(&mut buffer).await?;
            if count == 0 {
                framing.finish()?;
                break;
            }
            for record in framing.feed(&buffer[..count])? {
                if events.changed(&record) {
                    source.changed(false);
                }
            }
        }
    } else {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            ensure!(line.len() <= 4096, "Invalid PulseAudio event");
            if pulse_changed(&line) {
                source.changed(false);
            }
        }
    }
    // EOF invalidates the subscription even if the child has not exited yet.
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}
