"""
Pytest configuration and shared fixtures.
"""
import pytest
import sys
import os

# Add parent directory to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))

# Skip tests if AWS SDK dependencies not available
pytest_plugins = []

def pytest_configure(config):
    """Configure pytest with custom markers."""
    config.addinivalue_line("markers", "integration: integration tests")
    config.addinivalue_line("markers", "slow: slow running tests") 
    config.addinivalue_line("markers", "aws: tests requiring AWS SDK")

def pytest_collection_modifyitems(config, items):
    """Modify test collection to add markers automatically."""
    for item in items:
        # Mark integration tests
        if "integration" in item.nodeid:
            item.add_marker(pytest.mark.integration)
        
        # Mark AWS-dependent tests
        if any(keyword in item.nodeid for keyword in ["edgecommons", "messaging", "iot"]):
            item.add_marker(pytest.mark.aws)

@pytest.fixture(scope="session")
def aws_available():
    """Check if AWS SDK is available."""
    try:
        import awsiot.greengrasscoreipc
        return True
    except ImportError:
        return False