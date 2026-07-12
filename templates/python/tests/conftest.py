"""Put the component root on `sys.path` so `import app.<<COMPONENTNAME>>` resolves from anywhere."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
