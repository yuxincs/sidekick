use host_agent_core::{
    config::{Config, Exposure},
    connection::endpoint,
    credentials::{CredentialStore, FileCredentials},
    protocol::{Identity, Metrics},
    service::{Agent, AudioControl, MetricsProvider, PowerControl},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Samples;
impl MetricsProvider for Samples {
    fn sample(&mut self, _: Exposure) -> Metrics {
        Metrics {
            cpu_percent: Some(42.0),
            memory_percent: Some(61.0),
            ..Metrics::default()
        }
    }
}
struct Power(AtomicUsize);
impl PowerControl for Power {
    fn shutdown(&self) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn fixture() -> (tempfile::TempDir, Arc<Agent>, Arc<Power>) {
    let dir = tempfile::tempdir().unwrap();
    let power = Arc::new(Power(AtomicUsize::new(0)));
    let agent = Agent::new(
        Config::default(),
        dir.path().join("config.json"),
        Identity {
            id: String::new(),
            name: "Test".into(),
            os: "test".into(),
            version: "0.2.0".into(),
        },
        Box::new(Samples),
        power.clone(),
        Arc::new(AudioStub::default()),
    );
    (dir, agent, power)
}
#[test]
fn disabled_metrics_are_filtered_even_for_a_misbehaving_provider() {
    let (_dir, agent, _) = fixture();
    agent.refresh();
    assert!(agent.subscribe().borrow().metrics.cpu_percent.is_none());
    agent
        .set_exposure(Exposure {
            cpu: true,
            ..Exposure::default()
        })
        .unwrap();
    agent.refresh();
    assert_eq!(agent.subscribe().borrow().metrics.cpu_percent, Some(42.0));
    agent.set_exposure(Exposure::default()).unwrap();
    assert!(agent.subscribe().borrow().metrics.cpu_percent.is_none());
}
#[test]
fn local_permission_checks_and_duplicate_ids_are_authoritative() {
    let (_dir, agent, power) = fixture();
    assert!(agent.execute("a", "shutdown", deadline()).is_err());
    agent
        .set_exposure(Exposure {
            shutdown: true,
            ..Exposure::default()
        })
        .unwrap();
    agent.execute("b", "shutdown", deadline()).unwrap();
    assert!(agent.execute("b", "shutdown", deadline()).is_err());
    assert!(agent.execute("c", "unknown", deadline()).is_err());
    agent.set_exposure(Exposure::default()).unwrap();
    assert!(agent.execute("d", "shutdown", deadline()).is_err());
    assert_eq!(power.0.load(Ordering::SeqCst), 1);
}
#[test]
fn failed_settings_write_preserves_policy() {
    let (dir, agent, _) = fixture();
    std::fs::create_dir(dir.path().join("config.json")).unwrap();
    assert!(
        agent
            .set_exposure(Exposure {
                shutdown: true,
                audio: true,
                ..Exposure::default()
            })
            .is_err()
    );
    assert!(!agent.exposure().shutdown);
}
#[test]
fn credentials_are_separate_private_files_and_reject_traversal() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileCredentials(directory.path().join("credentials"));
    let key = uuid::Uuid::new_v4().to_string();
    store.set(&key, "secret").unwrap();
    assert_eq!(store.get(&key).unwrap(), "secret");
    assert!(store.set("../escape", "bad").is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.path().join("credentials").join(&key))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store.delete(&key).unwrap();
    assert!(store.get(&key).is_err());
}
#[test]
fn production_connections_require_https_and_clean_addresses() {
    for url in [
        "http://ha.local:8123",
        "http://127.0.0.1:8123",
        "https://user:pass@ha.local",
        "https://ha.local/?token=secret",
        "https://ha.local/#secret",
    ] {
        assert!(endpoint(url, "/api/sidekick/ws", false).is_err(), "{url}");
    }
    assert_eq!(
        endpoint("https://ha.local:8123/", "/api/sidekick/ws", false).unwrap(),
        "wss://ha.local:8123/api/sidekick/ws"
    );
    assert!(endpoint("http://127.0.0.1:8123", "/api/sidekick/ws", true).is_ok());
    assert!(endpoint("http://192.168.1.2:8123", "/api/sidekick/ws", true).is_err());
}
#[test]
fn old_config_migrates_without_losing_device_or_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let id = uuid::Uuid::new_v4();
    std::fs::write(&path,format!(r#"{{"device_id":"{id}","token":"old-key","port":9176,"exposure":{{"cpu":true,"memory":false,"shutdown":false}},"sample_interval_seconds":5}}"#)).unwrap();
    let config = Config::load_or_create(&path).unwrap();
    assert_eq!(config.device_id, id);
    assert!(config.exposure.cpu);
    assert!(config.enrollment.is_none());
    config.save(&path).unwrap();
    assert!(!std::fs::read_to_string(path).unwrap().contains("old-key"));
}

#[derive(Default)]
struct AudioStub(std::sync::atomic::AtomicBool);
impl AudioControl for AudioStub {
    fn subscribe(&self) -> anyhow::Result<host_agent_core::audio::AudioSubscription> {
        anyhow::bail!("Action-only test")
    }
    fn muted(&self) -> anyhow::Result<bool> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn set_muted(&self, muted: bool) -> anyhow::Result<()> {
        self.0.store(muted, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn audio_is_opt_in_reports_real_state_and_never_changes_volume() {
    let (_dir, agent, _) = fixture();
    assert!(agent.execute("disabled", "mute", deadline()).is_err());
    agent.refresh();
    assert!(agent.subscribe().borrow().metrics.audio_muted.is_none());
    agent
        .set_exposure(Exposure {
            audio: true,
            ..Exposure::default()
        })
        .unwrap();
    agent.refresh();
    assert!(agent.subscribe().borrow().metrics.audio_muted.is_none());
    agent.execute("mute", "mute", deadline()).unwrap();
    assert_eq!(agent.subscribe().borrow().metrics.audio_muted, Some(true));
    assert!(agent.execute("mute", "unmute", deadline()).is_err());
    assert_eq!(agent.subscribe().borrow().metrics.audio_muted, Some(true));
    agent.execute("unmute", "unmute", deadline()).unwrap();
    assert_eq!(agent.subscribe().borrow().metrics.audio_muted, Some(false));
    agent.set_exposure(Exposure::default()).unwrap();
    assert!(agent.subscribe().borrow().metrics.audio_muted.is_none());
    assert!(agent.execute("revoked", "unmute", deadline()).is_err());
}

#[test]
fn audio_errors_remain_unknown_and_disabled_audio_is_never_queried() {
    struct Unavailable(AtomicUsize);
    impl AudioControl for Unavailable {
        fn subscribe(&self) -> anyhow::Result<host_agent_core::audio::AudioSubscription> {
            anyhow::bail!("No output")
        }
        fn muted(&self) -> anyhow::Result<bool> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("No output")
        }
        fn set_muted(&self, _: bool) -> anyhow::Result<()> {
            anyhow::bail!("No output")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let audio = Arc::new(Unavailable(AtomicUsize::new(0)));
    let agent = Agent::new(
        Config::default(),
        dir.path().join("config.json"),
        Identity {
            id: String::new(),
            name: "Test".into(),
            os: "test".into(),
            version: "test".into(),
        },
        Box::new(Samples),
        Arc::new(Power(AtomicUsize::new(0))),
        audio.clone(),
    );
    agent.refresh();
    assert_eq!(audio.0.load(Ordering::SeqCst), 0);
    agent
        .set_exposure(Exposure {
            audio: true,
            ..Exposure::default()
        })
        .unwrap();
    agent.refresh();
    assert_eq!(audio.0.load(Ordering::SeqCst), 0);
    assert!(agent.execute("failed", "mute", deadline()).is_err());
    assert!(agent.subscribe().borrow().metrics.audio_muted.is_none());
}

fn deadline() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 10_000
}

#[test]
fn expired_audio_actions_never_reach_the_os() {
    let (_dir, agent, _) = fixture();
    agent
        .set_exposure(Exposure {
            audio: true,
            ..Exposure::default()
        })
        .unwrap();
    assert!(
        agent
            .execute("expired", "mute", 1)
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
    agent.refresh();
    assert!(agent.subscribe().borrow().metrics.audio_muted.is_none());
}
