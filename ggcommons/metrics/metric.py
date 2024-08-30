class Metric:
    def __init__(self, thing_name: str, component_name: str, name: str, namespace: str = None, measures: list = None,
                 dimensions: list = None):
        if measures is None:
            measures = {}
        if dimensions is None:
            dimensions = {}

        self.name = name
        self.namespace = namespace
        self.measures = measures
        self.dimensions = dimensions

        # Add default dimensions
        self.dimensions['coreName'] = thing_name
        self.dimensions['category'] = name
        self.dimensions['component'] = component_name

    def add_measure(self, measure):
        self.measures[measure.name] = measure

    def add_dimension(self, name, value):
        self.dimensions[name] = value

    def dimensions_as_json(self, include_core_name=True) -> list:
        dimensions_list = [
            {'name': key, 'value': value}
            for key, value in self.dimensions.items()
            if include_core_name or key != 'coreName'
        ]
        return dimensions_list

    def dimensions_as_collection(self, large_fleet_workaround: bool = False):
        return [
            {'Name': key, 'Value': "ALL" if large_fleet_workaround and key == 'coreName' else value}
            for key, value in self.dimensions.items()
        ]

    def get_name(self):
        return self.name

    def get_namespace(self):
        return self.namespace

    def get_measures(self):
        return self.measures

    def get_measure(self, name):
        return self.measures.get(name)

    def get_dimensions(self):
        return self.dimensions
