"""
Complete test suite runner for GGCommons.

This module provides a comprehensive test suite that runs all unit tests
and integration tests for the GGCommons Python library.
"""

import unittest
import sys
import os

# Add the parent directory to the path so we can import ggcommons
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))

# Always import basic tests
from test_basic import TestBasicFunctionality

# Import all test modules with error handling
try:
    from test_messaging_config import TestCredentialsConfig, TestLocalMqttConfig, TestIoTCoreConfig, TestMessagingConfiguration
    from test_messaging_client import TestMessagingClient
    from test_builders import TestGGCommonsBuilder, TestMessageBuilder, TestMetricBuilder
    from test_dependency_injection import TestServiceRegistry, TestServiceFactory
    from test_configuration import TestConfigurationValidator, TestConfigurationValidationException
    from test_metrics import TestMeasure, TestMetric, TestMetricService
    from test_utils import TestFileWatcher, TestUtils
    from test_integration import TestGGCommonsIntegration
    TESTS_AVAILABLE = True
except (ImportError, unittest.SkipTest) as e:
    print(f"Warning: Some tests skipped due to missing dependencies: {e}")
    TESTS_AVAILABLE = False


def create_test_suite():
    """Create comprehensive test suite."""
    suite = unittest.TestSuite()
    loader = unittest.TestLoader()
    
    # Always include basic tests
    suite.addTest(loader.loadTestsFromTestCase(TestBasicFunctionality))
    
    if not TESTS_AVAILABLE:
        return suite
    
    # Messaging Configuration Tests
    suite.addTest(loader.loadTestsFromTestCase(TestCredentialsConfig))
    suite.addTest(loader.loadTestsFromTestCase(TestLocalMqttConfig))
    suite.addTest(loader.loadTestsFromTestCase(TestIoTCoreConfig))
    suite.addTest(loader.loadTestsFromTestCase(TestMessagingConfiguration))
    
    # Messaging Client Tests
    suite.addTest(loader.loadTestsFromTestCase(TestMessagingClient))
    
    # Builder Tests
    suite.addTest(loader.loadTestsFromTestCase(TestGGCommonsBuilder))
    suite.addTest(loader.loadTestsFromTestCase(TestMessageBuilder))
    suite.addTest(loader.loadTestsFromTestCase(TestMetricBuilder))
    
    # Dependency Injection Tests
    suite.addTest(loader.loadTestsFromTestCase(TestServiceRegistry))
    suite.addTest(loader.loadTestsFromTestCase(TestServiceFactory))
    
    # Configuration Tests
    suite.addTest(loader.loadTestsFromTestCase(TestConfigurationValidator))
    suite.addTest(loader.loadTestsFromTestCase(TestConfigurationValidationException))
    
    # Metrics Tests
    suite.addTest(loader.loadTestsFromTestCase(TestMeasure))
    suite.addTest(loader.loadTestsFromTestCase(TestMetric))
    suite.addTest(loader.loadTestsFromTestCase(TestMetricService))
    
    # Utility Tests
    suite.addTest(loader.loadTestsFromTestCase(TestFileWatcher))
    suite.addTest(loader.loadTestsFromTestCase(TestUtils))
    
    # Integration Tests
    suite.addTest(loader.loadTestsFromTestCase(TestGGCommonsIntegration))
    
    return suite


def run_tests(verbosity=2):
    """Run all tests with specified verbosity."""
    suite = create_test_suite()
    runner = unittest.TextTestRunner(verbosity=verbosity)
    result = runner.run(suite)
    
    # Print summary
    print(f"\n{'='*60}")
    print("TEST SUMMARY")
    print(f"{'='*60}")
    print(f"Tests run: {result.testsRun}")
    print(f"Failures: {len(result.failures)}")
    print(f"Errors: {len(result.errors)}")
    print(f"Skipped: {len(result.skipped) if hasattr(result, 'skipped') else 0}")
    
    if result.failures:
        print(f"\nFAILURES ({len(result.failures)}):")
        for test, traceback in result.failures:
            print(f"  - {test}")
    
    if result.errors:
        print(f"\nERRORS ({len(result.errors)}):")
        for test, traceback in result.errors:
            print(f"  - {test}")
    
    success = len(result.failures) == 0 and len(result.errors) == 0
    print(f"\nResult: {'PASSED' if success else 'FAILED'}")
    print(f"{'='*60}")
    
    return success


def run_specific_test_module(module_name, verbosity=2):
    """Run tests from a specific module."""
    try:
        module = __import__(module_name)
        suite = unittest.TestLoader().loadTestsFromModule(module)
        runner = unittest.TextTestRunner(verbosity=verbosity)
        result = runner.run(suite)
        return len(result.failures) == 0 and len(result.errors) == 0
    except ImportError as e:
        print(f"Error importing test module '{module_name}': {e}")
        return False


def main():
    """Main entry point for test runner."""
    import argparse
    
    parser = argparse.ArgumentParser(description='GGCommons Test Suite Runner')
    parser.add_argument('--module', '-m', help='Run tests from specific module only')
    parser.add_argument('--verbose', '-v', action='store_true', help='Verbose output')
    parser.add_argument('--quiet', '-q', action='store_true', help='Quiet output')
    
    args = parser.parse_args()
    
    # Determine verbosity
    verbosity = 2  # Default
    if args.verbose:
        verbosity = 3
    elif args.quiet:
        verbosity = 1
    
    # Run tests
    if args.module:
        success = run_specific_test_module(args.module, verbosity)
    else:
        success = run_tests(verbosity)
    
    # Exit with appropriate code
    sys.exit(0 if success else 1)


if __name__ == '__main__':
    main()