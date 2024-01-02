import re
import sys

filename = "setup.py"

job_id = sys.argv[1]

with open(filename) as f:
  content = f.read()

match = re.search(r'VERSION = "([^"]+)"', content)
if match:
  version = match.group(1)
  parts = version.split(".")
  last = str(job_id)

  new_version = ".".join(parts[:-1] + [last])

  content = content.replace(f'VERSION = "{version}"', f'VERSION = "{new_version}"')

with open(filename, "w") as f:
  f.write(content)
