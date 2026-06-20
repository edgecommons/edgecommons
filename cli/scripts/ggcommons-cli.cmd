@echo off
setlocal enabledelayedexpansion

:: Name of the Python script
set PYTHON_SCRIPT=ggcommons_cli.py

:: Check if the Python script exists
if not exist %PYTHON_SCRIPT% (
    echo Error: %PYTHON_SCRIPT% not found in the current directory.
    exit /b 1
)

:: Collect all arguments
set args=
:collect_args
if "%~1"=="" goto run_script
set args=!args! %1
shift
goto collect_args

:run_script
:: Invoke the Python script with all passed arguments
python %PYTHON_SCRIPT% %args%
if errorlevel 1 (
    echo Error running Python script.
    exit /b 1
)

exit /b 0
