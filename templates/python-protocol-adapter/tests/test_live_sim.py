"""The live-sim integration test — the only test in this suite that talks to something outside the
process. Skipped by default so `python -m pytest` (and the CI gate) stays green offline; set
``EC_LIVE_SIM`` to the endpoint of a running simulator or a real device to exercise the connect/poll
path end to end.

    EC_LIVE_SIM=sim://device-1 python -m pytest tests/test_live_sim.py -v

The reference adapters wire the equivalent of this against a permanent, always-on simulator
container (see modbus-adapter's `validation/modbus_sim_server.py` + the permanent
`ggcommons-modbus-sim` Docker container, and ethernet-ip-adapter's cpppo/OpENer harness) — once you
replace ``SimBackend`` with a real protocol backend in ``<<SNAKENAME>>/device.py``, point
``EC_LIVE_SIM`` at that same kind of long-running fixture (a simulator container, or a lab device)
rather than something that only exists for the duration of one CI run.
"""
import os

import pytest

from <<SNAKENAME>>.device import Quality, make_backend

LIVE_SIM = os.environ.get("EC_LIVE_SIM")


@pytest.mark.skipif(not LIVE_SIM, reason="set EC_LIVE_SIM=<endpoint> to run against a live simulator/device")
def test_connect_poll_once_and_assert_readings_and_quality():
    backend = make_backend("sim")
    assert backend is not None, "the bundled sim backend must resolve for adapter='sim'"

    session = backend.connect({"endpoint": LIVE_SIM})
    try:
        # One poll cycle: read every configured signal exactly once, the way the device worker does
        # on each tick of its poll loop.
        readings = session.read_signals()
        by_id = {r.signal_id: r for r in readings}

        assert "temperature-1" in by_id, "the sim's healthy signal must be present"
        temperature = by_id["temperature-1"]
        assert temperature.quality == Quality.GOOD
        assert isinstance(temperature.value, float)

        assert "pressure-1" in by_id, "the sim's always-failing signal must still be reported"
        pressure = by_id["pressure-1"]
        assert pressure.quality == Quality.BAD
        assert pressure.value is None, "a BAD reading carries no value, not a stale/synthesized one"
        assert pressure.quality_raw == "SENSOR_FAULT"
    finally:
        session.close()
