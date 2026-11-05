"""Powered-off computers are normal availability state, never update failures."""

import logging

import pytest

from homeassistant.config_entries import ConfigEntryState
from pytest_homeassistant_custom_component.common import MockConfigEntry

from custom_components.sidekick import pairing
from custom_components.sidekick.const import DOMAIN
from .conftest import DEVICE_ID, make_snapshot, eventually
from .test_integration import begin, claim, approve, session

CPU = "sensor.test_desktop_cpu_usage"
RAM = "sensor.test_desktop_ram_usage"
SHUTDOWN = "button.test_desktop_shutdown"
CONNECTION = "binary_sensor.test_desktop_connection"


def unavailable(hass):
    return all(hass.states.get(entity).state == "unavailable" for entity in (CPU, RAM, SHUTDOWN))


def assert_no_connection_errors(caplog):
    assert not [
        record
        for record in caplog.records
        if record.name.startswith("custom_components.sidekick") and record.levelno >= logging.WARNING
    ]


async def test_hass_starts_with_a_powered_off_computer_and_refresh_stays_offline(hass, pairing_server, caplog):
    device = make_snapshot()["data"]["device"]
    entry = MockConfigEntry(
        domain=DOMAIN,
        title=device["name"],
        unique_id=DEVICE_ID,
        version=2,
        data={"device": device, "token_hash": pairing.digest("secret")},
    )
    entry.add_to_hass(hass)
    assert await hass.config_entries.async_setup(entry.entry_id)
    await hass.async_block_till_done()
    assert entry.state is ConfigEntryState.LOADED
    assert unavailable(hass)
    assert hass.states.get(CONNECTION).state == "off"
    assert not entry.runtime_data.connected
    for _ in range(3):
        await entry.runtime_data.async_refresh()
        assert unavailable(hass)
        assert hass.states.get(CONNECTION).state == "off"
        assert not entry.runtime_data.connected
        assert entry.runtime_data.data["metrics"] == {}
    assert_no_connection_errors(caplog)
    await hass.config_entries.async_unload(entry.entry_id)


async def test_disconnects_are_quiet_and_recovery_waits_for_fresh_data(hass, pairing_server, http_client, caplog):
    _, server = pairing_server
    flow, code = await begin(hass)
    pending, _ = await claim(http_client, server, code)
    entry, token = await approve(hass, flow, pending)
    assert unavailable(hass) and hass.states.get(CONNECTION).state == "off"
    for value in (42.0, 17.0, 9.0):
        ws = await session(http_client, server, token)
        # Authenticated connections have no current metrics until a new snapshot arrives.
        assert not entry.runtime_data.connected
        assert unavailable(hass)
        assert entry.runtime_data.data["metrics"] == {}
        data = make_snapshot(shutdown=True)
        data["data"]["metrics"]["cpu_percent"] = value
        await ws.send_json(data)
        await eventually(lambda: hass.states.get(CPU).state == str(value))
        assert hass.states.get(CONNECTION).state == "on"
        assert hass.states.get(RAM).state == "61.0"
        assert hass.states.get(SHUTDOWN).state != "unavailable"
        await ws.close(code=1001)
        await eventually(lambda: unavailable(hass) and hass.states.get(CONNECTION).state == "off")
        assert entry.runtime_data.data["metrics"] == {}
        assert not entry.runtime_data.connected
        assert entry.state is ConfigEntryState.LOADED
        await entry.runtime_data.async_refresh()
        assert unavailable(hass) and not entry.runtime_data.connected
    assert_no_connection_errors(caplog)
    await hass.config_entries.async_unload(entry.entry_id)


@pytest.mark.parametrize("failure", ["heartbeat", "receive_timeout"])
async def test_silent_peer_heartbeat_timeout_marks_all_entities_offline(
    hass, pairing_server, http_client, monkeypatch, caplog, failure
):
    _, server = pairing_server
    flow, code = await begin(hass)
    pending, _ = await claim(http_client, server, code)
    entry, token = await approve(hass, flow, pending)
    # Accelerate the real transport heartbeat, rather than calling offline() directly.
    monkeypatch.setattr(pairing, "HEARTBEAT_INTERVAL" if failure == "heartbeat" else "RECEIVE_TIMEOUT", 0.2)
    ws = await http_client.ws_connect(
        server.make_url("/api/sidekick/ws"),
        autoping=False,
        headers={"X-Sidekick-Device": DEVICE_ID, "Authorization": f"Bearer {token}"},
    )
    assert (await ws.receive_json(timeout=2))["type"] == "authenticated"
    await ws.send_json(make_snapshot(shutdown=True))
    await eventually(lambda: hass.states.get(CONNECTION).state == "on")
    # No reads/pong responses: simulate a peer that silently disappears.
    await eventually(lambda: unavailable(hass) and hass.states.get(CONNECTION).state == "off", timeout=3)
    assert not entry.runtime_data.connected
    assert entry.runtime_data.data["metrics"] == {}
    assert entry.state is ConfigEntryState.LOADED
    assert_no_connection_errors(caplog)
    if failure == "receive_timeout":
        await ws.receive(timeout=2)
        assert ws.close_code == 1001
    await ws.close()
    await hass.config_entries.async_unload(entry.entry_id)


async def test_reconnection_survives_cleanup_of_the_timed_out_socket(
    hass, pairing_server, http_client, monkeypatch, caplog
):
    manager, server = pairing_server
    flow, code = await begin(hass)
    pending, _ = await claim(http_client, server, code)
    entry, token = await approve(hass, flow, pending)
    monkeypatch.setattr(pairing, "RECEIVE_TIMEOUT", 0.15)
    cleanup_calls = []
    disconnect_session = manager.disconnect_session

    def track_cleanup(device_id, current):
        cleanup_calls.append(current)
        disconnect_session(device_id, current)

    monkeypatch.setattr(manager, "disconnect_session", track_cleanup)
    old = await session(http_client, server, token)
    old_session = manager.sessions[DEVICE_ID]
    await old.send_json(make_snapshot(shutdown=True))
    await eventually(lambda: hass.states.get(CONNECTION).state == "on")
    await eventually(lambda: unavailable(hass) and DEVICE_ID not in manager.sessions, timeout=2)
    # The old connection is still waiting for its close acknowledgement.
    assert cleanup_calls.count(old_session) == 1
    monkeypatch.setattr(pairing, "RECEIVE_TIMEOUT", 45)
    replacement = await session(http_client, server, token)
    new_session = manager.sessions[DEVICE_ID]
    data = make_snapshot(shutdown=True)
    data["data"]["metrics"]["cpu_percent"] = 7.0
    await replacement.send_json(data)
    await eventually(lambda: hass.states.get(CPU).state == "7.0")
    # Finish closing the old socket only after the new one is already online.
    await old.receive(timeout=2)
    await old.close()
    await eventually(lambda: cleanup_calls.count(old_session) >= 2)
    assert manager.sessions[DEVICE_ID] is new_session
    assert entry.runtime_data.client is new_session
    assert entry.runtime_data.connected
    assert hass.states.get(CPU).state == "7.0"
    assert hass.states.get(CONNECTION).state == "on"
    assert_no_connection_errors(caplog)
    await replacement.close()
    await hass.config_entries.async_unload(entry.entry_id)
