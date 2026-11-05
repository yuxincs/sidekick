from homeassistant.components.binary_sensor import BinarySensorEntity, BinarySensorDeviceClass
from homeassistant.helpers.entity import EntityCategory
from .entity import AgentEntity


async def async_setup_entry(hass, entry, async_add_entities):
    async_add_entities([ConnectionSensor(entry.runtime_data)])


class ConnectionSensor(AgentEntity, BinarySensorEntity):
    _attr_name = "Connection"
    _attr_device_class = BinarySensorDeviceClass.CONNECTIVITY
    _attr_entity_category = EntityCategory.DIAGNOSTIC

    def __init__(self, coordinator):
        super().__init__(coordinator)
        self._attr_unique_id = f"{coordinator.entry.unique_id}_connection"

    @property
    def available(self):
        return True

    @property
    def is_on(self):
        return self.coordinator.connected
