# GGCommons CLI: A Command-Line Interface for GGCommons

GGCommons CLI is a command-line tool for interacting with the GGCommons library. It provides a convenient way to create and manage components within the GGCommons ecosystem.

This project aims to simplify the process of working with GGCommons by providing an easy-to-use command-line interface. It allows developers to quickly create and manipulate components without the need for direct interaction with the underlying library code.

## Repository Structure

```
.
├── pyproject.toml                  # packaging + console entry points (ggcommons / ggcommons-cli)
├── ggcommons_cli/                  # the package
│   ├── cli.py                      # framework: arg parsing + command discovery
│   ├── recipe_lint.py              # shared Greengrass-recipe linting
│   └── commands/                   # auto-discovered commands (one class per file)
│       ├── create_component.py
│       ├── validate.py
│       ├── list_templates.py
│       └── doctor.py
└── scripts/                        # legacy wrapper scripts (optional once installed)
```

Commands are auto-discovered: any `commands/*.py` exposing a class with
`execute_command` + `get_json_configuration` becomes a subcommand. Templates are
**manifest-driven** — a template repo ships a `ggcommons-template.json` declaring the
placeholder substitutions / file renames, so adding a new language needs a template,
not CLI code.

## Installation

Requires Python 3.8+. Install as a tool (recommended, gives a global `ggcommons` command):

```
pipx install .          # or: python -m pip install .
```

For development, an editable install:

```
python -m pip install -e .
```

## Usage

```
ggcommons --help
ggcommons --version
```

### Commands

- **`create-component`** — scaffold a new component:
  ```
  ggcommons create-component -n com.example.MyComponent -l RUST
  ggcommons create-component -n com.example.MyComponent -l JAVA -j my-component
  ```
  Useful flags: `-l/--language {JAVA,PYTHON,RUST}`, `-p/--path <dir>`,
  `-a/--author`, `-b/--bucket`, `-r/--region`, `-g/--ggcommons-path` (Rust path dep),
  `-u/--template-url` (override the template source — a git URL **or** a local dir),
  `-f/--force` (overwrite an existing target).
- **`list-templates`** — show the available languages and their template sources.
- **`validate`** — check a generated component's recipe for issues that break
  `gdk component publish` (e.g. `{COMPONENT_NAME}`, an artifact `Permissions:` block,
  leftover `<<...>>` placeholders): `ggcommons validate -p ./MyComponent`.
- **`doctor`** — check for the external tools needed to build/publish (git, gdk,
  cargo, mvn, python3, aws).

### Configuration

The CLI does not require any specific configuration. However, ensure that you have the necessary permissions to create and modify files in the directory where you're running the commands.

## Data Flow

The GGCommons CLI follows a simple data flow:

1. User invokes the CLI script with a command and options.
2. The script calls the main `ggcommons_cli.py` file.
3. The main file parses the command and options.
4. The appropriate command module (e.g., `create_component.py`) is executed.
5. The command module interacts with the GGCommons library to perform the requested action.
6. Results or feedback are displayed to the user in the console.

```
[User Input] -> [CLI Script] -> [ggcommons_cli.py] -> [Command Module] -> [GGCommons Library]
     ^                                                                            |
     |                                                                            |
     +----------------------------------------------------------------------------+
                                 [Output/Feedback]
```

This flow ensures a separation of concerns between the CLI interface and the underlying library functionality.