//! Subscription lifecycle tests use no real OS audio devices.
use anyhow::Result;
use host_agent_core::{
    audio::{AudioNotifier, AudioSubscription, audio_channel},
    config::{Config, Exposure},
    protocol::{Identity, Metrics},
    service::{Agent, AudioControl, MetricsProvider, PowerControl},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct FakeAudio {
    muted: AtomicBool,
    fail_registration: AtomicBool,
    fail_read: AtomicBool,
    read_failures: AtomicUsize,
    reads: AtomicUsize,
    subscriptions: AtomicUsize,
    active: Arc<AtomicUsize>,
    source: Arc<Mutex<Option<AudioNotifier>>>,
    block_first: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    block_registration: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    registration_waiting: AtomicBool,
    writes: AtomicUsize,
}
struct Guard {
    source: Arc<Mutex<Option<AudioNotifier>>>,
    active: Arc<AtomicUsize>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.source.lock().unwrap().take();
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl AudioControl for FakeAudio {
    fn muted(&self) -> Result<bool> {
        if self.fail_read.load(Ordering::SeqCst) {
            self.read_failures.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("Audio service temporarily unavailable");
        }
        let value = self.muted.load(Ordering::SeqCst);
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            let receiver = self.block_first.lock().unwrap().take();
            if let Some(receiver) = receiver {
                receiver.recv_timeout(std::time::Duration::from_secs(5))?;
            }
        }
        Ok(value)
    }
    fn set_muted(&self, value: bool) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.change(value, false);
        Ok(())
    }
    fn subscribe(&self) -> Result<AudioSubscription> {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        if self.fail_registration.swap(false, Ordering::SeqCst) {
            anyhow::bail!("Audio service not running");
        }
        let registration = self.block_registration.lock().unwrap().take();
        if let Some(registration) = registration {
            self.registration_waiting.store(true, Ordering::SeqCst);
            registration.recv_timeout(std::time::Duration::from_secs(5))?;
        }
        let (source, events) = audio_channel();
        *self.source.lock().unwrap() = Some(source);
        self.active.fetch_add(1, Ordering::SeqCst);
        Ok(AudioSubscription::new(
            events,
            Guard {
                source: self.source.clone(),
                active: self.active.clone(),
            },
        ))
    }
}
impl FakeAudio {
    fn change(&self, value: bool, rebind: bool) {
        self.muted.store(value, Ordering::SeqCst);
        if let Some(source) = self.source.lock().unwrap().as_ref() {
            source.changed(rebind);
        }
    }
}
struct NoMetrics;
impl MetricsProvider for NoMetrics {
    fn sample(&mut self, _: Exposure) -> Metrics {
        Metrics::default()
    }
}
struct NoPower;
impl PowerControl for NoPower {
    fn shutdown(&self) -> Result<()> {
        anyhow::bail!("Never shut down during tests")
    }
}
fn fixture(audio: Arc<FakeAudio>) -> (tempfile::TempDir, Arc<Agent>) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::new(
        Config::default(),
        dir.path().join("config.json"),
        Identity {
            id: String::new(),
            name: "test".into(),
            os: "test".into(),
            version: "test".into(),
        },
        Box::new(NoMetrics),
        Arc::new(NoPower),
        audio,
    );
    (dir, agent)
}
fn enable(agent: &Agent, audio: bool) {
    agent
        .set_exposure(Exposure {
            audio,
            ..Exposure::default()
        })
        .unwrap();
}
fn state(agent: &Agent) -> Option<bool> {
    agent.subscribe().borrow().metrics.audio_muted
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !ready() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn events_push_without_metrics_ticks_and_permission_controls_subscription() {
    let audio = Arc::new(FakeAudio::default());
    let (_dir, agent) = fixture(audio.clone());
    let task = tokio::spawn(agent.clone().watch_audio());
    agent.refresh();
    tokio::task::yield_now().await;
    assert_eq!(audio.reads.load(Ordering::SeqCst), 0);
    assert_eq!(audio.subscriptions.load(Ordering::SeqCst), 0);
    enable(&agent, true);
    until(|| state(&agent) == Some(false)).await;
    let reads = audio.reads.load(Ordering::SeqCst);
    for _ in 0..20 {
        agent.refresh();
    }
    assert_eq!(audio.reads.load(Ordering::SeqCst), reads);
    audio.change(true, false);
    until(|| state(&agent) == Some(true)).await;
    agent
        .set_exposure(Exposure {
            audio: true,
            cpu: true,
            ..Exposure::default()
        })
        .unwrap();
    agent.refresh();
    assert_eq!(state(&agent), Some(true));
    let stale = audio.source.lock().unwrap().clone().unwrap();
    enable(&agent, false);
    until(|| audio.active.load(Ordering::SeqCst) == 0).await;
    stale.changed(false);
    assert_eq!(state(&agent), None);
    enable(&agent, true);
    until(|| state(&agent) == Some(true)).await;
    assert_eq!(audio.subscriptions.load(Ordering::SeqCst), 2);
    task.abort();
    let _ = task.await;
    assert_eq!(audio.active.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn output_changes_rebind_and_failed_streams_clear_stale_state_then_recover() {
    let audio = Arc::new(FakeAudio::default());
    let (_dir, agent) = fixture(audio.clone());
    enable(&agent, true);
    let task = tokio::spawn(agent.clone().watch_audio());
    until(|| state(&agent) == Some(false)).await;
    audio.change(true, true);
    until(|| state(&agent) == Some(true) && audio.subscriptions.load(Ordering::SeqCst) == 2).await;
    audio.source.lock().unwrap().take();
    until(|| state(&agent).is_none()).await;
    until(|| state(&agent) == Some(true) && audio.subscriptions.load(Ordering::SeqCst) == 3).await;
    task.abort();
    let _ = task.await;
}
#[tokio::test]
async fn an_old_in_flight_read_cannot_publish_after_permission_is_disabled_and_reenabled() {
    let (release, blocked) = std::sync::mpsc::channel();
    let audio = Arc::new(FakeAudio {
        block_first: Mutex::new(Some(blocked)),
        ..FakeAudio::default()
    });
    let (_dir, agent) = fixture(audio.clone());
    enable(&agent, true);
    let task = tokio::spawn(agent.clone().watch_audio());
    until(|| audio.reads.load(Ordering::SeqCst) == 1).await;
    enable(&agent, false);
    until(|| audio.active.load(Ordering::SeqCst) == 0).await;
    audio.muted.store(true, Ordering::SeqCst);
    enable(&agent, true);
    until(|| state(&agent) == Some(true)).await;
    release.send(()).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(state(&agent), Some(true));
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn failed_registration_retry_stops_when_audio_sharing_is_disabled() {
    let audio = Arc::new(FakeAudio::default());
    audio.fail_registration.store(true, Ordering::SeqCst);
    let (_dir, agent) = fixture(audio.clone());
    enable(&agent, true);
    let task = tokio::spawn(agent.clone().watch_audio());
    until(|| audio.subscriptions.load(Ordering::SeqCst) == 1).await;
    enable(&agent, false);
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(state(&agent), None);
    assert_eq!(audio.active.load(Ordering::SeqCst), 0);
    assert_eq!(audio.subscriptions.load(Ordering::SeqCst), 1);
    enable(&agent, true);
    until(|| state(&agent) == Some(false)).await;
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn a_failed_initial_read_recovers_without_requiring_a_new_mute_event() {
    let audio = Arc::new(FakeAudio::default());
    audio.fail_read.store(true, Ordering::SeqCst);
    let (_dir, agent) = fixture(audio.clone());
    enable(&agent, true);
    let task = tokio::spawn(agent.clone().watch_audio());
    until(|| audio.read_failures.load(Ordering::SeqCst) > 0).await;
    assert_eq!(state(&agent), None);
    audio.fail_read.store(false, Ordering::SeqCst);
    until(|| state(&agent) == Some(false)).await;
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn a_healthy_rebind_does_not_make_a_muted_switch_briefly_unavailable() {
    let audio = Arc::new(FakeAudio::default());
    audio.muted.store(true, Ordering::SeqCst);
    let (_dir, agent) = fixture(audio.clone());
    enable(&agent, true);
    let task = tokio::spawn(agent.clone().watch_audio());
    until(|| state(&agent) == Some(true)).await;
    let (ready, registration) = std::sync::mpsc::channel();
    *audio.block_registration.lock().unwrap() = Some(registration);
    audio.change(true, true);
    until(|| audio.registration_waiting.load(Ordering::SeqCst)).await;
    let during_rebind = state(&agent);
    ready.send(()).unwrap();
    until(|| audio.reads.load(Ordering::SeqCst) >= 2).await;
    let after_rebind = state(&agent);
    task.abort();
    let _ = task.await;
    assert_eq!(during_rebind, Some(true));
    assert_eq!(after_rebind, Some(true));
    assert_eq!(audio.writes.load(Ordering::SeqCst), 0);
}
