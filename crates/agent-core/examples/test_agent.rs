//! Explicit loopback test client. Power control is a harmless fake on every OS.
use host_agent_core::{
    config::{Config, Exposure},
    connection::Connector,
    credentials::FileCredentials,
    protocol::{Identity, Metrics},
    service::{Agent, AudioControl, MetricsProvider, PowerControl},
};
use std::{io::BufRead, sync::Arc, time::Duration};
struct MetricsStub;
impl MetricsProvider for MetricsStub {
    fn sample(&mut self, _: Exposure) -> Metrics {
        Metrics {
            cpu_percent: Some(42.0),
            memory_percent: Some(61.0),
            ..Metrics::default()
        }
    }
}
struct PowerStub;
impl PowerControl for PowerStub {
    fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() == 3, "test_agent <HASS URL> <test pairing code>");
    let directory = tempfile::tempdir()?;
    let audio = Arc::new(AudioStub::default());
    let agent = Agent::new(
        Config {
            device_id: "01234567-89ab-4def-8123-456789abcdef".parse()?,
            exposure: Exposure {
                cpu: true,
                memory: true,
                shutdown: true,
                audio: true,
            },
            ..Config::default()
        },
        directory.path().join("config.json"),
        Identity {
            id: String::new(),
            name: "Test desktop".into(),
            os: "test".into(),
            version: "0.2.0".into(),
        },
        Box::new(MetricsStub),
        Arc::new(PowerStub),
        audio.clone(),
    );
    tokio::spawn(agent.clone().watch_audio());
    agent.refresh();
    let connector = Connector::new(
        agent.clone(),
        Arc::new(FileCredentials(directory.path().join("credentials"))),
        true,
    );
    let status = connector.clone();
    tokio::spawn(async move {
        let mut changes = status.subscribe();
        loop {
            let value = changes.borrow_and_update().clone();
            println!("{}", serde_json::to_string(&value).unwrap());
            if changes.changed().await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(connector.clone().run());
    connector.pair(args[1].clone(), args[2].clone()).await?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });
    // Audio changes must reach HA without relying on a periodic metrics tick.
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = timer.tick() => agent.refresh(),
            line = rx.recv() => {
                let Some(line) = line else {break;};
                if line == "external_mute" { audio.set_muted(true)?; }
                else if line == "external_unmute" { audio.set_muted(false)?; }
                else if line == "audio_rebind" {
                    audio.2.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(source) = audio.1.lock().unwrap().as_ref() { source.changed(true); }
                }
                else if line == "disconnect" { connector.disconnect().await?; }
                else if let Ok(exposure) = serde_json::from_str::<Exposure>(&line) { agent.set_exposure(exposure)?; }
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct AudioStub(
    std::sync::atomic::AtomicBool,
    std::sync::Mutex<Option<host_agent_core::audio::AudioNotifier>>,
    std::sync::atomic::AtomicBool,
);
impl AudioControl for AudioStub {
    fn subscribe(&self) -> anyhow::Result<host_agent_core::audio::AudioSubscription> {
        if self.2.swap(false, std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(150));
        }
        let (source, events) = host_agent_core::audio::audio_channel();
        *self.1.lock().unwrap() = Some(source);
        println!("{}", serde_json::json!({"phase": "audio_subscribed"}));
        Ok(host_agent_core::audio::AudioSubscription::new(events, ()))
    }
    fn muted(&self) -> anyhow::Result<bool> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn set_muted(&self, muted: bool) -> anyhow::Result<()> {
        self.0.store(muted, std::sync::atomic::Ordering::SeqCst);
        if let Some(source) = self.1.lock().unwrap().as_ref() {
            source.changed(false);
        }
        Ok(())
    }
}
