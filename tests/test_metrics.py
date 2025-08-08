"""
Unit tests for metrics system.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock

try:
    from ggcommons.metrics.metric import Metric
    from ggcommons.metrics.measure import Measure
    from ggcommons.metrics.metric_service import MetricService
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestMeasure(unittest.TestCase):
    """Test Measure class."""
    
    def test_init(self):
        """Test initialization."""
        measure = Measure("count", "Count", 1.0)
        self.assertEqual(measure.name, "count")
        self.assertEqual(measure.unit, "Count")
        self.assertEqual(measure.value, 1.0)
    
    def test_to_dict(self):
        """Test to_dict method."""
        measure = Measure("count", "Count", 1.0)
        result = measure.to_dict()
        
        expected = {
            "name": "count",
            "unit": "Count",
            "value": 1.0
        }
        self.assertEqual(result, expected)


class TestMetric(unittest.TestCase):
    """Test Metric class."""
    
    def test_init(self):
        """Test initialization."""
        measures = [Measure("count", "Count", 1.0)]
        dimensions = {"instance": "main"}
        
        metric = Metric("test_metric", "TestApp/Metrics", measures, dimensions)
        
        self.assertEqual(metric.name, "test_metric")
        self.assertEqual(metric.namespace, "TestApp/Metrics")
        self.assertEqual(metric.measures, measures)
        self.assertEqual(metric.dimensions, dimensions)
    
    def test_to_dict(self):
        """Test to_dict method."""
        measures = [Measure("count", "Count", 1.0)]
        dimensions = {"instance": "main"}
        
        metric = Metric("test_metric", "TestApp/Metrics", measures, dimensions)
        result = metric.to_dict()
        
        expected = {
            "name": "test_metric",
            "namespace": "TestApp/Metrics",
            "measures": [{"name": "count", "unit": "Count", "value": 1.0}],
            "dimensions": {"instance": "main"}
        }
        self.assertEqual(result, expected)
    
    def test_add_measure(self):
        """Test add_measure method."""
        metric = Metric("test_metric", "TestApp/Metrics", [], {})
        measure = Measure("count", "Count", 1.0)
        
        metric.add_measure(measure)
        
        self.assertEqual(len(metric.measures), 1)
        self.assertEqual(metric.measures[0], measure)
    
    def test_add_dimension(self):
        """Test add_dimension method."""
        metric = Metric("test_metric", "TestApp/Metrics", [], {})
        
        metric.add_dimension("instance", "main")
        
        self.assertEqual(metric.dimensions["instance"], "main")


class TestMetricService(unittest.TestCase):
    """Test MetricService class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.mock_config_manager = Mock()
        self.service = MetricService(self.mock_config_manager)
    
    def test_init(self):
        """Test initialization."""
        self.assertEqual(self.service._config_manager, self.mock_config_manager)
        self.assertEqual(self.service._defined_metrics, {})
    
    def test_define_metric(self):
        """Test define_metric method."""
        metric = Metric("test_metric", "TestApp/Metrics", [], {})
        
        self.service.define_metric(metric)
        
        self.assertIn("test_metric", self.service._defined_metrics)
        self.assertEqual(self.service._defined_metrics["test_metric"], metric)
    
    def test_define_metric_none(self):
        """Test define_metric with None metric."""
        with self.assertRaises(ValueError):
            self.service.define_metric(None)
    
    def test_emit_metric_defined(self):
        """Test emit_metric with defined metric."""
        metric = Metric("test_metric", "TestApp/Metrics", [], {})
        self.service.define_metric(metric)
        
        values = {"count": 1.0}
        
        with patch.object(self.service, '_emit_metric_values') as mock_emit:
            self.service.emit_metric("test_metric", values)
            mock_emit.assert_called_once_with(metric, values)
    
    def test_emit_metric_undefined(self):
        """Test emit_metric with undefined metric."""
        values = {"count": 1.0}
        
        with self.assertRaises(ValueError) as context:
            self.service.emit_metric("undefined_metric", values)
        
        self.assertIn("Metric 'undefined_metric' not defined", str(context.exception))
    
    def test_emit_metric_none_values(self):
        """Test emit_metric with None values."""
        metric = Metric("test_metric", "TestApp/Metrics", [], {})
        self.service.define_metric(metric)
        
        with self.assertRaises(ValueError):
            self.service.emit_metric("test_metric", None)
    
    @patch('ggcommons.metrics.metric_service.MetricEmitter')
    def test_emit_metric_values(self, mock_emitter_class):
        """Test _emit_metric_values method."""
        mock_emitter = Mock()
        mock_emitter_class.return_value = mock_emitter
        
        measures = [Measure("count", "Count", 0)]
        metric = Metric("test_metric", "TestApp/Metrics", measures, {})
        values = {"count": 5.0}
        
        self.service._emit_metric_values(metric, values)
        
        # Verify measure value was updated
        self.assertEqual(measures[0].value, 5.0)
        
        # Verify emitter was called
        mock_emitter.emit_metric.assert_called_once_with(metric)
    
    def test_get_defined_metrics(self):
        """Test get_defined_metrics method."""
        metric1 = Metric("metric1", "TestApp/Metrics", [], {})
        metric2 = Metric("metric2", "TestApp/Metrics", [], {})
        
        self.service.define_metric(metric1)
        self.service.define_metric(metric2)
        
        result = self.service.get_defined_metrics()
        
        self.assertEqual(len(result), 2)
        self.assertIn("metric1", result)
        self.assertIn("metric2", result)


if __name__ == '__main__':
    unittest.main()