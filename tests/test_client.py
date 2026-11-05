import pytest
from custom_components.sidekick.client import AgentError, snapshot
from .conftest import make_snapshot


def test_disabled_values_are_discarded():
    data = snapshot(make_snapshot(cpu=False, memory=False))
    assert data["metrics"] == {}


@pytest.mark.parametrize(
    "mutate",
    [
        lambda m: m["data"].update(protocol=2),
        lambda m: m["data"]["device"].update(id="bad"),
        lambda m: m["data"]["exposure"].update(cpu="yes"),
        lambda m: m["data"]["metrics"].update(cpu_percent=float("nan")),
        lambda m: m["data"]["metrics"].update(cpu_percent=101),
        lambda m: m["data"].update(metrics=[]),
    ],
)
def test_invalid_snapshots(mutate):
    message = make_snapshot()
    mutate(message)
    with pytest.raises(AgentError):
        snapshot(message)


def test_audio_protocol_is_backward_compatible_and_permission_filtered():
    old = snapshot(make_snapshot())
    assert old["exposure"]["audio"] is False
    message = make_snapshot(audio=False)
    message["data"]["metrics"]["audio_muted"] = True
    assert "audio_muted" not in snapshot(message)["metrics"]
    message["data"]["exposure"]["audio"] = True
    assert snapshot(message)["metrics"]["audio_muted"] is True
    message["data"]["metrics"]["audio_muted"] = 1
    with pytest.raises(AgentError):
        snapshot(message)
    message["data"]["exposure"]["audio"] = "yes"
    with pytest.raises(AgentError):
        snapshot(message)
