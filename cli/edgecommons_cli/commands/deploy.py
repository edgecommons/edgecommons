import json
import os
import shutil
import subprocess
import tempfile
from typing import Dict, Any, Tuple

from edgecommons_cli import CommandBase


class Deploy(CommandBase):
    """Build and (optionally) publish a component with the GDK, and optionally create a
    cloud deployment to a target thing or thing group.

    Prerequisites: `gdk` on PATH (and, for publish/deploy, AWS credentials configured).
    Local on-device deployment is done with `greengrass-cli` on the core itself and is
    out of scope for this command.
    """

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "deploy",
            "description": "Build/publish a component with the GDK and optionally deploy it to a target",
            "parameters": [
                {
                    "name": "path",
                    "short": "p",
                    "description": "Path to the component project",
                    "type": "string",
                    "required": False,
                    "default": "."
                },
                {
                    "name": "publish",
                    "description": "Publish the component to the cloud after building",
                    "type": "boolean",
                    "required": False
                },
                {
                    "name": "target",
                    "short": "t",
                    "description": "Deployment target ARN (thing or thing group). Implies "
                                   "--publish and creates a cloud deployment.",
                    "type": "string",
                    "required": False
                },
                {
                    "name": "region",
                    "short": "r",
                    "description": "AWS region for the deployment",
                    "type": "string",
                    "required": False,
                    "default": "us-east-1"
                }
            ]
        }

    def execute_command(self, args: Dict[str, Any]):
        path = args.get("path", ".")
        target = args.get("target")
        do_publish = bool(args.get("publish")) or bool(target)

        if not os.path.isdir(path):
            raise FileNotFoundError(f"Project directory not found: {path}")
        gdk = shutil.which("gdk")
        if not gdk:
            raise RuntimeError("gdk not found on PATH. Install the Greengrass Development Kit "
                               "(see `edgecommons doctor`).")

        self._run([gdk, "component", "build"], path)
        if do_publish:
            self._run([gdk, "component", "publish"], path)

        if target:
            self._create_deployment(path, target, args.get("region"))
        elif not do_publish:
            print("Built only. Pass --publish to publish, or --target <arn> to publish + deploy.")

    def _run(self, cmd, cwd):
        print(f"$ {' '.join(cmd)}  (in {cwd})")
        try:
            subprocess.run(cmd, cwd=cwd, check=True)
        except subprocess.CalledProcessError as e:
            raise RuntimeError(f"command failed ({e.returncode}): {' '.join(cmd)}") from e

    def _read_name_version(self, path: str) -> Tuple[str, str]:
        cfg_path = os.path.join(path, "gdk-config.json")
        if not os.path.isfile(cfg_path):
            raise FileNotFoundError(f"gdk-config.json not found in {path}")
        with open(cfg_path, 'r', encoding='utf-8') as fh:
            cfg = json.load(fh)
        component = cfg["component"]
        name = next(iter(component))
        version = component[name].get("version", "")
        if version == "NEXT_PATCH" or not version:
            raise RuntimeError(
                "gdk-config.json uses version 'NEXT_PATCH'; set a concrete version before "
                "deploying with --target (so the exact version to deploy is unambiguous)."
            )
        return name, version

    def _create_deployment(self, path: str, target: str, region: str):
        aws = shutil.which("aws")
        if not aws:
            raise RuntimeError("aws CLI not found on PATH (needed to create a deployment).")
        name, version = self._read_name_version(path)
        deployment_input = {
            "targetArn": target,
            "deploymentName": f"{name}-{version}",
            "components": {name: {"componentVersion": version}},
        }
        tmp = None
        try:
            fd, tmp = tempfile.mkstemp(suffix=".json")
            with os.fdopen(fd, 'w', encoding='utf-8') as fh:
                json.dump(deployment_input, fh)
            print(f"Creating deployment of {name}:{version} -> {target}")
            self._run([aws, "greengrassv2", "create-deployment",
                       "--region", region, "--cli-input-json", f"file://{tmp}"], path)
        finally:
            if tmp and os.path.exists(tmp):
                os.remove(tmp)
