class Measure:
    DEFAULT_STORAGE_RESOLUTION = 60

    def __init__(self, name, unit, storage_resolution=DEFAULT_STORAGE_RESOLUTION):
        self.name = name
        self.unit = unit
        self.storage_resolution = 1 if storage_resolution < 60 else 60

    def get_name(self):
        return self.name

    def get_unit(self):
        return self.unit

    def get_storage_resolution(self):
        return self.storage_resolution
