#!/usr/bin/env python3
"""
Simple pytest runner for EdgeCommons tests.
"""

import sys
import subprocess
import argparse


def main():
    """Run pytest with appropriate options."""
    parser = argparse.ArgumentParser(description='EdgeCommons Pytest Runner')
    parser.add_argument('--verbose', '-v', action='store_true', help='Verbose output')
    parser.add_argument('--quiet', '-q', action='store_true', help='Quiet output')
    parser.add_argument('--coverage', '-c', action='store_true', help='Run with coverage')
    parser.add_argument('--markers', '-m', help='Run tests with specific markers')
    parser.add_argument('--file', '-f', help='Run specific test file')
    parser.add_argument('--function', help='Run specific test function')
    
    args = parser.parse_args()
    
    # Build pytest command
    cmd = ['python', '-m', 'pytest']
    
    # Add verbosity
    if args.verbose:
        cmd.append('-v')
    elif args.quiet:
        cmd.append('-q')
    else:
        cmd.append('-v')  # Default to verbose
    
    # Add coverage
    if args.coverage:
        cmd.extend(['--cov=edgecommons', '--cov-report=html', '--cov-report=term'])
    
    # Add markers
    if args.markers:
        cmd.extend(['-m', args.markers])
    
    # Add specific file or function
    if args.file:
        if args.function:
            cmd.append(f"{args.file}::{args.function}")
        else:
            cmd.append(args.file)
    elif args.function:
        cmd.extend(['-k', args.function])
    
    # Add test directory
    if not args.file:
        cmd.append('tests/')
    
    print(f"Running: {' '.join(cmd)}")
    
    # Run pytest
    result = subprocess.run(cmd, cwd='.')
    sys.exit(result.returncode)


if __name__ == '__main__':
    main()