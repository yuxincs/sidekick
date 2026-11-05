"""One-time pairing code, confirmation, and per-device revocation."""

import asyncio
import time
import voluptuous as vol
from homeassistant import config_entries
from homeassistant.core import callback
from .client import AgentError
from .const import DOMAIN
from .pairing import async_get_manager


def display_text(value):
    return "".join("\\" + char if char in r"\`*_{}[]()#+.!<>|" else char for char in value)


class SidekickConfigFlow(config_entries.ConfigFlow, domain=DOMAIN):
    VERSION = 2

    def __init__(self):
        self._pair = None
        self._code = ""
        self._manager = None
        self._repair = None

    async def async_step_user(self, user_input=None):
        self._manager = await async_get_manager(self.hass)
        return await self.async_step_pair()

    async def async_step_pair(self, user_input=None):
        errors = {}
        if self._pair and (self._pair.expires <= time.monotonic() or self._pair.error):
            errors["base"] = "pairing_expired"
            self._manager.cancel(self.flow_id)
            self._pair = None
        if self._pair is None:
            try:
                self._pair, self._code = self._manager.create(
                    self.flow_id, self._repair.unique_id if self._repair else None
                )
            except AgentError:
                return self.async_abort(reason="too_many_pairings")
        if user_input is not None and not errors:
            if self._pair.device:
                return await self.async_step_confirm()
            errors["base"] = "waiting_for_device"
        return self.async_show_form(
            step_id="pair", data_schema=vol.Schema({}), errors=errors, description_placeholders={"code": self._code}
        )

    async def async_step_confirm(self, user_input=None):
        pair = self._pair
        if not pair or pair.error or pair.expires <= time.monotonic():
            return await self.async_step_pair()
        errors = {}
        if user_input is not None:
            if not user_input.get("verified"):
                errors["base"] = "confirmation_required"
            else:
                await self.async_set_unique_id(pair.device["id"])
                if not self._repair:
                    self._abort_if_unique_id_configured()
                try:
                    data = await self._manager.approve(pair)
                except asyncio.CancelledError:
                    self._manager.cancel(self.flow_id)
                    raise
                except (AgentError, TimeoutError):
                    self._manager.cancel(self.flow_id)
                    return await self.async_step_pair()
                if self._repair:
                    await self._manager.revoke(self._repair)
                    result = self.async_update_reload_and_abort(self._repair, data_updates=data)
                else:
                    result = self.async_create_entry(title=pair.device["name"], data=data)
                pair.finished.set()
                return result
        return self.async_show_form(
            step_id="confirm",
            data_schema=vol.Schema({vol.Required("verified", default=False): bool}),
            errors=errors,
            description_placeholders={
                "name": display_text(pair.device["name"]),
                "os": display_text(pair.device["os"]),
                "comparison": pair.comparison,
            },
        )

    async def async_step_reconfigure(self, user_input=None):
        self._manager = await async_get_manager(self.hass)
        self._repair = self._get_reconfigure_entry()
        return await self.async_step_pair()

    @callback
    def async_remove(self):
        if self._manager and self._pair and not self._pair.finished.is_set():
            self._manager.cancel(self.flow_id)

    @staticmethod
    @callback
    def async_get_options_flow(config_entry):
        return SidekickOptionsFlow()


class SidekickOptionsFlow(config_entries.OptionsFlow):
    async def async_step_init(self, user_input=None):
        if user_input is not None:
            if user_input.get("revoke"):
                await (await async_get_manager(self.hass)).revoke(self.config_entry)
            return self.async_create_entry(title="", data={})
        return self.async_show_form(
            step_id="init", data_schema=vol.Schema({vol.Required("revoke", default=False): bool})
        )
