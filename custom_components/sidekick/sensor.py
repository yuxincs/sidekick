from homeassistant.components.sensor import SensorEntity, SensorStateClass
from homeassistant.const import PERCENTAGE

from .entity import AgentEntity


async def async_setup_entry(hass, entry, async_add_entities):
    coordinator = entry.runtime_data
    async_add_entities(
        [
            UsageSensor(coordinator, "cpu", "cpu_percent", "CPU usage", "mdi:cpu-64-bit"),
            UsageSensor(coordinator, "memory", "memory_percent", "RAM usage", "mdi:memory"),
        ]
    )


class UsageSensor(AgentEntity, SensorEntity):
    _attr_native_unit_of_measurement = PERCENTAGE
    _attr_state_class = SensorStateClass.MEASUREMENT

    def __init__(self, coordinator, policy, metric, name, icon):
        super().__init__(coordinator)
        self._policy = policy
        self._metric = metric
        self._attr_name = name
        self._attr_icon = icon
        self._attr_unique_id = f"{coordinator.entry.unique_id}_{metric}"

    @property
    def available(self):
        return super().available and self.coordinator.data["exposure"][self._policy]

    @property
    def native_value(self):
        data = self.coordinator.data
        if not self.available:
            return None
        return data["metrics"].get(self._metric)
