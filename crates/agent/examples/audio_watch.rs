//! Read-only native subscription diagnostic. Never changes speaker or microphone state.
#[path = "../src/audio.rs"]
mod audio;
use anyhow::{Result, ensure};
use host_agent_core::service::AudioControl;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let seconds = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "30".into())
        .parse::<u64>()?;
    ensure!(
        (1..=300).contains(&seconds),
        "Usage: audio_watch [seconds: 1..300]"
    );
    let mut subscription = tokio::task::spawn_blocking(|| audio::HostAudio.subscribe()).await??;
    let muted = tokio::task::spawn_blocking(|| audio::HostAudio.muted()).await??;
    println!("Subscribed; initial mute state: {muted}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while let Ok(event) = tokio::time::timeout_at(deadline, subscription.events.next()).await {
        let rebind = event.ok_or_else(|| anyhow::anyhow!("Audio subscription closed"))?;
        if rebind {
            subscription = tokio::task::spawn_blocking(|| audio::HostAudio.subscribe()).await??;
        }
        let muted = tokio::task::spawn_blocking(|| audio::HostAudio.muted()).await??;
        println!("Mute state changed: {muted}");
    }
    println!("Subscription detached");
    Ok(())
}
