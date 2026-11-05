"""Device pairing and inbound sessions on Home Assistant's HTTP server."""

from __future__ import annotations

import asyncio
from collections import deque
from dataclasses import dataclass, field
import hashlib
import secrets
import time
from uuid import UUID, uuid4

from aiohttp import web, WSMsgType
from homeassistant.components.http import HomeAssistantView
from homeassistant.const import EVENT_HOMEASSISTANT_STOP
from homeassistant.helpers.storage import Store

from .client import AgentError, ActionError, snapshot
from .const import DOMAIN

PAIR_URL = "/api/sidekick/pair"
SESSION_URL = "/api/sidekick/ws"
PAIR_TTL = 300
MAX_MESSAGE = 4096
HEARTBEAT_INTERVAL = 15
RECEIVE_TIMEOUT = 45


def digest(secret: str) -> str:
    return hashlib.sha256(secret.encode()).hexdigest()


def identity(data):
    if not isinstance(data, dict) or set(data) != {"id", "name", "os", "version"}:
        raise AgentError("Invalid identity")
    if any(not isinstance(v, str) or not 0 < len(v) <= 128 or any(ord(c) < 32 for c in v) for v in data.values()):
        raise AgentError("Invalid identity")
    if str(UUID(data["id"])) != data["id"]:
        raise AgentError("Invalid device ID")
    return dict(data)


@dataclass
class Pairing:
    flow_id: str
    code_hash: str
    expires: float
    expected_id: str | None = None
    device: dict | None = None
    comparison: str = ""
    claimed: bool = False
    error: str | None = None
    token_hash: str | None = None
    decision: asyncio.Future = field(default_factory=lambda: asyncio.get_running_loop().create_future())
    saved: asyncio.Event = field(default_factory=asyncio.Event)
    finished: asyncio.Event = field(default_factory=asyncio.Event)


class DeviceSession:
    def __init__(self, ws, coordinator):
        self.ws = ws
        self.coordinator = coordinator
        self.pending = {}

    async def shutdown(self):
        await self.action("shutdown")

    async def set_muted(self, muted):
        await self.action("mute" if muted else "unmute")

    async def action(self, action):
        if action not in ("shutdown", "mute", "unmute"):
            raise ActionError("Unknown action")
        policy = "shutdown" if action == "shutdown" else "audio"
        if self.ws.closed or not self.coordinator.connected:
            raise ActionError("Sidekick is offline")
        if not self.coordinator.data["exposure"].get(policy, False):
            raise ActionError(f"{policy.capitalize()} is disabled locally")
        if len(self.pending) >= 4:
            raise ActionError("Too many pending actions")
        request_id = str(uuid4())
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        try:
            async with asyncio.timeout(15):
                await self.ws.send_json(
                    {
                        "type": "action",
                        "id": request_id,
                        "action": action,
                        "expires_at": int(time.time() * 1000) + 10000,
                    }
                )
                await future
        except (TimeoutError, ConnectionError) as err:
            raise ActionError(f"{action.capitalize()} outcome unknown; request was not retried") from err
        finally:
            self.pending.pop(request_id, None)
            if not future.done():
                future.cancel()
            elif not future.cancelled():
                future.exception()

    def disconnected(self):
        for future in self.pending.values():
            if not future.done():
                future.set_exception(ActionError("Connection lost; action outcome unknown and not retried"))
        if self.coordinator.client is self:
            self.coordinator.client = None
            self.coordinator.offline()


class PairingManager:
    def __init__(self, hass):
        self.hass = hass
        self.pairings = {}
        self.coordinators = {}
        self.sessions = {}
        self.attempts = deque()
        self.instance_id = ""
        self.allow_insecure_loopback_for_tests = False
        self.pending_sockets = 0

    def require_tls(self, request):
        if not request.secure and not (
            self.allow_insecure_loopback_for_tests and request.remote in ("127.0.0.1", "::1")
        ):
            raise web.HTTPUpgradeRequired(reason="Sidekick requires HTTPS/WSS")

    async def initialize(self):
        store = Store(self.hass, 1, f"{DOMAIN}.identity")
        data = await store.async_load()
        if data is None:
            data = {"id": str(uuid4())}
            await store.async_save(data)
        self.instance_id = data["id"]
        self.hass.http.register_view(PairView(self))
        self.hass.http.register_view(SessionView(self))
        self.hass.bus.async_listen_once(EVENT_HOMEASSISTANT_STOP, self.stop)

    def create(self, flow_id, expected_id=None):
        now = time.monotonic()
        for key, pair in list(self.pairings.items()):
            if pair.expires <= now or key == flow_id:
                self.cancel(key)
        if len(self.pairings) >= 8:
            raise AgentError("Too many pairing requests")
        code = "".join(secrets.choice("0123456789ABCDEFGHJKMNPQRSTVWXYZ") for _ in range(16))
        pair = Pairing(flow_id, digest(code), now + PAIR_TTL, expected_id)
        self.pairings[flow_id] = pair
        return pair, "-".join(code[i : i + 4] for i in range(0, 16, 4))

    def cancel(self, flow_id):
        if pair := self.pairings.pop(flow_id, None):
            pair.error = "pairing_cancelled"
            pair.saved.set()
            if not pair.decision.done():
                pair.decision.set_result(None)
            pair.finished.set()

    def claim(self, code, device):
        now = time.monotonic()
        while self.attempts and self.attempts[0] < now - 60:
            self.attempts.popleft()
        if len(self.attempts) >= 30:
            raise web.HTTPTooManyRequests()
        self.attempts.append(now)
        if not isinstance(code, str) or len(code) > 32:
            raise web.HTTPUnauthorized()
        code_hash = digest(code.replace("-", "").upper())
        for pair in self.pairings.values():
            if pair.expires > now and not pair.claimed and secrets.compare_digest(pair.code_hash, code_hash):
                if pair.expected_id and pair.expected_id != device["id"]:
                    raise web.HTTPUnauthorized()
                if any(
                    other.claimed and other.device and other.device["id"] == device["id"]
                    for other in self.pairings.values()
                ):
                    raise web.HTTPConflict(reason="This computer already has an active pairing attempt")
                existing = self.entry(device["id"])
                if existing and pair.expected_id is None:
                    raise web.HTTPConflict(reason="Already paired; use Reconfigure in HASS")
                pair.claimed = True  # Atomic reservation before the next await.
                pair.device = device
                pair.comparison = f"{secrets.randbelow(1000000):06}"
                return pair
        raise web.HTTPUnauthorized()

    def entry(self, device_id):
        return next(
            (entry for entry in self.hass.config_entries.async_entries(DOMAIN) if entry.unique_id == device_id), None
        )

    async def approve(self, pair):
        if pair.expires <= time.monotonic() or pair.error or not pair.claimed or pair.decision.done():
            raise AgentError("Pairing expired or disconnected")
        token = secrets.token_urlsafe(32)
        pair.token_hash = digest(token)
        pair.decision.set_result(
            {"type": "credential", "token": token, "device_id": pair.device["id"], "instance_id": self.instance_id}
        )
        async with asyncio.timeout(15):
            await pair.saved.wait()
        if pair.error:
            raise AgentError("Pairing disconnected")
        return {"device": pair.device, "token_hash": pair.token_hash}

    def disconnect_session(self, device_id, session):
        # A timed-out socket may still be closing when a new session arrives.
        # Its final cleanup must never remove the replacement connection.
        if self.sessions.get(device_id) is session:
            self.sessions.pop(device_id)
        session.disconnected()

    async def revoke(self, entry):
        self.hass.config_entries.async_update_entry(entry, data={**entry.data, "token_hash": ""})
        if session := self.sessions.get(entry.unique_id):
            await session.ws.close(code=1008, message=b"Pairing revoked")

    async def stop(self, _event=None):
        for flow_id in list(self.pairings):
            self.cancel(flow_id)
        for session in list(self.sessions.values()):
            await session.ws.close(code=1001)


async def async_get_manager(hass):
    key = f"{DOMAIN}_manager_task"
    if key not in hass.data:

        async def create():
            manager = PairingManager(hass)
            await manager.initialize()
            return manager

        hass.data[key] = hass.async_create_task(create())
    return await asyncio.shield(hass.data[key])


class PairView(HomeAssistantView):
    url = PAIR_URL
    name = "api:sidekick:pair"
    requires_auth = False  # The one-time code and subsequent HASS approval authenticate enrollment.

    def __init__(self, manager):
        self.manager = manager

    async def get(self, request):
        self.manager.require_tls(request)
        if request.headers.get("Origin"):
            raise web.HTTPForbidden()
        # Opening unauthenticated sockets is bounded separately from pairing attempts.
        manager = self.manager
        if manager.pending_sockets >= 16:
            raise web.HTTPTooManyRequests()
        ws = web.WebSocketResponse(heartbeat=HEARTBEAT_INTERVAL, max_msg_size=MAX_MESSAGE)
        pair = None
        manager.pending_sockets += 1
        try:
            await ws.prepare(request)
            async with asyncio.timeout(5):
                message = await ws.receive_json()
            if not isinstance(message, dict) or set(message) != {"type", "code", "device"} or message["type"] != "pair":
                raise AgentError("Invalid pairing message")
            device = identity(message["device"])
            pair = manager.claim(message["code"], device)
            await ws.send_json({"type": "pending", "comparison": pair.comparison, "expires_in": PAIR_TTL})
            # Read concurrently so disconnects cannot leave a claim looking active.
            decision = asyncio.ensure_future(asyncio.shield(pair.decision))
            closed = asyncio.create_task(ws.receive())
            try:
                done, _ = await asyncio.wait(
                    [decision, closed],
                    timeout=max(0, pair.expires - time.monotonic()),
                    return_when=asyncio.FIRST_COMPLETED,
                )
                if decision not in done or not decision.result():
                    raise AgentError("Pairing cancelled or disconnected")
                if closed.done():
                    raise AgentError("Pairing disconnected")
                closed.cancel()
                await asyncio.gather(closed, return_exceptions=True)
                await ws.send_json(decision.result())
                async with asyncio.timeout(15):
                    ack = await ws.receive_json()
                if ack != {"type": "credential_saved"}:
                    raise AgentError("Credential was not saved")
                pair.saved.set()
                async with asyncio.timeout(15):
                    await pair.finished.wait()
                if pair.error:
                    raise AgentError("Pairing cancelled")
                await ws.send_json({"type": "paired", "instance_id": manager.instance_id})
            finally:
                decision.cancel()
                closed.cancel()
                await asyncio.gather(decision, closed, return_exceptions=True)
        except (AgentError, ValueError, KeyError, TypeError, TimeoutError, ConnectionError, web.HTTPException):
            if pair:
                pair.error = "pairing_disconnected"
                pair.saved.set()
            if not ws.prepared:
                raise
            if not ws.closed:
                await ws.close(code=1008, message=b"Pairing failed; generate a new code in HASS")
        finally:
            manager.pending_sockets -= 1
            if pair:
                manager.pairings.pop(pair.flow_id, None)
            if ws.prepared:
                await ws.close()
        return ws


class SessionView(HomeAssistantView):
    url = SESSION_URL
    name = "api:sidekick:ws"
    requires_auth = False  # Device-scoped bearer credentials, not HASS user tokens.

    def __init__(self, manager):
        self.manager = manager

    async def get(self, request):
        self.manager.require_tls(request)
        if request.headers.get("Origin"):
            raise web.HTTPForbidden()
        manager = self.manager
        device_id = request.headers.get("X-Sidekick-Device", "")
        entry = manager.entry(device_id)
        header = request.headers.get("Authorization", "")
        supplied = header[7:] if header.startswith("Bearer ") else ""
        stored = entry.data.get("token_hash", "") if entry else ""
        if not supplied or len(supplied) > 256 or not stored or not secrets.compare_digest(digest(supplied), stored):
            raise web.HTTPUnauthorized()
        coordinator = manager.coordinators.get(device_id)
        if coordinator is None:
            raise web.HTTPServiceUnavailable()
        if device_id in manager.sessions:
            raise web.HTTPConflict()
        ws = web.WebSocketResponse(heartbeat=HEARTBEAT_INTERVAL, max_msg_size=MAX_MESSAGE)
        # Reserve before await to reject simultaneous valid connections.
        session = DeviceSession(ws, coordinator)
        manager.sessions[device_id] = session
        try:
            await ws.prepare(request)
            if not secrets.compare_digest(entry.data.get("token_hash", ""), stored):
                await ws.close(code=1008, message=b"Pairing revoked")
                return ws
            coordinator.client = session
            await ws.send_json({"type": "authenticated", "instance_id": manager.instance_id})
            while True:
                message = await ws.receive(timeout=RECEIVE_TIMEOUT)
                if message.type != WSMsgType.TEXT:
                    break
                payload = message.json()
                if not isinstance(payload, dict):
                    raise AgentError("Invalid message")
                if payload.get("type") == "snapshot":
                    data = snapshot(payload)
                    if data["device"]["id"] != device_id:
                        raise AgentError("Device identity mismatch")
                    coordinator.received(data)
                elif payload.get("type") == "action_result":
                    if not isinstance(payload.get("id"), str) or type(payload.get("success")) is not bool:
                        raise AgentError("Invalid action result")
                    future = session.pending.get(payload["id"])
                    if future and not future.done():
                        if payload["success"]:
                            future.set_result(None)
                        else:
                            future.set_exception(ActionError(str(payload.get("error") or "Action failed")[:512]))
                else:
                    raise AgentError("Unexpected message")
        except (TimeoutError, ConnectionError):
            manager.disconnect_session(device_id, session)
            await ws.close(code=1001, message=b"Connection unavailable")
        except (AgentError, ValueError, TypeError):
            manager.disconnect_session(device_id, session)
            await ws.close(code=1008, message=b"Invalid message")
        finally:
            manager.disconnect_session(device_id, session)
            if ws.prepared:
                await ws.close()
        return ws
