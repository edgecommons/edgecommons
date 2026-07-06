#!/bin/sh

# Name of the Python script
PYTHON_SCRIPT="edgecommons_cli.py"

# Check if the Python script exists
if [ ! -f "$PYTHON_SCRIPT" ]; then
    echo "Error: $PYTHON_SCRIPT not found in the current directory."
    exit 1
fi

# Invoke the Python script with all passed arguments
python3 "$PYTHON_SCRIPT" "$@"
