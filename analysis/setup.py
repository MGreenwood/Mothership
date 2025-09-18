#!/usr/bin/env python3
"""
Setup script for Mothership analysis environment.
Installs required tools and dependencies for comprehensive analysis.
"""

import os
import sys
import subprocess
import json
from pathlib import Path

def run_command(command, description):
    """Run a command and handle errors."""
    print(f"Running: {description}")
    try:
        result = subprocess.run(command, shell=True, check=True, capture_output=True, text=True)
        print(f"✓ {description} completed successfully")
        return result.stdout
    except subprocess.CalledProcessError as e:
        print(f"✗ {description} failed: {e}")
        print(f"Error output: {e.stderr}")
        return None

def check_python_version():
    """Check if Python version is compatible."""
    if sys.version_info < (3, 8):
        print("Error: Python 3.8 or higher is required")
        sys.exit(1)
    print(f"✓ Python {sys.version_info.major}.{sys.version_info.minor} detected")

def install_python_dependencies():
    """Install Python dependencies."""
    print("\n=== Installing Python Dependencies ===")
    
    # Upgrade pip first
    run_command(f"{sys.executable} -m pip install --upgrade pip", "Upgrading pip")
    
    # Install requirements
    requirements_file = Path(__file__).parent / "requirements.txt"
    if requirements_file.exists():
        run_command(f"{sys.executable} -m pip install -r {requirements_file}", "Installing Python packages")
    else:
        print("Warning: requirements.txt not found")

def check_rust_toolchain():
    """Check if Rust toolchain is available."""
    print("\n=== Checking Rust Toolchain ===")
    
    rust_version = run_command("rustc --version", "Checking Rust compiler")
    if rust_version:
        print(f"✓ Rust toolchain available: {rust_version.strip()}")
        
        cargo_version = run_command("cargo --version", "Checking Cargo")
        if cargo_version:
            print(f"✓ Cargo available: {cargo_version.strip()}")
    else:
        print("Warning: Rust toolchain not found. Some code analysis features may be limited.")
        print("Install Rust from: https://rustup.rs/")

def check_docker():
    """Check if Docker is available."""
    print("\n=== Checking Docker ===")
    
    docker_version = run_command("docker --version", "Checking Docker")
    if docker_version:
        print(f"✓ Docker available: {docker_version.strip()}")
        
        compose_version = run_command("docker compose version", "Checking Docker Compose")
        if compose_version:
            print(f"✓ Docker Compose available: {compose_version.strip()}")
    else:
        print("Warning: Docker not found. Deployment testing will be limited.")

def check_powershell():
    """Check if PowerShell is available."""
    print("\n=== Checking PowerShell ===")
    
    # Try PowerShell Core first
    ps_version = run_command("pwsh --version", "Checking PowerShell Core")
    if not ps_version:
        # Try Windows PowerShell
        ps_version = run_command("powershell -Command '$PSVersionTable.PSVersion'", "Checking Windows PowerShell")
    
    if ps_version:
        print(f"✓ PowerShell available")
    else:
        print("Warning: PowerShell not found. Integration tests may be limited.")

def create_analysis_directories():
    """Create necessary analysis directories."""
    print("\n=== Creating Analysis Directories ===")
    
    base_dir = Path(__file__).parent
    directories = [
        "evidence/code_analysis",
        "evidence/functional_tests", 
        "evidence/integration_tests",
        "evidence/screenshots",
        "evidence/logs",
        "evidence/artifacts",
        "logs",
        "reports"
    ]
    
    for directory in directories:
        dir_path = base_dir / directory
        dir_path.mkdir(parents=True, exist_ok=True)
        print(f"✓ Created directory: {directory}")

def validate_configuration():
    """Validate configuration files."""
    print("\n=== Validating Configuration ===")
    
    config_dir = Path(__file__).parent / "config"
    config_files = [
        "analysis_config.json",
        "logging_config.json"
    ]
    
    for config_file in config_files:
        config_path = config_dir / config_file
        if config_path.exists():
            try:
                with open(config_path, 'r') as f:
                    json.load(f)
                print(f"✓ Configuration file valid: {config_file}")
            except json.JSONDecodeError as e:
                print(f"✗ Configuration file invalid: {config_file} - {e}")
        else:
            print(f"✗ Configuration file missing: {config_file}")

def setup_git_hooks():
    """Set up git hooks for analysis tracking."""
    print("\n=== Setting up Git Hooks ===")
    
    # Check if we're in a git repository
    git_dir = Path(__file__).parent.parent / ".git"
    if git_dir.exists():
        print("✓ Git repository detected")
        # Could add pre-commit hooks here if needed
    else:
        print("Warning: Not in a git repository")

def main():
    """Main setup function."""
    print("Mothership Analysis Environment Setup")
    print("=" * 40)
    
    # Check prerequisites
    check_python_version()
    
    # Install dependencies
    install_python_dependencies()
    
    # Check external tools
    check_rust_toolchain()
    check_docker()
    check_powershell()
    
    # Set up environment
    create_analysis_directories()
    validate_configuration()
    setup_git_hooks()
    
    print("\n" + "=" * 40)
    print("Setup completed!")
    print("\nNext steps:")
    print("1. Review configuration files in config/")
    print("2. Run analysis tools from the tools/ directory")
    print("3. Check logs/ directory for execution logs")
    print("4. View results in reports/ directory")
    
    print("\nTo start analysis, run:")
    print("  cd tools/")
    print("  python code_analyzer.py")

if __name__ == "__main__":
    main()