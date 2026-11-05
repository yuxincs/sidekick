//! Default playback-output mute control and native event subscriptions.
use anyhow::Result;
use host_agent_core::{audio::AudioSubscription, service::AudioControl};
#[cfg(any(target_os = "linux", test))]
#[path = "audio/linux_events.rs"]
mod linux_events;
#[cfg(target_os = "macos")]
#[path = "audio/macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "audio/windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "audio/linux.rs"]
mod platform;

pub struct HostAudio;
impl AudioControl for HostAudio {
    fn muted(&self) -> Result<bool> {
        platform::muted()
    }
    fn set_muted(&self, muted: bool) -> Result<()> {
        platform::set_muted(muted)
    }
    fn subscribe(&self) -> Result<AudioSubscription> {
        platform::subscribe()
    }
}

/// Native handles remain on their owning thread. Dropping the returned guard
/// closes the stop channel; callbacks are unregistered before COM/blocks are freed.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn native_subscription<F, G>(setup: F) -> Result<AudioSubscription>
where
    F: FnOnce(host_agent_core::audio::AudioNotifier) -> Result<G> + Send + 'static,
    G: 'static,
{
    use host_agent_core::audio::audio_channel;
    use std::{sync::mpsc, time::Duration};
    let (source, events) = audio_channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("sidekick-audio".into())
        .spawn(move || match setup(source) {
            Ok(guard) => {
                let _ = ready.send(Ok(()));
                let _ = stopped.recv();
                drop(guard);
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        })?;
    started.recv_timeout(Duration::from_secs(5))??;
    Ok(AudioSubscription::new(events, stop))
}
