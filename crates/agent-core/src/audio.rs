//! Coalesced, nonblocking notifications shared by native audio backends.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

struct Signal {
    ready: Notify,
    rebind: AtomicBool,
    closed: AtomicBool,
}
struct Source(Arc<Signal>);
impl Drop for Source {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.ready.notify_one();
    }
}

/// Clones may be called from native callbacks: no I/O, blocking, or allocation.
#[derive(Clone)]
pub struct AudioNotifier(Arc<Source>);
impl AudioNotifier {
    pub fn changed(&self, rebind: bool) {
        if rebind {
            self.0.0.rebind.store(true, Ordering::Release);
        }
        self.0.0.ready.notify_one();
    }
}

pub struct AudioNotifications(Arc<Signal>);
impl AudioNotifications {
    /// None means the backend exited; true requests a new default-device subscription.
    pub async fn next(&mut self) -> Option<bool> {
        self.0.ready.notified().await;
        if self.0.closed.load(Ordering::Acquire) {
            None
        } else {
            Some(self.0.rebind.swap(false, Ordering::AcqRel))
        }
    }
}

pub fn audio_channel() -> (AudioNotifier, AudioNotifications) {
    let signal = Arc::new(Signal {
        ready: Notify::new(),
        rebind: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    (
        AudioNotifier(Arc::new(Source(signal.clone()))),
        AudioNotifications(signal),
    )
}

/// Dropping this value detaches callbacks or stops the platform's event stream.
pub struct AudioSubscription {
    pub events: AudioNotifications,
    _guard: Box<dyn Send>,
}
impl AudioSubscription {
    pub fn new(events: AudioNotifications, guard: impl Send + 'static) -> Self {
        Self {
            events,
            _guard: Box::new(guard),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn coalescing_preserves_device_changes_and_detects_closed_sources() {
        let (source, mut events) = audio_channel();
        source.changed(false);
        source.changed(true);
        source.changed(false);
        assert_eq!(events.next().await, Some(true));
        source.changed(false);
        assert_eq!(events.next().await, Some(false));
        let other = source.clone();
        drop(source);
        other.changed(false);
        assert_eq!(events.next().await, Some(false));
        drop(other);
        assert_eq!(events.next().await, None);
    }
}
