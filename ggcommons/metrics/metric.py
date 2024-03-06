import json
from ggcommons.metrics.metric_emitter import MetricEmitter
from typing import Dict, List


class Metric:
    def __init__(self, name, namespace=None, measures=None, dimensions=None):
        if measures is None:
            measures = {}
        if dimensions is None:
            dimensions = {}

        self.name = name
        self.namespace = namespace
        self.measures = measures
        self.dimensions = dimensions

        # Add default dimensions
        self.dimensions['coreName'] = MetricEmitter.get_thing_name()
        self.dimensions['category'] = name
        self.dimensions['component'] = MetricEmitter.get_component_name()

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

    def dimensions_as_collection(self):
        return [
            {'Name': key, 'Value': value}
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
