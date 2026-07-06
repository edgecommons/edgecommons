from enum import Enum


class Qos(Enum):
    """MQTT Quality of Service level used by EdgeCommons messaging APIs."""

    AT_MOST_ONCE = 0
    AT_LEAST_ONCE = 1
    EXACTLY_ONCE = 2

    @property
    def mqtt_level(self) -> int:
        """Return the MQTT numeric QoS level: 0, 1, or 2."""
        return int(self.value)

    @classmethod
    def from_mqtt_level(cls, mqtt_level: int) -> "Qos":
        try:
            return cls(int(mqtt_level))
        except (ValueError, TypeError) as exc:
            raise ValueError(f"MQTT QoS must be 0, 1, or 2 (got {mqtt_level!r})") from exc
