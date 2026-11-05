"""Default playback-output mute control, explicitly permitted on the computer."""

from homeassistant.components.switch import SwitchEntity
from homeassistant.exceptions import HomeAssistantError
from .client import AgentError
from .entity import AgentEntity


async def async_setup_entry(hass, entry, async_add_entities):
    async_add_entities([MuteSwitch(entry.runtime_data)])


class MuteSwitch(AgentEntity, SwitchEntity):
    _attr_name = "Mute"
    _attr_icon = "mdi:volume-off"

    def __init__(self, coordinator):
        super().__init__(coordinator)
        self._attr_unique_id = f"{coordinator.entry.unique_id}_mute"

    @property
    def available(self):
        return (
            super().available
            and self.coordinator.data["exposure"].get("audio", False)
            and type(self.coordinator.data["metrics"].get("audio_muted")) is bool
        )

    @property
    def is_on(self):
        return self.coordinator.data["metrics"].get("audio_muted")

    async def async_turn_on(self, **kwargs):
        await self._set_muted(True)

    async def async_turn_off(self, **kwargs):
        await self._set_muted(False)

    async def _set_muted(self, muted):
        if not self.available or self.coordinator.client is None:
            raise HomeAssistantError("Audio control unavailable or disabled locally")
        try:
            await self.coordinator.client.set_muted(muted)
        except AgentError as err:
            raise HomeAssistantError(str(err)) from err
