use crate::{
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
}

struct Inner {
    config: Config,
    metrics: Metrics,
    provider: Box<dyn MetricsProvider>,
    recent_actions: VecDeque<String>,
}

pub struct Agent {
    inner: Mutex<Inner>,
    identity: Identity,
    config_path: PathBuf,
    power: Arc<dyn PowerControl>,
    audio: Arc<dyn AudioControl>,
    updates: watch::Sender<Snapshot>,
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
        Arc::new(Self {
            inner: Mutex::new(Inner {
                config,
                metrics: Metrics::default(),
                provider,
                recent_actions: VecDeque::new(),
            }),
            identity,
            config_path,
            power,
            audio,
            updates,
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
        inner.config = next;
        // Discard cached data on every policy change; fresh values arrive on the next sample.
        inner.metrics = Metrics::default();
        self.publish(&inner);
        Ok(())
    }

    pub fn refresh(&self) {
        let mut inner = self.inner.lock().unwrap();
        let exposure = inner.config.exposure;
        inner.metrics = inner.provider.sample(exposure);
        inner.metrics.audio_muted = if exposure.audio {
            self.audio.muted().ok()
        } else {
            None
        };
        self.publish(&inner);
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
