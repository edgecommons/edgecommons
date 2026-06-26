"""
The HTTP health subsystem (Phase 1c health slice).

Exposes the minimal, dependency-free health server and the thread-safe readiness state backing the
Kubernetes liveness/readiness/startup probes. See :mod:`ggcommons.health.health_server`.
"""

from ggcommons.health.health_server import HealthServer, ReadinessState

__all__ = ["HealthServer", "ReadinessState"]
