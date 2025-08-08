"""
Unit tests for utility classes.
"""

import unittest
from unittest.mock import Mock, patch, mock_open
import tempfile
import os
import time
import threading

try:
    from ggcommons.utils.file_watcher import FileWatcher
    from ggcommons.utils.utils import Utils
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestFileWatcher(unittest.TestCase):
    """Test FileWatcher class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.temp_file = None
        self.callback_called = False
        self.callback_path = None
    
    def tearDown(self):
        """Clean up after tests."""
        if self.temp_file and os.path.exists(self.temp_file):
            os.unlink(self.temp_file)
    
    def callback(self, file_path):
        """Test callback function."""
        self.callback_called = True
        self.callback_path = file_path
    
    def test_init(self):
        """Test initialization."""
        watcher = FileWatcher("test.txt", self.callback)
        self.assertEqual(watcher._file_path, "test.txt")
        self.assertEqual(watcher._callback, self.callback)
        self.assertFalse(watcher._running)
        self.assertIsNone(watcher._thread)
    
    def test_start_stop(self):
        """Test start and stop methods."""
        # Create temporary file
        with tempfile.NamedTemporaryFile(delete=False) as f:
            self.temp_file = f.name
            f.write(b"initial content")
        
        watcher = FileWatcher(self.temp_file, self.callback)
        
        # Start watching
        watcher.start()
        self.assertTrue(watcher._running)
        self.assertIsNotNone(watcher._thread)
        
        # Stop watching
        watcher.stop()
        self.assertFalse(watcher._running)
        
        # Wait for thread to finish
        if watcher._thread:
            watcher._thread.join(timeout=1)
    
    def test_file_change_detection(self):
        """Test file change detection."""
        # Create temporary file
        with tempfile.NamedTemporaryFile(delete=False) as f:
            self.temp_file = f.name
            f.write(b"initial content")
        
        watcher = FileWatcher(self.temp_file, self.callback, check_interval=0.1)
        watcher.start()
        
        try:
            # Modify file
            time.sleep(0.2)  # Wait for initial check
            with open(self.temp_file, 'w') as f:
                f.write("modified content")
            
            # Wait for change detection
            time.sleep(0.3)
            
            # Verify callback was called
            self.assertTrue(self.callback_called)
            self.assertEqual(self.callback_path, self.temp_file)
            
        finally:
            watcher.stop()
    
    def test_nonexistent_file(self):
        """Test watching non-existent file."""
        watcher = FileWatcher("nonexistent.txt", self.callback)
        
        # Should not raise exception
        watcher.start()
        time.sleep(0.1)
        watcher.stop()


class TestUtils(unittest.TestCase):
    """Test Utils class."""
    
    def test_get_component_name_from_path(self):
        """Test get_component_name_from_path method."""
        # Test with typical component path
        path = "/greengrass/v2/packages/artifacts/com.example.MyComponent/1.0.0/main.py"
        result = Utils.get_component_name_from_path(path)
        self.assertEqual(result, "com.example.MyComponent")
        
        # Test with Windows path
        path = "C:\\greengrass\\v2\\packages\\artifacts\\com.example.MyComponent\\1.0.0\\main.py"
        result = Utils.get_component_name_from_path(path)
        self.assertEqual(result, "com.example.MyComponent")
    
    def test_get_component_name_from_path_invalid(self):
        """Test get_component_name_from_path with invalid path."""
        path = "/some/other/path/main.py"
        result = Utils.get_component_name_from_path(path)
        self.assertIsNone(result)
    
    def test_parse_file_size(self):
        """Test parse_file_size method."""
        # Test bytes
        self.assertEqual(Utils.parse_file_size("1024"), 1024)
        self.assertEqual(Utils.parse_file_size("1024B"), 1024)
        
        # Test kilobytes
        self.assertEqual(Utils.parse_file_size("1KB"), 1024)
        self.assertEqual(Utils.parse_file_size("2KB"), 2048)
        
        # Test megabytes
        self.assertEqual(Utils.parse_file_size("1MB"), 1024 * 1024)
        self.assertEqual(Utils.parse_file_size("5MB"), 5 * 1024 * 1024)
        
        # Test gigabytes
        self.assertEqual(Utils.parse_file_size("1GB"), 1024 * 1024 * 1024)
        
        # Test terabytes
        self.assertEqual(Utils.parse_file_size("1TB"), 1024 * 1024 * 1024 * 1024)
    
    def test_parse_file_size_invalid(self):
        """Test parse_file_size with invalid input."""
        with self.assertRaises(ValueError):
            Utils.parse_file_size("invalid")
        
        with self.assertRaises(ValueError):
            Utils.parse_file_size("1XB")
        
        with self.assertRaises(ValueError):
            Utils.parse_file_size("")
    
    def test_safe_get_nested_value(self):
        """Test safe_get_nested_value method."""
        data = {
            "level1": {
                "level2": {
                    "value": "found"
                },
                "list": [1, 2, 3]
            }
        }
        
        # Test successful nested access
        result = Utils.safe_get_nested_value(data, "level1.level2.value")
        self.assertEqual(result, "found")
        
        # Test with default value
        result = Utils.safe_get_nested_value(data, "level1.nonexistent", "default")
        self.assertEqual(result, "default")
        
        # Test with None data
        result = Utils.safe_get_nested_value(None, "level1.level2.value", "default")
        self.assertEqual(result, "default")
        
        # Test with empty path
        result = Utils.safe_get_nested_value(data, "", "default")
        self.assertEqual(result, "default")
    
    def test_merge_dictionaries(self):
        """Test merge_dictionaries method."""
        dict1 = {
            "a": 1,
            "b": {
                "c": 2,
                "d": 3
            }
        }
        
        dict2 = {
            "b": {
                "d": 4,
                "e": 5
            },
            "f": 6
        }
        
        result = Utils.merge_dictionaries(dict1, dict2)
        
        expected = {
            "a": 1,
            "b": {
                "c": 2,
                "d": 4,  # Overridden
                "e": 5   # Added
            },
            "f": 6
        }
        
        self.assertEqual(result, expected)
    
    def test_merge_dictionaries_none_inputs(self):
        """Test merge_dictionaries with None inputs."""
        dict1 = {"a": 1}
        
        result = Utils.merge_dictionaries(dict1, None)
        self.assertEqual(result, dict1)
        
        result = Utils.merge_dictionaries(None, dict1)
        self.assertEqual(result, dict1)
        
        result = Utils.merge_dictionaries(None, None)
        self.assertEqual(result, {})
    
    def test_validate_required_fields(self):
        """Test validate_required_fields method."""
        data = {
            "field1": "value1",
            "field2": "value2",
            "field3": None
        }
        
        # Test with all required fields present
        Utils.validate_required_fields(data, ["field1", "field2"])
        
        # Test with missing field
        with self.assertRaises(ValueError) as context:
            Utils.validate_required_fields(data, ["field1", "missing_field"])
        self.assertIn("missing_field", str(context.exception))
        
        # Test with None value
        with self.assertRaises(ValueError) as context:
            Utils.validate_required_fields(data, ["field1", "field3"])
        self.assertIn("field3", str(context.exception))
        
        # Test with None data
        with self.assertRaises(ValueError):
            Utils.validate_required_fields(None, ["field1"])


if __name__ == '__main__':
    unittest.main()