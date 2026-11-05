from homeassistant.components.button import ButtonEntity
from homeassistant.exceptions import HomeAssistantError

from .client import AgentError
from .entity import AgentEntity


async def async_setup_entry(hass, entry, async_add_entities):
    async_add_entities([ShutdownButton(entry.runtime_data)])


class ShutdownButton(AgentEntity, ButtonEntity):
    _attr_name = "Shutdown"
    _attr_icon = "mdi:power"

    def __init__(self, coordinator):
        super().__init__(coordinator)
        self._attr_unique_id = f"{coordinator.entry.unique_id}_shutdown"

    @property
    def available(self):
        return super().available and self.coordinator.data["exposure"]["shutdown"]

    async def async_press(self):
        if not self.available or self.coordinator.client is None:
            raise HomeAssistantError("Shutdown unavailable or disabled locally")
        try:
            await self.coordinator.client.shutdown()
        except AgentError as err:
            raise HomeAssistantError(str(err)) from err
