import shutil
from typing import Dict, Any

from edgecommons_cli import CommandBase

# (display name, [executable alternatives], why it's needed)
CHECKS = [
    ("git", ["git"], "clone component templates"),
    ("gdk", ["gdk"], "build/publish components (Greengrass Development Kit)"),
    ("cargo", ["cargo"], "build Rust components"),
    ("mvn", ["mvn"], "build Java components"),
    ("python3", ["python3", "python"], "run/build Python components"),
    ("node", ["node"], "run/build TypeScript components"),
    ("npm", ["npm"], "install TypeScript dependencies"),
    ("aws", ["aws"], "publish to AWS / deploy"),
]


class Doctor(CommandBase):
    """Check that the tools needed to build and publish components are installed."""

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "doctor",
            "description": "Check for the external tools needed to build/publish components",
            "parameters": []
        }

    def execute_command(self, args: Dict[str, Any]):
        print("Checking prerequisites:\n")
        missing = []
        for name, executables, why in CHECKS:
            path = next((shutil.which(exe) for exe in executables if shutil.which(exe)), None)
            if path:
                print(f"  [ok]      {name:<8} -> {path}")
            else:
                print(f"  [missing] {name:<8} -- needed to {why}")
                missing.append(name)
        print()
        if missing:
            print(f"Missing: {', '.join(missing)}. Install the ones for your target language/flow.")
        else:
            print("All prerequisites found.")
