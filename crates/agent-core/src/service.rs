use crate::{
    audio::AudioSubscription,
    config::{Config, Enrollment, Exposure},
    protocol::{Identity, Metrics, PROTOCOL_VERSION, Snapshot},
};
use anyhow::{Result, bail};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub trait MetricsProvider: Send {
    fn sample(&mut self, exposure: Exposure) -> Metrics;
}

pub trait PowerControl: Send + Sync {
    /// Return only after the OS accepts the request, or return an error.
    fn shutdown(&self) -> Result<()>;
}

pub trait AudioControl: Send + Sync {
    fn muted(&self) -> Result<bool>;
    fn set_muted(&self, muted: bool) -> Result<()>;
    fn subscribe(&self) -> Result<AudioSubscription>;
}

struct Inner {
    config: Config,
    metrics: Metrics,
    provider: Box<dyn MetricsProvider>,
    recent_actions: VecDeque<String>,
    audio_generation: u64,
}

pub struct Agent {
    inner: Mutex<Inner>,
    identity: Identity,
    config_path: PathBuf,
    power: Arc<dyn PowerControl>,
    audio: Arc<dyn AudioControl>,
    updates: watch::Sender<Snapshot>,
    audio_policy: watch::Sender<u64>,
}

impl Agent {
    pub fn new(
        config: Config,
        config_path: PathBuf,
        mut identity: Identity,
        provider: Box<dyn MetricsProvider>,
        power: Arc<dyn PowerControl>,
        audio: Arc<dyn AudioControl>,
    ) -> Arc<Self> {
        identity.id = config.device_id.to_string();
        let snapshot = Snapshot {
            protocol: PROTOCOL_VERSION,
            device: identity.clone(),
            exposure: config.exposure,
            metrics: Metrics::default(),
        };
        let (updates, _) = watch::channel(snapshot);
        let (audio_policy, _) = watch::channel(0);
        Arc::new(Self {
            inner: Mutex::new(Inner {
                config,
                metrics: Metrics::default(),
                provider,
                recent_actions: VecDeque::new(),
                audio_generation: 0,
            }),
            identity,
            config_path,
            power,
            audio,
            updates,
            audio_policy,
        })
    }

    fn publish(&self, inner: &Inner) {
        let mut metrics = inner.metrics.clone();
        if !inner.config.exposure.cpu {
            metrics.cpu_percent = None;
        }
        if !inner.config.exposure.memory {
            metrics.memory_percent = None;
        }
        if !inner.config.exposure.audio {
            metrics.audio_muted = None;
        }
        self.updates.send_replace(Snapshot {
            protocol: PROTOCOL_VERSION,
            device: self.identity.clone(),
            exposure: inner.config.exposure,
            metrics,
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.updates.subscribe()
    }

    pub fn enrollment(&self) -> Option<Enrollment> {
        self.inner.lock().unwrap().config.enrollment.clone()
    }

    pub fn set_enrollment(&self, enrollment: Option<Enrollment>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let mut next = inner.config.clone();
        next.enrollment = enrollment;
        next.save(&self.config_path)?;
        inner.config = next;
        Ok(())
    }

    pub fn exposure(&self) -> Exposure {
        self.inner.lock().unwrap().config.exposure
    }

    pub fn set_exposure(&self, exposure: Exposure) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let mut next = inner.config.clone();
        next.exposure = exposure;
        next.save(&self.config_path)?;
        let audio_changed = inner.config.exposure.audio != exposure.audio;
        let muted = if !audio_changed {
            inner.metrics.audio_muted
        } else {
            None
        };
        inner.config = next;
        inner.metrics = Metrics {
            audio_muted: muted,
            ..Metrics::default()
        };
        if audio_changed {
            inner.audio_generation = inner.audio_generation.wrapping_add(1);
            self.audio_policy.send_replace(inner.audio_generation);
        }
        self.publish(&inner);
        Ok(())
    }

    pub fn refresh(&self) {
        let mut inner = self.inner.lock().unwrap();
        let exposure = inner.config.exposure;
        let muted = inner.metrics.audio_muted;
        inner.metrics = inner.provider.sample(exposure);
        // Audio belongs to its event subscription, never the CPU/RAM sampling timer.
        inner.metrics.audio_muted = if exposure.audio { muted } else { None };
        self.publish(&inner);
    }

    fn audio_state(&self) -> (bool, u64) {
        let inner = self.inner.lock().unwrap();
        (inner.config.exposure.audio, inner.audio_generation)
    }

    fn publish_audio(&self, generation: u64, value: Option<bool>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.config.exposure.audio
            && inner.audio_generation == generation
            && inner.metrics.audio_muted != value
        {
            inner.metrics.audio_muted = value;
            self.publish(&inner);
        }
    }

    async fn read_audio(self: &Arc<Self>, generation: u64) -> bool {
        let audio = self.audio.clone();
        let value = tokio::task::spawn_blocking(move || audio.muted().ok())
            .await
            .unwrap_or(None);
        self.publish_audio(generation, value);
        value.is_some()
    }

    /// Register first, read current state, then wait for notifications. Retries only
    /// re-establish failed subscriptions; a healthy backend is never polled.
    pub async fn watch_audio(self: Arc<Self>) {
        use std::time::Duration;
        let mut policy = self.audio_policy.subscribe();
        let mut retry = Duration::from_secs(1);
        loop {
            policy.borrow_and_update();
            let (enabled, generation) = self.audio_state();
            if !enabled {
                if policy.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let audio = self.audio.clone();
            let subscription = tokio::select! {
                _ = policy.changed() => continue,
                result = tokio::task::spawn_blocking(move || audio.subscribe()) => result,
            };
            let mut reconnect_immediately = false;
            if let Ok(Ok(mut subscription)) = subscription {
                let mut initial = true;
                let mut available = true;
                loop {
                    if initial {
                        initial = false;
                    } else {
                        match tokio::select! {
                            _ = policy.changed() => { reconnect_immediately = true; break; },
                            event = subscription.events.next() => event,
                            _ = tokio::time::sleep(retry), if !available => {
                                retry = (retry * 2).min(Duration::from_secs(30));
                                reconnect_immediately = true;
                                break;
                            },
                        } {
                            Some(true) => {
                                reconnect_immediately = true;
                                break;
                            }
                            Some(false) => {}
                            None => break,
                        }
                    }
                    available = tokio::select! {
                        _ = policy.changed() => { reconnect_immediately = true; break; },
                        healthy = self.read_audio(generation) => healthy,
                    };
                    if available {
                        retry = Duration::from_secs(1);
                    }
                }
            }
            if reconnect_immediately {
                // A routine device refresh is not a failed state read. Keep the
                // last value until the new subscription supplies a value/error.
                continue;
            }
            self.publish_audio(generation, None);
            tokio::select! {
                _ = policy.changed() => {},
                _ = tokio::time::sleep(retry) => { retry = (retry * 2).min(Duration::from_secs(30)); },
            }
        }
    }

    pub fn execute(&self, id: &str, action: &str, expires_at: u64) -> Result<()> {
        if id.is_empty() || id.len() > 128 {
            bail!("Invalid action ID");
        }
        {
            let mut inner = self.inner.lock().unwrap();
            // Re-check after acquiring the host-operation lock: a queued worker may have expired.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as u64;
            if expires_at <= now || expires_at > now.saturating_add(30_000) {
                bail!("Action expired or clocks are out of sync");
            }
            match action {
                "shutdown" if !inner.config.exposure.shutdown => {
                    bail!("Shutdown is disabled locally")
                }
                "mute" | "unmute" if !inner.config.exposure.audio => {
                    bail!("Audio control is disabled locally")
                }
                "shutdown" | "mute" | "unmute" => {}
                _ => bail!("Unknown action"),
            }
            if inner.recent_actions.iter().any(|seen| seen == id) {
                bail!("Duplicate action ID");
            }
            inner.recent_actions.push_back(id.to_owned());
            if inner.recent_actions.len() > 256 {
                inner.recent_actions.pop_front();
            }
            if action == "mute" || action == "unmute" {
                let result = self.audio.set_muted(action == "mute");
                // Never optimistically report the requested state as the OS state.
                inner.metrics.audio_muted = self.audio.muted().ok();
                self.publish(&inner);
                return result;
            }
        }
        self.power.shutdown()
    }
}
