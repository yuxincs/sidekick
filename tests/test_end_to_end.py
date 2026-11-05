"""Real Rust outbound connection, actual HA config flow/entities, harmless power backend."""

import asyncio
import json
import os
from pathlib import Path
import pytest
from .conftest import eventually
from .test_integration import begin


@pytest.fixture
async def rust_agent(pairing_server, hass):
    manager, server = pairing_server
    flow, code = await begin(hass)
    binary = Path("target/debug/examples/test_agent").resolve()
    if not binary.is_file():
        pytest.fail("Build the outbound test client: cargo build -p host-agent-core --example test_agent")
    process = await asyncio.create_subprocess_exec(
        str(binary),
        str(server.make_url("/")).rstrip("/"),
        code,
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env={**os.environ, **({"SSL_CERT_FILE": server.trust_ca} if hasattr(server, "trust_ca") else {})},
    )
    states = []

    async def read():
        while line := await process.stdout.readline():
            states.append(json.loads(line))

    reader = asyncio.create_task(read())
    try:
        await eventually(
            lambda: any(s["phase"] == "approval" for s in states) or process.returncode is not None, timeout=10
        )
        assert process.returncode is None, (await process.stderr.read()).decode()
        result = await hass.config_entries.flow.async_configure(flow["flow_id"], {})
        assert result["description_placeholders"]["comparison"] == next(
            s["comparison"] for s in states if s["phase"] == "approval"
        )
        result = await hass.config_entries.flow.async_configure(flow["flow_id"], {"verified": True})
        await hass.async_block_till_done()
        entry = result["result"]
        await eventually(lambda: entry.runtime_data.connected, timeout=10)
        yield process, entry, manager, states
        await hass.config_entries.async_unload(entry.entry_id)
    finally:
        if process.returncode is None:
            process.terminate()
        reader.cancel()
        await asyncio.gather(reader, return_exceptions=True)
        await asyncio.wait_for(process.communicate(), 5)


async def test_rust_pairs_pushes_permissions_and_receives_actions(hass, rust_agent):
    process, entry, manager, states = rust_agent
    assert hass.states.get("sensor.test_desktop_cpu_usage").state == "42.0"
    await hass.services.async_call("button", "press", {"entity_id": "button.test_desktop_shutdown"}, blocking=True)
    process.stdin.write((json.dumps({"cpu": False, "memory": False, "shutdown": False}) + "\n").encode())
    await process.stdin.drain()
    await eventually(lambda: hass.states.get("sensor.test_desktop_cpu_usage").state == "unavailable")
    assert entry.runtime_data.data["metrics"] == {}
    await manager.sessions[entry.unique_id].ws.close(code=1001)
    await eventually(lambda: not entry.runtime_data.connected)
    await eventually(lambda: entry.runtime_data.connected, timeout=10)
    assert entry.runtime_data.data["metrics"] == {}
    await manager.revoke(entry)
    await eventually(lambda: not entry.runtime_data.connected)


async def test_rust_rejects_expired_actions_and_local_permission_denials(hass, rust_agent):
    from custom_components.sidekick.client import ActionError

    process, entry, manager, states = rust_agent
    current = manager.sessions[entry.unique_id]
    future = asyncio.get_running_loop().create_future()
    current.pending["expired"] = future
    await current.ws.send_json({"type": "action", "id": "expired", "action": "shutdown", "expires_at": 1})
    with pytest.raises(ActionError, match="expired"):
        await asyncio.wait_for(future, 2)
    current.pending.pop("expired")
    process.stdin.write((json.dumps({"cpu": True, "memory": True, "shutdown": False}) + "\n").encode())
    await process.stdin.drain()
    await eventually(lambda: not entry.runtime_data.data["exposure"]["shutdown"])
    with pytest.raises(ActionError, match="disabled"):
        await current.shutdown()


@pytest.mark.parametrize("pairing_server", ["tls"], indirect=True)
async def test_real_wss_pairing_and_metrics_with_a_trusted_certificate(hass, rust_agent):
    process, entry, manager, states = rust_agent
    assert entry.runtime_data.connected
    assert any(state["phase"] == "connected" for state in states)
    assert hass.states.get("sensor.test_desktop_cpu_usage").state == "42.0"


@pytest.mark.parametrize("pairing_server", ["tls"], indirect=True)
async def test_rust_rejects_an_untrusted_tls_certificate(hass, pairing_server):
    manager, server = pairing_server
    flow, code = await begin(hass)
    binary = Path("target/debug/examples/test_agent").resolve()
    env = {key: value for key, value in os.environ.items() if key not in ("SSL_CERT_FILE", "SSL_CERT_DIR")}
    process = await asyncio.create_subprocess_exec(
        str(binary),
        str(server.make_url("/")).rstrip("/"),
        code,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=env,
    )
    stdout, stderr = await asyncio.wait_for(process.communicate(), 10)
    assert process.returncode != 0
    assert b"Could not connect securely" in stderr
    assert manager.pairings[flow["flow_id"]].device is None


async def test_rust_mute_switch_state_permission_and_disconnect(hass, rust_agent):
    from homeassistant.exceptions import HomeAssistantError
    from custom_components.sidekick.client import ActionError

    process, entry, manager, _ = rust_agent
    entity = "switch.test_desktop_mute"
    await eventually(lambda: hass.states.get(entity).state == "off")
    process.stdin.write(b"external_mute\n")
    await process.stdin.drain()
    await eventually(lambda: hass.states.get(entity).state == "on")
    process.stdin.write(b"external_unmute\n")
    await process.stdin.drain()
    await eventually(lambda: hass.states.get(entity).state == "off")
    await hass.services.async_call("switch", "turn_on", {"entity_id": entity}, blocking=True)
    await eventually(lambda: hass.states.get(entity).state == "on")
    await hass.services.async_call("switch", "turn_off", {"entity_id": entity}, blocking=True)
    await eventually(lambda: hass.states.get(entity).state == "off")
    process.stdin.write((json.dumps({"cpu": True, "memory": True, "shutdown": False, "audio": False}) + "\n").encode())
    await process.stdin.drain()
    await eventually(lambda: hass.states.get(entity).state == "unavailable")
    assert "audio_muted" not in entry.runtime_data.data["metrics"]
    with pytest.raises(ActionError, match="disabled"):
        await manager.sessions[entry.unique_id].set_muted(True)
    process.stdin.write((json.dumps({"cpu": True, "memory": True, "shutdown": False, "audio": True}) + "\n").encode())
    await process.stdin.drain()
    await eventually(lambda: hass.states.get(entity).state == "off")
    await manager.revoke(entry)
    await eventually(lambda: hass.states.get(entity).state == "unavailable")
    # Direct entity method must reject offline calls, even outside HA's service filtering.
    from custom_components.sidekick.switch import MuteSwitch

    with pytest.raises(HomeAssistantError, match="unavailable"):
        await MuteSwitch(entry.runtime_data).async_turn_on()
