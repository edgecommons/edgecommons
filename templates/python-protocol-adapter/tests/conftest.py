"""Put the component root on `sys.path` so the adapter package imports resolve from anywhere
(`pyproject.toml`'s `pythonpath = ["."]` does the same under pytest; this keeps a bare
`python -m pytest` working even without it)."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
