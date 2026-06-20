import argparse
import os
import sys
import inspect
import importlib.util
from typing import Dict, Any
import warnings
import jsonschema


class CommandBase:
    def execute_command(self, args: Dict[str, Any]):
        raise NotImplementedError("Subclasses must implement execute_command method")

    @classmethod
    def get_json_configuration(cls) -> Dict[str, Any]:
        raise NotImplementedError("Subclasses must implement get_json_configuration method")


class CLIFramework:
    def __init__(self, cli_name: str, command_dir: str):
        self.cli_name = cli_name
        self.command_dir = command_dir
        self.parser = argparse.ArgumentParser(prog=cli_name, description=f"{cli_name} CLI")
        try:
            from importlib.metadata import version, PackageNotFoundError
            try:
                _ver = version("ggcommons-cli")
            except PackageNotFoundError:
                _ver = "0.0.0+local"
        except ImportError:  # py<3.8
            _ver = "0.0.0"
        self.parser.add_argument("--version", action="version", version=f"%(prog)s {_ver}")
        self.subparsers = self.parser.add_subparsers(dest="command", help="Available commands")
        self.commands: Dict[str, CommandBase] = {}
        self.load_commands()

    def load_commands(self):
        for filename in os.listdir(self.command_dir):
            if filename.endswith(".py") and filename != "__init__.py":
                module_name = filename[:-3]  # Remove .py extension
                module_path = os.path.join(self.command_dir, filename)
                spec = importlib.util.spec_from_file_location(module_name, module_path)
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)

                members = inspect.getmembers(module)
                for name, obj in members:
                    if inspect.isclass(obj):
                        if name != 'CommandBase' and hasattr(obj, 'execute_command') and hasattr(obj, 'get_json_configuration'):
                            try:
                                self.validate_command_class(obj)
                                command_instance = obj()
                                self.add_command(command_instance)
                            except (TypeError, NotImplementedError) as e:
                                warnings.warn(f"Invalid command class '{name}' in {filename}: {str(e)}")

    def validate_command_class(self, cls):
        if not hasattr(cls, 'execute_command'):
            raise NotImplementedError(f"Class {cls.__name__} must implement execute_command method")
        if not hasattr(cls, 'get_json_configuration'):
            raise NotImplementedError(f"Class {cls.__name__} must implement get_json_configuration method")

        # Check if get_json_configuration is a classmethod
        if not isinstance(cls.__dict__.get('get_json_configuration'), classmethod):
            raise TypeError(f"get_json_configuration in {cls.__name__} must be a classmethod")

    def validate_json_configuration(self, config: Dict[str, Any]):
        schema = {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "parameters": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "description": {"type": "string"},
                            "type": {"type": "string"},
                            "required": {"type": "boolean"},
                            "short": {"type": "string"},
                            "default": {},
                            "enum": {
                                "type": "array",
                                "items": {"type": "string"}
                            }
                        },
                        "required": ["name", "description", "type"]
                    }
                }
            },
            "required": ["name", "description"]
        }
        try:
            jsonschema.validate(instance=config, schema=schema)
        except jsonschema.exceptions.ValidationError as e:
            raise ValueError(f"Invalid JSON configuration: {str(e)}")

    def add_command(self, command_instance: CommandBase):
        try:
            command_def = command_instance.get_json_configuration()
            self.validate_json_configuration(command_def)
        except (ValueError, TypeError) as e:
            warnings.warn(f"Invalid JSON configuration for command {command_instance.__class__.__name__}: {str(e)}")
            return

        name = command_def["name"]
        self.commands[name] = command_instance
        subparser = self.subparsers.add_parser(name, help=command_def["description"])

        for param in command_def.get("parameters", []):
            flags = [f"--{param['name']}"]
            if "short" in param:
                flags.insert(0, f"-{param['short']}")

            kwargs = {
                "help": param["description"],
                "required": param.get("required", False)
            }

            if param.get("type") == "boolean":
                kwargs["action"] = "store_true"
            elif "default" in param:
                kwargs["default"] = param["default"]

            if "enum" in param:
                kwargs["choices"] = param["enum"]

            subparser.add_argument(*flags, **kwargs)

    def run(self):
        args = self.parser.parse_args()
        if args.command:
            self.execute_command(args.command, vars(args))
        else:
            self.parser.print_help()

    def execute_command(self, command: str, args: Dict[str, Any]):
        if command not in self.commands:
            print(f"Unknown command: {command}", file=sys.stderr)
            sys.exit(2)
        try:
            self.commands[command].execute_command(args)
        except Exception as e:  # surface a clean error + non-zero exit, not a traceback
            print(f"error: {e}", file=sys.stderr)
            sys.exit(1)

def main():
    # Resolve the commands directory relative to this package so the CLI works when
    # installed and when run from any working directory.
    command_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "commands")
    cli = CLIFramework("ggcommons", command_dir)
    cli.run()


if __name__ == "__main__":
    main()
