"""<<COMPONENTNAME>> — a southbound protocol adapter on the edgecommons Python library.

The package is laid out to teach the same shape as the reference adapters:

* :mod:`.device` — the protocol seam (``DeviceBackend``/``DeviceSession``) + an in-process simulator.
* :mod:`.metrics` — ``southbound_health`` (the §5 canonical set) + the operational-family pattern.
* :mod:`.command_service` — the ``sb/*`` command surface + the three edge-console panels.
* :mod:`.adapter` — the connect/poll/reconnect worker that ties them together (:class:`.adapter.App`).
"""
