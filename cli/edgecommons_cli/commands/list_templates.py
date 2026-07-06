from typing import Dict, Any

from edgecommons_cli import CommandBase
from edgecommons_cli.commands.create_component import DEFAULT_TEMPLATE_SOURCES


class ListTemplates(CommandBase):
    """List the component templates (languages) the CLI can generate."""

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "list-templates",
            "description": "List the available component templates and their sources",
            "parameters": []
        }

    def execute_command(self, args: Dict[str, Any]):
        print("Available templates (override any source with --template-url):\n")
        width = max(len(k) for k in DEFAULT_TEMPLATE_SOURCES)
        for language, source in sorted(DEFAULT_TEMPLATE_SOURCES.items()):
            print(f"  {language.ljust(width)}  {source}")
