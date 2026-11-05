"""Push state and expected offline availability, without treating sleep as an error."""

import logging
from homeassistant.core import callback
from homeassistant.helpers.update_coordinator import DataUpdateCoordinator
from .const import DOMAIN


class AgentCoordinator(DataUpdateCoordinator[dict]):
    def __init__(self, hass, entry):
        super().__init__(hass, logging.getLogger(__name__), name=DOMAIN, update_interval=None, config_entry=entry)
        self.entry = entry
        self.client = None
        self._has_snapshot = False
        self.data = {
            "protocol": 1,
            "device": entry.data["device"],
            "exposure": {"cpu": False, "memory": False, "shutdown": False, "audio": False},
            "metrics": {},
        }

    @property
    def connected(self):
        # A successful handshake alone must not restore stale readings or actions.
        return self._has_snapshot and self.client is not None and not self.client.ws.closed

    async def _async_update_data(self):
        # Manual refresh reads our current push state. Offline is valid state,
        # and must neither log an update error nor pretend a connection recovered.
        return self.data

    @callback
    def received(self, data):
        self._has_snapshot = True
        self.async_set_updated_data(data)

    @callback
    def offline(self):
        self._has_snapshot = False
        self.async_set_updated_data({**self.data, "metrics": {}})

    async def stop(self):
        self.offline()
        if self.client:
            await self.client.ws.close(code=1001)
