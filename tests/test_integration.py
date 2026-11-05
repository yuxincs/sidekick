import asyncio
import time

import pytest
from homeassistant.config_entries import SOURCE_USER, ConfigEntryState
from homeassistant.data_entry_flow import FlowResultType
from aiohttp import web

from custom_components.sidekick.const import DOMAIN
from custom_components.sidekick.pairing import digest
from .conftest import DEVICE_ID, make_snapshot, eventually


async def begin(hass):
    flow = await hass.config_entries.flow.async_init(DOMAIN, context={"source": SOURCE_USER})
    assert flow["type"] is FlowResultType.FORM and flow["step_id"] == "pair"
    return flow, flow["description_placeholders"]["code"]


async def claim(client, server, code, device=None):
    ws = await client.ws_connect(server.make_url("/api/sidekick/pair"))
    await ws.send_json({"type": "pair", "code": code, "device": device or make_snapshot()["data"]["device"]})
    pending = await ws.receive_json(timeout=2)
    assert pending["type"] == "pending"
    return ws, pending


async def approve(hass, flow, ws):
    result = await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    assert result["step_id"] == "confirm"
    task = asyncio.create_task(hass.config_entries.flow.async_configure(flow["flow_id"], {"verified": True}))
    credential = await ws.receive_json(timeout=2)
    assert credential["type"] == "credential"
    await ws.send_json({"type": "credential_saved"})
    result = await task
    assert (await ws.receive_json(timeout=2))["type"] == "paired"
    await ws.close()
    await hass.async_block_till_done()
    return result["result"], credential["token"]


async def session(client, server, token, device_id=DEVICE_ID):
    ws = await client.ws_connect(
        server.make_url("/api/sidekick/ws"),
        headers={"X-Sidekick-Device": device_id, "Authorization": f"Bearer {token}"},
    )
    assert (await ws.receive_json(timeout=2))["type"] == "authenticated"
    return ws


async def test_pairing_saves_only_hash_and_pushes_entities(hass, pairing_server, http_client):
    manager, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    pair_ws, pending = await claim(client, server, code)
    confirm = await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    assert confirm["description_placeholders"]["comparison"] == pending["comparison"]
    entry, token = await approve(hass, flow, pair_ws)
    assert entry.state is ConfigEntryState.LOADED
    assert entry.data["token_hash"] == digest(token) and token not in str(entry.data)
    ws = await session(client, server, token)
    await ws.send_json(make_snapshot())
    await eventually(lambda: hass.states.get("sensor.test_desktop_cpu_usage").state == "42.0")
    await ws.send_json(make_snapshot(cpu=False, memory=False))
    await eventually(lambda: hass.states.get("sensor.test_desktop_cpu_usage").state == "unavailable")
    assert entry.runtime_data.data["metrics"] == {}
    await ws.close()
    await eventually(lambda: not entry.runtime_data.connected)
    assert DEVICE_ID not in manager.sessions
    await hass.config_entries.async_unload(entry.entry_id)


async def test_code_cannot_be_reused_or_guessed(hass, pairing_server, http_client):
    _, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    ws, _ = await claim(client, server, code)
    for submitted in (code, "WRONG-CODE"):
        other = await client.ws_connect(server.make_url("/api/sidekick/pair"))
        await other.send_json({"type": "pair", "code": submitted, "device": make_snapshot()["data"]["device"]})
        assert (await other.receive(timeout=2)).type != web.WSMsgType.TEXT
        await other.close()
    await ws.close()


async def test_expired_code_is_rejected_and_refreshed(hass, pairing_server, http_client):
    manager, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    manager.pairings[flow["flow_id"]].expires = time.monotonic() - 1
    ws = await client.ws_connect(server.make_url("/api/sidekick/pair"))
    await ws.send_json({"type": "pair", "code": code, "device": make_snapshot()["data"]["device"]})
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    result = await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    assert result["description_placeholders"]["code"] != code
    await ws.close()


async def test_credentials_are_device_scoped_and_revocation_closes_sessions(hass, pairing_server, http_client):
    manager, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    pair_ws, _ = await claim(client, server, code)
    entry, token = await approve(hass, flow, pair_ws)
    import aiohttp

    for device_id, secret in [(DEVICE_ID, "wrong"), ("ffffffff-ffff-4fff-8fff-ffffffffffff", token)]:
        with pytest.raises(aiohttp.WSServerHandshakeError) as error:
            await session(client, server, secret, device_id)
        assert error.value.status == 401
    ws = await session(client, server, token)
    await ws.send_json(make_snapshot())
    await eventually(lambda: entry.runtime_data.connected)
    await manager.revoke(entry)
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    await eventually(lambda: not entry.runtime_data.connected)
    with pytest.raises(aiohttp.WSServerHandshakeError):
        await session(client, server, token)
    await hass.config_entries.async_unload(entry.entry_id)


async def test_identity_spoof_and_malformed_messages_close_connection(hass, pairing_server, http_client):
    _, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    pair_ws, _ = await claim(client, server, code)
    entry, token = await approve(hass, flow, pair_ws)
    for payload in (
        [1],
        {"type": "arbitrary_command"},
        {
            **make_snapshot(),
            "data": {
                **make_snapshot()["data"],
                "device": {**make_snapshot()["data"]["device"], "id": "ffffffff-ffff-4fff-8fff-ffffffffffff"},
            },
        },
    ):
        ws = await session(client, server, token)
        await ws.send_json(payload)
        assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
        await ws.close()
        await eventually(lambda: entry.runtime_data.client is None)
    await hass.config_entries.async_unload(entry.entry_id)


async def test_action_result_and_disconnect_never_retry(hass, pairing_server, http_client):
    _, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    pair_ws, _ = await claim(client, server, code)
    entry, token = await approve(hass, flow, pair_ws)
    ws = await session(client, server, token)
    await ws.send_json(make_snapshot(shutdown=True))
    await eventually(lambda: entry.runtime_data.connected)
    action = asyncio.create_task(entry.runtime_data.client.shutdown())
    message = await ws.receive_json(timeout=2)
    assert message["action"] == "shutdown"
    await ws.send_json({"type": "action_result", "id": message["id"], "success": True, "error": None})
    await action
    action = asyncio.create_task(entry.runtime_data.client.shutdown())
    await ws.receive_json(timeout=2)
    await ws.close()
    from custom_components.sidekick.client import ActionError

    with pytest.raises(ActionError, match="outcome unknown"):
        await action
    await eventually(lambda: entry.runtime_data.client is None)
    replacement = await session(client, server, token)
    with pytest.raises(TimeoutError):
        await replacement.receive_json(timeout=0.05)
    await replacement.close()
    await hass.config_entries.async_unload(entry.entry_id)


async def test_pairing_requires_explicit_confirmation_and_saved_credential(hass, pairing_server, http_client):
    manager, server = pairing_server
    client = http_client
    flow, code = await begin(hass)
    ws, _ = await claim(client, server, code)
    await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    result = await hass.config_entries.flow.async_configure(flow["flow_id"], {"verified": False})
    assert result["errors"] == {"base": "confirmation_required"}
    assert not hass.config_entries.async_entries(DOMAIN)
    await ws.close()
    await eventually(lambda: flow["flow_id"] not in manager.pairings)
    assert not hass.config_entries.async_entries(DOMAIN)


async def test_tls_and_browser_origins_are_enforced(hass, pairing_server, http_client):
    import aiohttp

    manager, server = pairing_server
    manager.allow_insecure_loopback_for_tests = False
    with pytest.raises(aiohttp.WSServerHandshakeError) as error:
        await http_client.ws_connect(server.make_url("/api/sidekick/pair"))
    assert error.value.status == 426
    manager.allow_insecure_loopback_for_tests = True
    with pytest.raises(aiohttp.WSServerHandshakeError) as error:
        await http_client.ws_connect(
            server.make_url("/api/sidekick/pair"), headers={"Origin": "https://attacker.example"}
        )
    assert error.value.status == 403


async def test_pairing_connection_and_attempt_limits(hass, pairing_server, http_client):
    import aiohttp

    manager, server = pairing_server
    manager.pending_sockets = 16
    with pytest.raises(aiohttp.WSServerHandshakeError) as error:
        await http_client.ws_connect(server.make_url("/api/sidekick/pair"))
    assert error.value.status == 429
    manager.pending_sockets = 0
    manager.attempts.extend([time.monotonic()] * 30)
    flow, code = await begin(hass)
    ws = await http_client.ws_connect(server.make_url("/api/sidekick/pair"))
    await ws.send_json({"type": "pair", "code": code, "device": make_snapshot()["data"]["device"]})
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    await ws.close()


async def test_duplicate_session_is_rejected_and_metrics_clear_on_unload(hass, pairing_server, http_client):
    import aiohttp

    manager, server = pairing_server
    flow, code = await begin(hass)
    pair_ws, _ = await claim(http_client, server, code)
    entry, token = await approve(hass, flow, pair_ws)
    ws = await session(http_client, server, token)
    await ws.send_json(make_snapshot())
    await eventually(lambda: entry.runtime_data.connected)
    with pytest.raises(aiohttp.WSServerHandshakeError) as error:
        await session(http_client, server, token)
    assert error.value.status == 409
    coordinator = entry.runtime_data
    await hass.config_entries.async_unload(entry.entry_id)
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    await eventually(lambda: DEVICE_ID not in manager.sessions)
    assert not coordinator.connected and coordinator.data["metrics"] == {}


async def test_bad_credential_ack_does_not_create_an_entry(hass, pairing_server, http_client):
    manager, server = pairing_server
    flow, code = await begin(hass)
    ws, _ = await claim(http_client, server, code)
    await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    task = asyncio.create_task(hass.config_entries.flow.async_configure(flow["flow_id"], {"verified": True}))
    assert (await ws.receive_json(timeout=2))["type"] == "credential"
    await ws.send_json({"type": "not_saved"})
    result = await task
    assert result["step_id"] == "pair" and result["errors"]["base"] == "pairing_expired"
    assert not hass.config_entries.async_entries(DOMAIN)
    await ws.close()


async def test_repair_is_bound_to_the_existing_device_and_replaces_credentials(hass, pairing_server, http_client):
    from homeassistant.config_entries import SOURCE_RECONFIGURE

    manager, server = pairing_server
    flow, code = await begin(hass)
    pair_ws, _ = await claim(http_client, server, code)
    entry, old_token = await approve(hass, flow, pair_ws)
    flow = await hass.config_entries.flow.async_init(
        DOMAIN, context={"source": SOURCE_RECONFIGURE, "entry_id": entry.entry_id}
    )
    code = flow["description_placeholders"]["code"]
    wrong = {**make_snapshot()["data"]["device"], "id": "ffffffff-ffff-4fff-8fff-ffffffffffff"}
    ws = await http_client.ws_connect(server.make_url("/api/sidekick/pair"))
    await ws.send_json({"type": "pair", "code": code, "device": wrong})
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    await ws.close()
    ws, _ = await claim(http_client, server, code)
    await hass.config_entries.flow.async_configure(flow["flow_id"], {})
    task = asyncio.create_task(hass.config_entries.flow.async_configure(flow["flow_id"], {"verified": True}))
    credential = await ws.receive_json(timeout=2)
    await ws.send_json({"type": "credential_saved"})
    result = await task
    assert result["reason"] == "reconfigure_successful"
    assert entry.data["token_hash"] == digest(credential["token"]) and entry.data["token_hash"] != digest(old_token)
    assert len(hass.config_entries.async_entries(DOMAIN)) == 1
    await ws.close()
    await hass.async_block_till_done()
    await hass.config_entries.async_unload(entry.entry_id)


async def test_options_revoke_and_legacy_identity_migration(hass, pairing_server, http_client):
    from pytest_homeassistant_custom_component.common import MockConfigEntry
    from custom_components.sidekick import async_migrate_entry

    manager, server = pairing_server
    legacy = MockConfigEntry(
        domain=DOMAIN,
        title="Legacy desktop",
        unique_id=DEVICE_ID,
        version=1,
        data={"host": "old.local", "token": "obsolete"},
    )
    legacy.add_to_hass(hass)
    assert await async_migrate_entry(hass, legacy)
    assert legacy.version == 2 and legacy.data["device"]["id"] == DEVICE_ID and legacy.data["token_hash"] == ""
    assert "obsolete" not in str(legacy.data)
    hass.config_entries.async_update_entry(legacy, data={**legacy.data, "token_hash": digest("test")})
    result = await hass.config_entries.options.async_init(legacy.entry_id)
    result = await hass.config_entries.options.async_configure(result["flow_id"], {"revoke": True})
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert legacy.data["token_hash"] == ""


async def test_registered_hass_http_endpoint_uses_device_auth(hass, pairing_server, http_client, hass_client):
    manager, server = pairing_server
    flow, code = await begin(hass)
    pair_ws, _ = await claim(http_client, server, code)
    entry, token = await approve(hass, flow, pair_ws)
    client = await hass_client()
    ws = await client.ws_connect(
        "/api/sidekick/ws", headers={"X-Sidekick-Device": DEVICE_ID, "Authorization": f"Bearer {token}"}
    )
    assert (await ws.receive_json(timeout=2))["type"] == "authenticated"
    await ws.send_json(make_snapshot())
    await eventually(lambda: entry.runtime_data.connected)
    await ws.close()
    await hass.config_entries.async_unload(entry.entry_id)


async def test_cancelled_hass_flow_closes_the_pending_pairing(hass, pairing_server, http_client):
    manager, server = pairing_server
    flow, code = await begin(hass)
    ws, _ = await claim(http_client, server, code)
    hass.config_entries.flow.async_abort(flow["flow_id"])
    assert (await ws.receive(timeout=2)).type != web.WSMsgType.TEXT
    await eventually(lambda: not manager.pairings)
    assert not hass.config_entries.async_entries(DOMAIN)
    await ws.close()


async def test_two_codes_cannot_pair_the_same_device_concurrently(hass, pairing_server, http_client):
    manager, server = pairing_server
    first, code = await begin(hass)
    ws, _ = await claim(http_client, server, code)
    second, other_code = await begin(hass)
    other = await http_client.ws_connect(server.make_url("/api/sidekick/pair"))
    await other.send_json({"type": "pair", "code": other_code, "device": make_snapshot()["data"]["device"]})
    assert (await other.receive(timeout=2)).type != web.WSMsgType.TEXT
    assert manager.pairings[first["flow_id"]].claimed
    assert not manager.pairings[second["flow_id"]].claimed
    await other.close()
    await ws.close()
