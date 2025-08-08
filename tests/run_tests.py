#!/usr/bin/env python3
"""
Simple test runner for GGCommons that works without AWS SDK dependencies.
"""

import unittest
import sys
import os

# Add parent directory to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))

def run_basic_tests():
    """Run basic tests that don't require AWS SDK."""
    from test_basic import TestBasicFunctionality
    
    suite = unittest.TestSuite()
    loader = unittest.TestLoader()
    suite.addTest(loader.loadTestsFromTestCase(TestBasicFunctionality))
    
    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)
    
    return len(result.failures) == 0 and len(result.errors) == 0

def run_java_compatibility_tests():
    """Run Java compatibility tests from moved files."""
    test_files = [
        'test_java_compatible.py',
        'test_config_direct.py'
    ]
    
    success = True
    for test_file in test_files:
        if os.path.exists(test_file):
            print(f"\n{'='*60}")
            print(f"Running {test_file}")
            print(f"{'='*60}")
            
            try:
                # Import and run the test
                module_name = test_file[:-3]  # Remove .py extension
                spec = __import__(module_name)
                
                if hasattr(spec, 'main'):
                    result = spec.main()
                    if not result:
                        success = False
                elif hasattr(spec, 'test_java_compatible_config'):
                    result = spec.test_java_compatible_config()
                    if not result:
                        success = False
                elif hasattr(spec, 'test_messaging_config'):
                    result = spec.test_messaging_config()
                    if not result:
                        success = False
                else:
                    print(f"No main function found in {test_file}")
                    
            except Exception as e:
                print(f"Error running {test_file}: {e}")
                success = False
    
    return success

def main():
    """Main test runner."""
    print("GGCommons Python Test Runner")
    print("="*60)
    
    # Run basic tests
    print("\n1. Running Basic Tests (no AWS SDK required)")
    print("-" * 40)
    basic_success = run_basic_tests()
    
    # Run Java compatibility tests
    print("\n2. Running Java Compatibility Tests")
    print("-" * 40)
    java_success = run_java_compatibility_tests()
    
    # Summary
    print(f"\n{'='*60}")
    print("TEST SUMMARY")
    print(f"{'='*60}")
    print(f"Basic Tests: {'PASSED' if basic_success else 'FAILED'}")
    print(f"Java Compatibility Tests: {'PASSED' if java_success else 'FAILED'}")
    
    overall_success = basic_success and java_success
    print(f"\nOverall Result: {'PASSED' if overall_success else 'FAILED'}")
    
    return 0 if overall_success else 1

if __name__ == '__main__':
    sys.exit(main())