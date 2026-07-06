#   Copyright (c) 2024. Amazon.com Inc. or its affiliates.  All Rights Reserved.
#
#    Licensed under the Apache License, Version 2.0 (the "License"). You may not use this file except in compliance
#    with the License. A copy of the License is located at
#
#         http://www.apache.org/licenses/LICENSE-2.0
#
#    or in the 'license' file accompanying this file. This file is distributed on an 'AS IS' BASIS, WITHOUT WARRANTIES
#    OR CONDITIONS OF ANY KIND, express or implied. See the License for the specific language governing permissions
#    and limitations under the License.
#
import time

from edgecommons.metrics.metric import Metric
from edgecommons.config.metric_config import MetricConfiguration


def build_metric_data_emf(
    metric_config: MetricConfiguration,
    metric: Metric,
    measure_values: dict,
    large_fleet_workaround: bool,
):
    emf_object = {}

    aws_object = {
        "Timestamp": int(time.time() * 1000),
        "CloudWatchMetrics": [get_metrics_metadata_emf(metric_config, metric)],
    }

    emf_object["_aws"] = aws_object
    for key, value in metric.get_dimensions().items():
        if large_fleet_workaround and key == "coreName":
            emf_object[key] = "ALL"
        else:
            emf_object[key] = value
    for key, value in measure_values.items():
        emf_object[key] = value

    return emf_object


def get_metrics_metadata_emf(metric_config: MetricConfiguration, metric: Metric):
    namespace = (
        metric.get_namespace()
        if metric.get_namespace() is not None
        else metric_config.get_namespace()
    )
    cw_metrics_array_entry = {
        "Namespace": namespace,
        "Dimensions": [[dimension for dimension in metric.get_dimensions().keys()]],
        "Metrics": [
            {
                "Name": measure.get_name(),
                "Unit": measure.get_unit(),
                "StorageResolution": measure.get_storage_resolution(),
            }
            for measure in metric.get_measures().values()
        ],
    }
    return cw_metrics_array_entry
