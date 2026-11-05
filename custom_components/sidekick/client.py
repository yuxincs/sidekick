"""Versioned WebSocket protocol client; independent of Home Assistant."""

from __future__ import annotations

import math
from typing import Any
from uuid import UUID


class AgentError(Exception):
    """Agent communication or protocol failure."""


class InvalidAuth(AgentError):
    """Pairing token was rejected."""


class ActionError(AgentError):
    """An action failed or its outcome is unknown."""


def snapshot(message: Any) -> dict[str, Any]:
    """Validate the wire contract and discard metrics excluded by local policy."""
    try:
        if message["type"] != "snapshot":
            raise ValueError
        data = message["data"]
        if type(data["protocol"]) is not int or data["protocol"] != 1:
            raise ValueError
        device = data["device"]
        UUID(device["id"])
        for key in ("id", "name", "os", "version"):
            if not isinstance(device[key], str) or not 0 < len(device[key]) <= 255:
                raise ValueError
        exposure = data["exposure"]
        if any(type(exposure[key]) is not bool for key in ("cpu", "memory", "shutdown")):
            raise ValueError
        exposure = {**exposure, "audio": exposure.get("audio", False)}
        if type(exposure["audio"]) is not bool:
            raise ValueError
        metrics = {}
        for policy, key in (("cpu", "cpu_percent"), ("memory", "memory_percent")):
            value = data["metrics"].get(key)
            if exposure[policy] and value is not None:
                if type(value) not in (int, float) or not math.isfinite(value) or not 0 <= value <= 100:
                    raise ValueError
                metrics[key] = value
        muted = data["metrics"].get("audio_muted")
        if exposure["audio"] and muted is not None:
            if type(muted) is not bool:
                raise ValueError
            metrics["audio_muted"] = muted
        return {"protocol": 1, "device": dict(device), "exposure": dict(exposure), "metrics": metrics}
    except (KeyError, TypeError, ValueError, AttributeError) as err:
        raise AgentError("Invalid or unsupported agent snapshot") from err
