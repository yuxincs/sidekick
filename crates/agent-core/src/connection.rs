//! Device-initiated pairing and push sessions; no network listener.
use crate::{
    config::Enrollment,
    credentials::CredentialStore,
    protocol::{ClientMessage, ServerMessage},
    service::Agent,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{Mutex, watch},
    time::{Instant, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;
use uuid::Uuid;

#[derive(Debug)]
struct PairingRejected;
impl std::fmt::Display for PairingRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Pairing was revoked or is no longer valid. Pair again in Home Assistant"
        )
    }
}
impl std::error::Error for PairingRejected {}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionStatus {
    pub phase: String,
    pub message: String,
    pub url: Option<String>,
    pub comparison: Option<String>,
}

pub struct Connector {
    agent: Arc<Agent>,
    store: Arc<dyn CredentialStore>,
    status: watch::Sender<ConnectionStatus>,
    changed: watch::Sender<u64>,
    pairing: Mutex<()>,
    allow_loopback_http: bool,
}

pub fn endpoint(base: &str, path: &str, allow_loopback_http: bool) -> Result<String> {
    let mut url = Url::parse(base).context("Enter a full Home Assistant HTTPS address")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "The address must not contain credentials, a query, or a fragment"
    );
    let local_test =
        allow_loopback_http && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"));
    ensure!(
        url.scheme() == "https" || (local_test && url.scheme() == "http"),
        "Home Assistant must use HTTPS with a trusted certificate"
    );
    ensure!(
        url.host_str().is_some(),
        "Home Assistant address needs a hostname"
    );
    let target = format!("{}{}", url.path().trim_end_matches('/'), path);
    url.set_path(&target);
    let secure = url.scheme() == "https";
    url.set_scheme(if secure { "wss" } else { "ws" })
        .map_err(|_| anyhow::anyhow!("Invalid address"))?;
    Ok(url.into())
}

impl Connector {
    /// The insecure switch is for explicit loopback-only test harnesses; desktop callers pass false.
    pub fn new(
        agent: Arc<Agent>,
        store: Arc<dyn CredentialStore>,
        allow_loopback_http: bool,
    ) -> Arc<Self> {
        let (status, _) = watch::channel(ConnectionStatus {
            phase: "unpaired".into(),
            message: "Connect this computer to Home Assistant".into(),
            url: None,
            comparison: None,
        });
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            agent,
            store,
            status,
            changed,
            pairing: Mutex::new(()),
            allow_loopback_http,
        })
    }
    pub fn subscribe(&self) -> watch::Receiver<ConnectionStatus> {
        self.status.subscribe()
    }
    pub fn is_pairing(&self) -> bool {
        self.pairing.try_lock().is_err()
    }
    fn report(&self, phase: &str, message: &str, comparison: Option<String>, url: Option<String>) {
        self.status.send_replace(ConnectionStatus {
            phase: phase.into(),
            message: message.into(),
            comparison,
            url,
        });
    }
    fn notify(&self) {
        self.changed.send_modify(|generation| *generation += 1);
    }

    pub async fn pair(&self, base: String, code: String) -> Result<()> {
        let _guard = self
            .pairing
            .try_lock()
            .map_err(|_| anyhow::anyhow!("A pairing attempt is already in progress"))?;
        let result = self.pair_inner(base.clone(), code).await;
        if let Err(error) = &result {
            self.report("error", &error.to_string(), None, Some(base));
        }
        result
    }
    async fn pair_inner(&self, base: String, code: String) -> Result<()> {
        let target = endpoint(base.trim(), "/api/sidekick/pair", self.allow_loopback_http)?;
        ensure!(
            (16..=32).contains(&code.len()) && code.is_ascii(),
            "Enter the pairing code shown in Home Assistant"
        );
        self.report(
            "pairing",
            "Connecting securely to Home Assistant…",
            None,
            Some(base.clone()),
        );
        let mut socket = open(&target, None).await?;
        let device = self.agent.subscribe().borrow().device.clone();
        send(
            &mut socket,
            &json!({"type":"pair", "code":code.trim(), "device":device}),
        )
        .await?;
        let pending = receive_json(&mut socket, Duration::from_secs(10)).await?;
        ensure!(
            pending["type"] == "pending",
            "Pairing rejected; generate a new code in Home Assistant"
        );
        let comparison = pending["comparison"]
            .as_str()
            .filter(|s| s.len() == 6 && s.bytes().all(|c| c.is_ascii_digit()))
            .context("Invalid confirmation number")?
            .to_owned();
        self.report(
            "approval",
            "Compare this number in Home Assistant, then approve the computer",
            Some(comparison),
            Some(base.clone()),
        );
        let credential = receive_json(&mut socket, Duration::from_secs(300)).await?;
        ensure!(
            credential["type"] == "credential" && credential["device_id"] == device.id,
            "Pairing rejected or device identity mismatch"
        );
        let secret = credential["token"]
            .as_str()
            .filter(|s| {
                s.len() >= 43
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .context("Invalid device credential")?;
        let instance = credential["instance_id"]
            .as_str()
            .context("Missing Home Assistant identity")?;
        Uuid::parse_str(instance).context("Invalid Home Assistant identity")?;
        let id = Uuid::new_v4().to_string();
        self.store
            .set(&id, secret)
            .context("Could not save the credential securely")?;
        let old = self.agent.enrollment();
        let enrollment = Enrollment {
            url: base.trim().trim_end_matches('/').into(),
            credential_id: id.clone(),
            instance_id: instance.into(),
        };
        if let Err(error) = self.agent.set_enrollment(Some(enrollment)) {
            let _ = self.store.delete(&id);
            return Err(error);
        }
        let completed = async {
            send(&mut socket, &json!({"type":"credential_saved"})).await?;
            let response = receive_json(&mut socket, Duration::from_secs(20)).await?;
            ensure!(
                response["type"] == "paired" && response["instance_id"] == instance,
                "Home Assistant did not finish pairing; try a new code"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = completed {
            self.agent.set_enrollment(old)?;
            let _ = self.store.delete(&id);
            return Err(error);
        }
        if let Some(old) = old {
            let _ = self.store.delete(&old.credential_id);
        }
        let _ = socket.close(None).await;
        self.report(
            "connecting",
            "Paired. Establishing the device connection…",
            None,
            Some(base),
        );
        self.notify();
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        let _guard = self
            .pairing
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Finish or cancel pairing before disconnecting"))?;
        let old = self.agent.enrollment();
        self.agent.set_enrollment(None)?;
        self.notify();
        if let Some(old) = old {
            self.store.delete(&old.credential_id)?;
        }
        self.report("unpaired", "Disconnected. Revoke this computer's pairing in Home Assistant if it is no longer used", None, None);
        Ok(())
    }

    pub async fn run(self: Arc<Self>) {
        let mut changed = self.changed.subscribe();
        let mut backoff = 1;
        loop {
            let Some(enrollment) = self.agent.enrollment() else {
                if changed.changed().await.is_err() {
                    return;
                }
                backoff = 1;
                continue;
            };
            let started = Instant::now();
            let attempt = self.session(&enrollment);
            let result = tokio::select! {
                _ = changed.changed() => { backoff = 1; continue; },
                result = attempt => result,
            };
            if result
                .as_ref()
                .err()
                .is_some_and(|error| error.downcast_ref::<PairingRejected>().is_some())
            {
                if !self.is_pairing() {
                    self.report(
                        "error",
                        "Pairing was revoked or is no longer valid. Pair again in Home Assistant",
                        None,
                        Some(enrollment.url.clone()),
                    );
                }
                if changed.changed().await.is_err() {
                    return;
                }
                backoff = 1;
                continue;
            }
            if !self.is_pairing() {
                let message = result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Connection closed".into());
                self.report(
                    "offline",
                    &format!("{message}. Reconnecting…"),
                    None,
                    Some(enrollment.url.clone()),
                );
            }
            if started.elapsed() > Duration::from_secs(30) {
                backoff = 1;
            }
            tokio::select! { _ = changed.changed() => {backoff = 1;}, _ = tokio::time::sleep(Duration::from_secs(backoff)) => {backoff = (backoff * 2).min(60);} }
        }
    }

    async fn session(&self, enrollment: &Enrollment) -> Result<()> {
        let target = endpoint(
            &enrollment.url,
            "/api/sidekick/ws",
            self.allow_loopback_http,
        )?;
        let secret = self.store.get(&enrollment.credential_id)?;
        let device_id = self.agent.subscribe().borrow().device.id.clone();
        let mut socket = open(&target, Some((&device_id, &secret))).await?;
        let response = receive_json(&mut socket, Duration::from_secs(10)).await?;
        ensure!(
            response["type"] == "authenticated"
                && response["instance_id"] == enrollment.instance_id,
            "Home Assistant identity changed; pair again"
        );
        if !self.is_pairing() {
            self.report(
                "connected",
                "Connected to Home Assistant",
                None,
                Some(enrollment.url.clone()),
            );
        }
        let mut updates = self.agent.subscribe();
        let data = updates.borrow_and_update().clone();
        send(&mut socket, &ServerMessage::Snapshot { data }).await?;
        let mut actions = tokio::task::JoinSet::new();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.tick().await;
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                _ = updates.changed() => {
                    let data = updates.borrow_and_update().clone();
                    send(&mut socket, &ServerMessage::Snapshot {data}).await?;
                }
                result = actions.join_next(), if !actions.is_empty() => {
                    if let Some(Ok(message)) = result { send(&mut socket, &message).await?; }
                }
                _ = heartbeat.tick() => {
                    ensure!(last_seen.elapsed() <= Duration::from_secs(45), "Home Assistant stopped responding");
                    timeout(Duration::from_secs(5), socket.send(Message::Ping(Vec::new().into()))).await??;
                }
                message = socket.next() => {
                    let message = message.context("Home Assistant disconnected")??;
                    last_seen = Instant::now();
                    match message {
                        Message::Text(text) => {
                            let request = serde_json::from_str::<ClientMessage>(&text).context("Invalid action message")?;
                            let ClientMessage::Action { id, action, expires_at } = request;
                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
                            if expires_at <= now || expires_at > now.saturating_add(30_000) {
                                send(&mut socket, &ServerMessage::ActionResult {id,success:false,error:Some("Action expired or clocks are out of sync".into())}).await?;
                                continue;
                            }
                            if actions.len() >= 4 { send(&mut socket, &ServerMessage::ActionResult {id,success:false,error:Some("Too many pending actions".into())}).await?; continue; }
                            let worker = self.agent.clone();
                            actions.spawn(async move {
                                let request_id = id.clone();
                                let result = tokio::task::spawn_blocking(move || worker.execute(&request_id, &action, expires_at)).await;
                                match result {
                                    Ok(result) => ServerMessage::ActionResult {id,success:result.is_ok(),error:result.err().map(|e|e.to_string())},
                                    Err(_) => ServerMessage::ActionResult {id,success:false,error:Some("Action worker failed".into())},
                                }
                            });
                        }
                        Message::Ping(_) => { timeout(Duration::from_secs(5), socket.flush()).await??; }
                        Message::Pong(_) => {},
                        Message::Close(_) => bail!("Home Assistant disconnected; pairing may have been revoked"),
                        _ => bail!("Unexpected message"),
                    }
                }
            }
        }
    }
}

async fn open(url: &str, auth: Option<(&str, &str)>) -> Result<Socket> {
    let mut request = url.into_client_request()?;
    if let Some((id, token)) = auth {
        request
            .headers_mut()
            .insert("X-Sidekick-Device", id.parse()?);
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse()?);
    }
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(4096))
        .max_frame_size(Some(4096));
    let result = timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    .context("Connection timed out")?;
    let (socket, _) = match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status().as_u16() == 401 =>
        {
            return Err(PairingRejected.into());
        }
        result => result.context(
            "Could not connect securely; check the HTTPS address, certificate, and pairing",
        )?,
    };
    Ok(socket)
}
async fn send<T: Serialize>(socket: &mut Socket, message: &T) -> Result<()> {
    timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(serde_json::to_string(message)?.into())),
    )
    .await??;
    Ok(())
}
async fn receive_json(socket: &mut Socket, deadline: Duration) -> Result<Value> {
    timeout(deadline, async {
        while let Some(message) = socket.next().await {
            match message? {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Ping(_) => socket.flush().await?,
                Message::Pong(_) => {}
                _ => bail!("Connection closed before pairing finished"),
            }
        }
        bail!("Connection closed")
    })
    .await
    .context("Pairing timed out")?
}
