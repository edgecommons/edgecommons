"""
Pytest configuration and shared fixtures.
"""
import pytest
import sys
import os

# Add parent directory to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))

pytest_plugins = []


def pytest_configure(config):
    """Configure pytest with custom markers."""
    config.addinivalue_line("markers", "integration: integration tests")
    config.addinivalue_line("markers", "slow: slow running tests")
    config.addinivalue_line(
        "markers",
        "aws: needs a real AWS SDK call (credentials / network). Apply it EXPLICITLY, to the "
        "test. The coverage gate runs -m 'not aws', so anything marked here is invisible to it.",
    )


def pytest_collection_modifyitems(config, items):
    """Auto-mark integration tests by nodeid.

    `aws` is deliberately NOT auto-applied.

    It used to be, by substring — any test whose nodeid contained "edgecommons", "messaging" or
    "iot" was marked as "requiring AWS SDK". That is a claim about a test's *filename*, not about
    what it does. It branded 123 pure-mock tests as AWS-dependent, and since the coverage gate runs
    `-m "not aws"`, CI collected them, threw them away, and then measured coverage without them.
    `messaging_client.py` read 62% covered while its tests sat in the discard pile: the lines WERE
    exercised, by a file the gate refused to count.

    Those 123 tests pass in 1.7s with AWS credentials forcibly absent. They never needed the marker.

    So: mark `aws` explicitly, on the test, when the test genuinely reaches AWS. A marker that is
    inferred from a name will always drift from the truth, because the name is not the fact.
    """
    for item in items:
        if "integration" in item.nodeid:
            item.add_marker(pytest.mark.integration)


@pytest.fixture(scope="session")
def aws_available():
    """Whether the Greengrass IPC SDK is importable (for tests that genuinely need it)."""
    try:
        import awsiot.greengrasscoreipc  # noqa: F401
        return True
    except ImportError:
        return False
