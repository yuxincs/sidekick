<p align="center">
  <img src="custom_components/sidekick/brand/icon@2x.png" alt="Sidekick logo" width="96" height="96">
</p>

# Sidekick

[![Tests](https://github.com/yuxincs/sidekick/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/yuxincs/sidekick/actions/workflows/test.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust: 2024 edition](https://img.shields.io/badge/Rust-2024_edition-orange?logo=rust)](Cargo.toml)
[![Platforms: macOS, Windows, Linux](https://img.shields.io/badge/Platforms-macOS%20%7C%20Windows%20%7C%20Linux-2082dc)](#desktop-apps-and-startup)
[![Home Assistant: 2026.2.3+](https://img.shields.io/badge/Home_Assistant-2026.2.3%2B-18bcf2?logo=homeassistant&logoColor=white)](#pair-a-computer)
[![HACS: Custom integration](https://img.shields.io/badge/HACS-Custom_integration-41bdf5)](#pair-a-computer)

A standalone desktop companion for Home Assistant on Windows, macOS, and Linux. Sidekick opens an **outbound encrypted WebSocket** to Home Assistant, pushes CPU/RAM usage, and receives permitted shutdown and speaker mute/unmute requests. Desktop machines need no LAN listener or inbound firewall exception.

Sidekick uses a one-time pairing code and administrator approval in Home Assistant. It never needs your Home Assistant password, user access token, or refresh token. CPU, RAM, remote shutdown, and audio control start disabled and are controlled locally.

## Pair a computer

1. In Marketplace/HACS, add `https://github.com/yuxincs/sidekick` as a custom repository of type **Integration**, then install Sidekick and restart Home Assistant. Alternatively, extract the integration archive into your Home Assistant configuration directory so `custom_components/sidekick` exists, then restart. Install the desktop app separately on each computer.
2. Make sure your Home Assistant HTTPS address works with a trusted certificate, either on HASS itself or through a TLS reverse proxy. With a proxy, configure Home Assistant's trusted proxies and forwarded headers correctly. Both pairing and device endpoints reject plain HTTP.
3. In **Settings → Devices & services → Add integration → Sidekick**, a four-group pairing code appears. It expires after five minutes.
4. Launch Sidekick. Its first launch opens a native Connection settings window; you can also open it from the S tray/menu-bar icon. Enter your HASS HTTPS address and the pairing code.
5. Sidekick displays a six-digit confirmation number. Return to the HASS flow, select Submit, verify the computer and matching number, check the confirmation box, and approve.
6. The credential is saved, and Sidekick connects automatically. Enable CPU/RAM/shutdown/audio control locally in Connection settings or the tray as desired.

The HASS integration creates CPU and RAM percentage sensors, a shutdown button, a Mute switch (on means muted), and a diagnostic Connection binary sensor. Metrics/actions disabled locally are unavailable. Connection remains visible even when no metrics are exposed. A powered-off or sleeping computer is normal: CPU/RAM, shutdown, and mute controls are unavailable, Connection is off, and cached metrics are cleared without error logs or setup retries. A new connection becomes available only after a fresh valid snapshot, and manual refresh does not change offline status. Previously recorded HASS history is retained when exposure is disabled.

Home Assistant may be on a hostname or IP, but the certificate must match that address. Private CAs can be trusted through your OS certificate store. There is no option to disable certificate verification or pair over plaintext LAN connections. One Sidekick installation connects to one Home Assistant instance at a time.

The integration bundles Sidekick’s S icon in standard and high-resolution sizes. Home Assistant 2026.3 or later displays this local branding; earlier supported versions continue to work without it. After updating the integration, restart Home Assistant to load the branding.

## Desktop apps and startup

| Platform | Package | Installation |
| --- | --- | --- |
| macOS 11+ Intel/Apple Silicon | Universal `.dmg` / `.app.zip` | Drag Sidekick to Applications and launch it. The S icon appears in the menu bar. |
| Windows 10/11 x64 | `-Setup.exe` / portable `.zip` | Install per user or keep the portable executable in a permanent directory. |
| Linux x64, glibc 2.35+ | `.deb` / `.tar.gz` | Install the Debian package, or extract the tarball and run `./install.sh` for a per-user install. |

The tray has Connection settings, Start at login, CPU exposure, RAM exposure, shutdown permission, audio permission, and Quit. Start at login is opt-in and uses a per-user LaunchAgent, Windows Run entry, or XDG autostart file. It starts in a desktop login session, not as a pre-login system service. Moving an app requires re-enabling startup from its new location. Quit stops the current process without removing the login preference.

Settings use a native desktop window rendered by egui/eframe, with shared Rust UI code on all three platforms. There is no web server, browser, or webview. Closing settings hides the window while Sidekick keeps running in the tray; Connection settings reopens it. Pairing runs asynchronously so the window stays responsive. Startup can also be configured in the window.

Standalone apps need no Rust or Python installation. macOS uses OS frameworks; Windows uses system APIs (MSVC CI builds statically link the CRT); Linux uses the desktop's D-Bus/X11/Wayland facilities. Linux tray support requires a StatusNotifierItem host, such as KDE or an AppIndicator extension on GNOME. A headless mode is also available.

Shutdown uses Windows shutdown.exe, macOS System Events, or systemctl poweroff on Linux. It does not elevate privileges or install a privileged helper. macOS may request Automation permission; Linux requires systemd and local power-management authorization. A failed OS command reports an error to HASS. Requests expire after ten seconds and are never replayed on reconnect; severely skewed clocks are rejected. A machine may power off before its acknowledgement arrives, so HASS may report an unknown outcome.

Speaker mute controls the default playback output, never the microphone or volume level. The permission also exposes its current mute state, updated through system notifications and immediately after an action. macOS uses CoreAudio property listeners; Windows uses endpoint-volume and default-device callbacks. Linux uses a persistent `pw-dump --monitor` stream with `wpctl` (PipeWire/WirePlumber), or `pactl subscribe` (PulseAudio or PipeWire's PulseAudio compatibility service). Linux needs `wpctl` plus `pw-dump`, or `pactl`. A healthy audio subscription is never polled. Subscription failures clear the mute state and retry with backoff; default-output changes reattach the native listeners. Disabling audio sharing detaches its subscription. Outputs without mute support or a working audio session appear unavailable. Audio permission defaults to off.

## Revoke, disconnect, or pair again

- In HASS's integration options, **Revoke this computer's pairing** immediately disconnects it and invalidates its credential. Removing its config entry also rejects future connections.
- Use HASS's **Reconfigure** action to generate a new pairing code for the same computer. The persistent device UUID must match, preserving existing entities.
- Sidekick's **Disconnect** clears its saved local enrollment and credential. Revoke the old pairing in HASS if it is no longer needed.
- Uninstall: disable Start at login and quit first. Use Installed apps on Windows, delete the macOS app, or run `~/.local/share/host-agent/uninstall.sh` for a per-user Linux install (adjust for XDG_DATA_HOME). User settings are retained.

## Authentication and credentials

Pairing uses an 80-bit random one-time code, five-minute expiry, one atomic claimant, administrator confirmation, and an acknowledgement after credentials are saved. Pending sockets and attempts are bounded. Codes are invalid after HASS restart, cancellation, or use; interrupted enrollment can require a fresh code. User approval is protected by Home Assistant's existing administrator-only config-flow API.

HASS generates a random 256-bit device credential using Python secrets. Only its SHA-256 hash is retained in the config entry; raw credentials and pairing codes are not logged. Credentials authenticate a single device on a dedicated integration endpoint and grant no general HASS API access. macOS/Windows desktop credentials live in Keychain/Credential Manager; Linux/headless tests use separate owner-only credential files (directory 0700, files 0600). Config JSON contains only the credential reference, HASS URL, and instance ID.

Production device connections always use WSS, system-trusted roots, and normal hostname verification. Revocation closes active sessions; invalid credentials stop retrying until re-pairing. Incoming snapshots and action results have size, schema, and device-identity checks. Shutdown and mute/unmute are allowlisted and checked locally on every invocation. Native WebSocket libraries and TLS implement the transport; no custom cryptography is used.

Rust denies `unsafe` code by default across workspace crates, tests, and examples. The shared core additionally forbids it, so it cannot opt out. Explicit lint exceptions are limited to native audio adapters, Windows Credential Manager, and Windows console/error-dialog calls. Clippy also denies undocumented unsafe blocks: each boundary must explain its safety invariants with a `// SAFETY:` comment. Dependencies have their own safety policies.

## Architecture

```text
crates/agent-core/
  audio.rs          coalesced notifications and subscription lifetimes
  config.rs         stable identity, exposure policy, enrollment reference
  credentials.rs    pluggable secret-storage interface and private-file fallback
  connection.rs     outbound pairing, credential handoff, push, reconnect
  service.rs        provider interfaces, permission checks, action deduplication
  protocol.rs       snapshot and action DTOs
crates/agent/
  credentials.rs    Keychain / Credential Manager / Linux fallback
  settings.rs       native settings, pairing, permissions, startup
  tray.rs           native tray and startup controls
  startup.rs        per-user OS login registration
  metrics.rs        shared sysinfo CPU/RAM implementation
  power.rs          OS-specific shutdown behind PowerControl
  audio.rs          shared audio adapter and native worker lifetime
  audio/            native callbacks and Linux event stream adapters
custom_components/sidekick/
  pairing.py        HASS HTTP views, enrollment and device sessions
  config_flow.py    pairing, confirmation, re-pairing, revocation options
  coordinator.py    incoming push state and availability
  sensor.py         CPU/RAM entities
  binary_sensor.py  connection diagnostic
  button.py         permitted shutdown requests
  switch.py         permission-aware playback mute switch
```

HASS reuses its HTTP listener and aiohttp WebSockets. `/api/sidekick/pair` accepts only pairing traffic; `/api/sidekick/ws` authenticates device credentials before upgrade using Authorization and X-Sidekick-Device headers. The first authenticated message identifies the HASS instance, followed by a complete snapshot from Sidekick. Messages and action responses are bounded at 4 KiB. Snapshots are pushed every five seconds by default, and immediately when exposure or mute state changes. CPU requires a warmup sample.

Ping/pong checks run every 15 seconds; silent peers time out. Sidekick reconnects with exponential delays capped at 60 seconds and sends a fresh snapshot. Action IDs, expiration, up to four workers per connection, and local permission checks prevent automatic replay or delayed shutdown. HASS never polls for metrics or opens a connection to a desktop.

Settings are stored in the OS per-user application config directory. For portable/headless operation or isolated tests, pass `--config-dir PATH`.

## Build, test, and package

```sh
cargo build --workspace --locked
cargo build -p host-agent-core --example test_agent --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
uv sync --locked --python 3.13
uv run ruff check custom_components tests scripts
uv run ruff format --check custom_components tests scripts
uv run pytest -q
```

The test agent is a loopback-only test harness with deterministic metrics and harmless fake power and audio backends. Tests cover the real Rust client and HASS pairing/config-flow/entity lifecycle, TLS trust and rejection, native pairing input validation, audio permissions and state, approval failures, expired/reused codes, wrong-device credentials, reconnects, revocation, and stale actions. Audio tests cover event delivery without metrics ticks, permission changes, stale in-flight reads, device rebinding, stream failure/recovery, and Linux event filtering. No test requests a real OS shutdown, changes host audio, or modifies the user's startup settings.

For a read-only native audio check, run `cargo run -p sidekick --example audio_watch -- 30` and change mute using your keyboard or system settings. It prints subscription updates and detaches after 30 seconds; it never changes audio itself.

Run `cargo run -p sidekick` for the tray, or `sidekick --headless --config-dir PATH`. Headless mode runs an existing enrollment without a settings window or tray; pair and configure through the desktop app first. Release workflow builds native desktop packages on macOS, Windows, and Linux. Package generation uses Python 3.11+: `uv run python scripts/package.py --platform macos --arch aarch64 --binary target/release/sidekick`. Universal macOS builds additionally take `--arch universal --second-binary <intel-binary>`; Windows needs NSIS, Linux needs dpkg-deb. Outputs, integration ZIP, licenses, and checksums are in dist/.

Local macOS apps are ad-hoc signed; public distribution requires Developer ID signing/notarization. The packager supports `--sign-identity` and `--notary-profile`. Windows installers are unsigned until signed with your certificate. Pushing a version tag such as `v0.2.1` publishes a GitHub release after all platform package builds succeed. The tag must match the workspace version in `Cargo.toml`. Release notes list commit messages since the nearest reachable version tag, with commit and comparison links; the first release includes the full history. Tags with a prerelease suffix are marked as prereleases. Installers, portable packages, the Home Assistant integration ZIP, and checksums are attached. Manual workflow runs build artifacts without publishing a release. Native GUI/startup and real shutdown/audio acceptance still require each target OS.

## License

Sidekick is licensed under the [Apache License 2.0](LICENSE). Third-party dependencies retain their own licenses.
