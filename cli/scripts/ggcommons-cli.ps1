# Name of the Python script
$PYTHON_SCRIPT = "ggcommons_cli.py"

# Check if the Python script exists
if (-not (Test-Path $PYTHON_SCRIPT)) {
    Write-Error "Error: $PYTHON_SCRIPT not found in the current directory."
    exit 1
}

# Get all arguments passed to this script
$args = $MyInvocation.UnboundArguments

# Invoke the Python script with all passed arguments
try {
    python $PYTHON_SCRIPT $args
}
catch {
    Write-Error "Error running Python script: $_"
    exit 1
}
