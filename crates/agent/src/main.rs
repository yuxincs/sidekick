#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod audio;
mod credentials;
mod metrics;
mod power;
mod settings;
mod startup;
mod tray;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs2::FileExt;
use host_agent_core::{config::Config, protocol::Identity, service::Agent};
use std::{fs::OpenOptions, sync::Arc, time::Duration};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let gui = !args
        .iter()
        .any(|a| matches!(a.as_str(), "--help" | "--headless"));
    #[cfg(target_os = "windows")]
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "--help" | "--headless"))
    {
        // A GUI executable stays console-free, while explicit CLI use can attach to its parent.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    if let Err(error) = run(args) {
        eprintln!("Sidekick: {error:#}");
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if gui {
            show_error(&format!("Sidekick could not start:\n{error:#}"));
        }
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Sidekick".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(target_os = "macos")]
fn show_error(message: &str) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAlert, NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::NSString;
    let mtm = MainThreadMarker::new().expect("Application starts on the main thread");
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Sidekick"));
    alert.setInformativeText(&NSString::from_str(message));
    alert.runModal();
}

fn run(args: Vec<String>) -> Result<()> {
    if args.iter().any(|a| a == "--help") {
        println!(
            "sidekick [--headless] [--config-dir PATH]\nPair through the tray menu: Connection settings."
        );
        return Ok(());
    }
    let mut config_override = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--headless" | "--autostart" => {}
            "--config-dir" => {
                index += 1;
                config_override = Some(std::path::PathBuf::from(
                    args.get(index).context("--config-dir needs a path")?,
                ));
            }
            _ => bail!("Unknown option; use --help"),
        }
        index += 1;
    }
    let dirs =
        ProjectDirs::from("dev", "HostAgent", "HostAgent").context("No user config directory")?;
    let config_dir = config_override.unwrap_or_else(|| dirs.config_dir().to_owned());
    std::fs::create_dir_all(&config_dir)?;
    let path = config_dir.join("config.json");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config_dir.join("agent.lock"))?;
    if let Err(error) = lock.try_lock_exclusive() {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            return Ok(());
        }
        return Err(error).context("Could not lock agent config");
    }
    let config = Config::load_or_create(&path)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let id = config.device_id.to_string();
    let identity = Identity {
        id: id.clone(),
        name: sysinfo::System::host_name().unwrap_or_else(|| "Host".into()),
        os: std::env::consts::OS.into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let agent = Agent::new(
        config.clone(),
        path.clone(),
        identity,
        Box::new(metrics::HostMetrics::new()),
        Arc::new(power::HostPower),
        Arc::new(audio::HostAudio),
    );
    let worker = agent.clone();
    let sampler = runtime.spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(config.sample_interval_seconds));
        loop {
            interval.tick().await;
            let worker = worker.clone();
            if tokio::task::spawn_blocking(move || worker.refresh())
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let connector = host_agent_core::connection::Connector::new(
        agent.clone(),
        credentials::store(config_dir.clone()),
        false,
    );
    let connection_task = runtime.spawn(connector.clone().run());
    eprintln!("Sidekick started; configure through the native settings window.");
    if args.iter().any(|a| a == "--headless") {
        runtime.block_on(tokio::signal::ctrl_c())?;
    } else {
        settings::run(agent, connector, runtime.handle().clone())?;
    }
    connection_task.abort();
    sampler.abort();
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}
