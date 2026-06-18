class Metric:
    # CloudWatch allows at most 10 dimensions per metric.
    MAX_DIMENSIONS = 10

    def __init__(
        self,
        thing_name: str,
        component_name: str,
        name: str,
        namespace: str = None,
        measures: dict = None,
        dimensions: dict = None,
    ):
        # Copy the incoming dicts so we never mutate a caller-owned collection when
        # injecting the default dimensions below.
        self.name = name
        self.namespace = namespace
        self.measures = dict(measures) if measures else {}
        self.dimensions = dict(dimensions) if dimensions else {}

        # Add default dimensions
        self.dimensions["coreName"] = thing_name
        self.dimensions["category"] = name
        self.dimensions["component"] = component_name

    def add_measure(self, measure):
        self.measures[measure.name] = measure

    def add_dimension(self, name, value):
        # Enforce the CloudWatch 10-dimension cap on the Metric itself (not just in
        # the builder), so the limit holds regardless of how the metric is built.
        if name not in self.dimensions and len(self.dimensions) >= self.MAX_DIMENSIONS:
            raise ValueError(
                f"Cannot add dimension '{name}': a metric may have at most "
                f"{self.MAX_DIMENSIONS} dimensions"
            )
        self.dimensions[name] = value

    def dimensions_as_json(self, include_core_name=True) -> list:
        dimensions_list = [
            {"name": key, "value": value}
            for key, value in self.dimensions.items()
            if include_core_name or key != "coreName"
        ]
        return dimensions_list

    def dimensions_as_collection(self, large_fleet_workaround: bool = False):
        return [
            {
                "Name": key,
                "Value": "ALL"
                if large_fleet_workaround and key == "coreName"
                else value,
            }
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
