from dataclasses import dataclass


@dataclass
class Measure:
    DEFAULT_STORAGE_RESOLUTION = 60  # class constant, not a dataclass field

    name: str
    unit: str
    storage_resolution: int = DEFAULT_STORAGE_RESOLUTION

    def __post_init__(self):
        # CloudWatch only allows storage resolutions of 1 (high-res) or 60.
        self.storage_resolution = 1 if self.storage_resolution < 60 else 60

    # Java-parity accessors retained alongside the public attributes.
    def get_name(self):
        return self.name

    def get_unit(self):
        return self.unit

    def get_storage_resolution(self):
        return self.storage_resolution
