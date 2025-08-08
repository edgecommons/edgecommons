"""
Unit tests for builder classes.
"""

import unittest
from unittest.mock import Mock, patch

try:
    from ggcommons.ggcommons_builder import GGCommonsBuilder
    from ggcommons.messaging.message_builder import MessageBuilder
    from ggcommons.metrics.metric_builder import MetricBuilder
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestGGCommonsBuilder(unittest.TestCase):
    """Test GGCommonsBuilder class."""
    
    def test_create(self):
        """Test create method."""
        builder = GGCommonsBuilder.create("com.test.Component")
        self.assertIsInstance(builder, GGCommonsBuilder)
        self.assertEqual(builder._component_name, "com.test.Component")
    
    def test_create_empty_name(self):
        """Test create with empty component name."""
        with self.assertRaises(ValueError):
            GGCommonsBuilder.create("")
    
    def test_create_none_name(self):
        """Test create with None component name."""
        with self.assertRaises(ValueError):
            GGCommonsBuilder.create(None)
    
    def test_with_args(self):
        """Test with_args method."""
        builder = GGCommonsBuilder.create("com.test.Component")
        args = ["-c", "FILE", "config.json"]
        
        result = builder.with_args(args)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder._args, args)
    
    def test_with_app_options(self):
        """Test with_app_options method."""
        builder = GGCommonsBuilder.create("com.test.Component")
        options = Mock()
        
        result = builder.with_app_options(options)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder._app_options, options)
    
    def test_with_receive_own_messages(self):
        """Test with_receive_own_messages method."""
        builder = GGCommonsBuilder.create("com.test.Component")
        
        result = builder.with_receive_own_messages(True)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertTrue(builder._receive_own_messages)
    
    @patch('ggcommons.ggcommons_builder.GGCommons')
    def test_build(self, mock_ggcommons_class):
        """Test build method."""
        mock_ggcommons = Mock()
        mock_ggcommons_class.return_value = mock_ggcommons
        
        builder = GGCommonsBuilder.create("com.test.Component") \
            .with_args(["-c", "FILE", "config.json"]) \
            .with_receive_own_messages(False)
        
        result = builder.build()
        
        self.assertEqual(result, mock_ggcommons)
        mock_ggcommons_class.assert_called_once_with(
            "com.test.Component",
            ["-c", "FILE", "config.json"],
            None,
            False
        )


class TestMessageBuilder(unittest.TestCase):
    """Test MessageBuilder class."""
    
    def test_create(self):
        """Test create method."""
        builder = MessageBuilder.create("TestMessage", "1.0")
        self.assertIsInstance(builder, MessageBuilder)
        self.assertEqual(builder.name, "TestMessage")
        self.assertEqual(builder.version, "1.0")
    
    def test_with_payload(self):
        """Test with_payload method."""
        builder = MessageBuilder.create("TestMessage", "1.0")
        payload = {"data": "test"}
        
        result = builder.with_payload(payload)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder.payload, payload)
    
    def test_with_config(self):
        """Test with_config method."""
        builder = MessageBuilder.create("TestMessage", "1.0")
        config = Mock()
        
        result = builder.with_config(config)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder.config_service, config)
    
    def test_with_correlation_id(self):
        """Test with_correlation_id method."""
        builder = MessageBuilder.create("TestMessage", "1.0")
        
        result = builder.with_correlation_id("test-123")
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder.correlation_id, "test-123")
    
    def test_build_requires_config(self):
        """Test build method requires config."""
        builder = MessageBuilder.create("TestMessage", "1.0") \
            .with_payload({"data": "test"})
        
        with self.assertRaises(ValueError):
            builder.build()
    
    def test_build_with_config(self):
        """Test build method with config."""
        config = Mock()
        config.get_thing_name.return_value = "test-thing"
        config.get_tag_config.return_value = Mock()
        config.get_tag_config.return_value.to_dict.return_value = {}
        
        builder = MessageBuilder.create("TestMessage", "1.0") \
            .with_payload({"data": "test"}) \
            .with_config(config) \
            .with_correlation_id("test-123")
        
        result = builder.build()
        
        self.assertIsNotNone(result)
        self.assertEqual(result.get_header().name, "TestMessage")
        self.assertEqual(result.get_header().version, "1.0")
        self.assertEqual(result.get_header().correlation_id, "test-123")
        self.assertEqual(result.get_body(), {"data": "test"})


class TestMetricBuilder(unittest.TestCase):
    """Test MetricBuilder class."""
    
    def test_create(self):
        """Test create method."""
        builder = MetricBuilder.create("test_metric")
        self.assertIsInstance(builder, MetricBuilder)
        self.assertEqual(builder._name, "test_metric")
    
    def test_with_namespace(self):
        """Test with_namespace method."""
        builder = MetricBuilder.create("test_metric")
        
        result = builder.with_namespace("TestApp/Metrics")
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder._namespace, "TestApp/Metrics")
    
    def test_add_measure(self):
        """Test add_measure method."""
        builder = MetricBuilder.create("test_metric")
        
        result = builder.add_measure("count", "Count", 1.0)
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(len(builder._measures), 1)
        measure = builder._measures[0]
        self.assertEqual(measure.name, "count")
        self.assertEqual(measure.unit, "Count")
        self.assertEqual(measure.value, 1.0)
    
    def test_add_dimension(self):
        """Test add_dimension method."""
        builder = MetricBuilder.create("test_metric")
        
        result = builder.add_dimension("instance", "main")
        
        self.assertEqual(result, builder)  # Fluent interface
        self.assertEqual(builder._dimensions["instance"], "main")
    
    @patch('ggcommons.metrics.metric_builder.Metric')
    def test_build(self, mock_metric_class):
        """Test build method."""
        mock_metric = Mock()
        mock_metric_class.return_value = mock_metric
        
        builder = MetricBuilder.create("test_metric") \
            .with_namespace("TestApp/Metrics") \
            .add_measure("count", "Count", 1.0) \
            .add_dimension("instance", "main")
        
        result = builder.build()
        
        self.assertEqual(result, mock_metric)
        # Verify Metric constructor was called
        mock_metric_class.assert_called_once()


if __name__ == '__main__':
    unittest.main()