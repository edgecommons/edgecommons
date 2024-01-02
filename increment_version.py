import re
import sys

filename = "setup.py"

with open(filename) as f:
  content = f.read()

match = re.search(r'VERSION = "([^"]+)"', content)
if match:
  version = match.group(1)
  parts = version.split(".")

  last = parts[-1]
  last = str(int(last) + 1)

  new_version = ".".join(parts[:-1] + [last])

  content = content.replace(f'VERSION = "{version}"', f'VERSION = "{new_version}"')

with open(filename, "w") as f:
  f.write(content)
