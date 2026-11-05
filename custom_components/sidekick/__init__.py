"""Sidekick: outbound desktop agents paired through Home Assistant."""

from homeassistant.const import Platform
from .coordinator import AgentCoordinator
from .pairing import async_get_manager

PLATFORMS = [Platform.SENSOR, Platform.BUTTON, Platform.BINARY_SENSOR, Platform.SWITCH]


async def async_setup(hass, config):
    await async_get_manager(hass)
    return True


async def async_setup_entry(hass, entry):
    manager = await async_get_manager(hass)
    coordinator = AgentCoordinator(hass, entry)
    entry.runtime_data = coordinator
    manager.coordinators[entry.unique_id] = coordinator
    try:
        await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    except BaseException:
        manager.coordinators.pop(entry.unique_id, None)
        await coordinator.stop()
        raise
    return True


async def async_unload_entry(hass, entry):
    if await hass.config_entries.async_unload_platforms(entry, PLATFORMS):
        manager = await async_get_manager(hass)
        manager.coordinators.pop(entry.unique_id, None)
        await entry.runtime_data.stop()
        return True
    return False


async def async_remove_entry(hass, entry):
    await (await async_get_manager(hass)).revoke(entry)


async def async_migrate_entry(hass, entry):
    if entry.version == 1:
        device = {"id": entry.unique_id, "name": entry.title, "os": "unknown", "version": "unknown"}
        hass.config_entries.async_update_entry(entry, data={"device": device, "token_hash": ""}, version=2)
    return entry.version == 2
