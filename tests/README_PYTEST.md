# Pytest Test Suite

The GGCommons test suite has been converted to pytest for better maintainability and modern testing practices.

## Running Tests

### Install Dependencies
```bash
pip install -r requirements-test.txt
```

### Basic Usage
```bash
# Run all tests
pytest

# Run with verbose output
pytest -v

# Run specific test file
pytest tests/test_basic.py

# Run specific test function
pytest tests/test_basic.py::test_messaging_config_classes_exist

# Run tests with markers
pytest -m "not slow"  # Skip slow tests
pytest -m "not aws"   # Skip AWS-dependent tests
pytest -m integration # Run only integration tests
```

### Coverage
```bash
# Run with coverage
pytest --cov=ggcommons --cov-report=html --cov-report=term
```

### Parallel Execution
```bash
# Run tests in parallel
pytest -n auto
```

## Test Organization

### Files
- `test_basic.py` - Basic functionality tests (no external dependencies)
- `test_builders_pytest.py` - Builder pattern tests
- `test_messaging_config_pytest.py` - Messaging configuration tests
- `test_ggcommons_pytest.py` - Integration tests
- `conftest.py` - Pytest configuration and fixtures

### Markers
- `@pytest.mark.integration` - Integration tests
- `@pytest.mark.slow` - Slow-running tests
- `@pytest.mark.aws` - Tests requiring AWS SDK

### Legacy Files
The original unittest files are preserved for backward compatibility:
- `test_suite.py` - Original unittest runner
- `test_*.py` - Original unittest test files

## Migration Benefits

1. **Simpler syntax** - Functions instead of classes, plain `assert` statements
2. **Better fixtures** - Shared setup/teardown with dependency injection
3. **Automatic discovery** - No manual test suite creation needed
4. **Better parametrization** - Easy data-driven testing
5. **Rich plugin ecosystem** - Coverage, parallel execution, etc.
6. **Cleaner output** - Better failure reporting

## Configuration

See `pytest.ini` for pytest configuration and `conftest.py` for fixtures and test setup.