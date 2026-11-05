from homeassistant.helpers.entity import DeviceInfo
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .const import DOMAIN
from .coordinator import AgentCoordinator


class AgentEntity(CoordinatorEntity[AgentCoordinator]):
    _attr_has_entity_name = True

    @property
    def available(self):
        return super().available and self.coordinator.connected

    @property
    def device_info(self) -> DeviceInfo:
        device = self.coordinator.data["device"]
        return DeviceInfo(
            identifiers={(DOMAIN, device["id"])},
            name=device["name"],
            manufacturer="Sidekick",
            model=device["os"],
            sw_version=device["version"],
        )
