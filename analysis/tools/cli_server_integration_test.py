#!/usr/bin/env python3
"""
CLI-Server Integration Test for Mothership Analysis
Tests the integration between CLI and server components
"""

import subprocess
import json
import os
import sys
import time
import requests
from pathlib import Path
from typing import Dict, List, Any, Optional

def run_cli_command(cli_path: str, args: List[str], timeout: int = 10, input_text: str = None) -> Dict[str, Any]:
    """Run a CLI command and capture results"""
    cmd = [cli_path] + args
    print(f"Running: {' '.join(cmd)}")
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            input=input_text,
            encoding='utf-8',
            errors='replace'
        )
        
        return {
            'command': ' '.join(cmd),
            'success': True,
            'returncode': result.returncode,
            'stdout': result.stdout,
            'stderr': result.stderr
        }
    except subprocess.TimeoutExpired:
        return {
            'command': ' '.join(cmd),
            'success': False,
            'error': 'timeout',
            'timeout': timeout
        }
    except Exception as e:
        return {
            'command': ' '.join(cmd),
            'success': False,
            'error': str(e)
        }

def check_server_running(port: int = 7523) -> Dict[str, Any]:
    """Check if a Mothership server is running"""
    try:
        response = requests.get(f"http://localhost:{port}/health", timeout=5)
        return {
            'running': True,
            'port': port,
            'status_code': response.status_code,
            'response': response.text[:200]
        }
    except requests.exceptions.ConnectionError:
        return {
            'running': False,
            'port': port,
            'error': 'Connection refused'
        }
    except Exception as e:
        return {
            'running': False,
            'port': port,
            'error': str(e)
        }

def check_daemon_running(port: int = 7525) -> Dict[str, Any]:
    """Check if the Mothership daemon is running"""
    try:
        response = requests.get(f"http://localhost:{port}/health", timeout=5)
        return {
            'running': True,
            'port': port,
            'status_code': response.status_code,
            'response': response.text[:200]
        }
    except requests.exceptions.ConnectionError:
        return {
            'running': False,
            'port': port,
            'error': 'Connection refused'
        }
    except Exception as e:
        return {
            'running': False,
            'port': port,
            'error': str(e)
        }

def test_daemon_integration(cli_path: str) -> Dict[str, Any]:
    """Test CLI-daemon integration"""
    print("Testing CLI-daemon integration...")
    
    results = {}
    
    # Test daemon status
    result = run_cli_command(cli_path, ['daemon', 'status'])
    results['daemon_status'] = result
    
    # Check if daemon is actually running via HTTP
    daemon_check = check_daemon_running()
    results['daemon_http_check'] = daemon_check
    
    # Test daemon API directly
    if daemon_check['running']:
        try:
            # Test status endpoint
            response = requests.get("http://localhost:7525/status", timeout=5)
            results['daemon_api_status'] = {
                'success': True,
                'status_code': response.status_code,
                'response': response.json() if response.headers.get('content-type', '').startswith('application/json') else response.text
            }
        except Exception as e:
            results['daemon_api_status'] = {
                'success': False,
                'error': str(e)
            }
            
        try:
            # Test projects endpoint
            response = requests.get("http://localhost:7525/projects", timeout=5)
            results['daemon_api_projects'] = {
                'success': True,
                'status_code': response.status_code,
                'response': response.json() if response.headers.get('content-type', '').startswith('application/json') else response.text
            }
        except Exception as e:
            results['daemon_api_projects'] = {
                'success': False,
                'error': str(e)
            }
    
    return results

def test_server_connection(cli_path: str) -> Dict[str, Any]:
    """Test CLI-server connection capabilities"""
    print("Testing CLI-server connection...")
    
    results = {}
    
    # Test server status
    result = run_cli_command(cli_path, ['server', 'status'])
    results['server_status'] = result
    
    # Test server list
    result = run_cli_command(cli_path, ['server', 'list'])
    results['server_list'] = result
    
    # Check if any Mothership server is running
    server_ports = [7523, 8080, 3000, 8000]  # Common ports
    for port in server_ports:
        server_check = check_server_running(port)
        results[f'server_check_{port}'] = server_check
        if server_check['running']:
            print(f"Found server running on port {port}")
            break
    
    # Test connection to invalid server (should fail gracefully)
    result = run_cli_command(cli_path, ['connect', 'http://invalid-server.test'], timeout=15)
    results['connect_invalid_server'] = result
    
    return results

def test_authentication_flow(cli_path: str) -> Dict[str, Any]:
    """Test authentication flow without actually authenticating"""
    print("Testing authentication flow...")
    
    results = {}
    
    # Test auth help
    result = run_cli_command(cli_path, ['auth', '--help'])
    results['auth_help'] = result
    
    # Test logout (should work even if not authenticated)
    result = run_cli_command(cli_path, ['logout'])
    results['logout'] = result
    
    # Test commands that require authentication
    auth_required_commands = [
        ['gateway', 'list'],
        ['status'],
        ['history']
    ]
    
    for cmd in auth_required_commands:
        result = run_cli_command(cli_path, cmd)
        results[f"auth_required_{'_'.join(cmd)}"] = result
    
    return results

def test_project_operations(cli_path: str) -> Dict[str, Any]:
    """Test project operations that don't require server"""
    print("Testing project operations...")
    
    results = {}
    
    # Test in a temporary directory
    import tempfile
    with tempfile.TemporaryDirectory() as temp_dir:
        original_dir = os.getcwd()
        try:
            os.chdir(temp_dir)
            
            # Test init help
            result = run_cli_command(cli_path, ['init', '--help'])
            results['init_help'] = result
            
            # Test beam help
            result = run_cli_command(cli_path, ['beam', '--help'])
            results['beam_help'] = result
            
            # Test rift help
            result = run_cli_command(cli_path, ['rift', '--help'])
            results['rift_help'] = result
            
        finally:
            os.chdir(original_dir)
    
    return results

def main():
    # Find CLI binary
    cli_path = None
    search_paths = [
        "target/debug/mothership.exe",
        "target/release/mothership.exe",
        "mothership-cli/target/debug/mothership.exe",
        "mothership-cli/target/release/mothership.exe"
    ]
    
    for path in search_paths:
        if os.path.exists(path):
            cli_path = path
            break
    
    if not cli_path:
        print("Could not find CLI binary")
        sys.exit(1)
    
    print(f"Using CLI: {cli_path}")
    print("="*60)
    
    # Run integration tests
    all_results = {}
    
    test_categories = [
        ('daemon_integration', test_daemon_integration),
        ('server_connection', test_server_connection),
        ('authentication_flow', test_authentication_flow),
        ('project_operations', test_project_operations)
    ]
    
    for category_name, test_func in test_categories:
        print(f"\n{'='*20} {category_name.upper()} {'='*20}")
        try:
            results = test_func(cli_path)
            all_results[category_name] = results
        except Exception as e:
            print(f"Error in {category_name}: {e}")
            all_results[category_name] = {'error': str(e)}
    
    # Save results
    output_dir = Path("analysis/evidence/cli_testing")
    output_dir.mkdir(parents=True, exist_ok=True)
    
    with open(output_dir / "integration_test_results.json", 'w', encoding='utf-8') as f:
        json.dump(all_results, f, indent=2, default=str)
    
    # Generate summary
    print("\n" + "="*60)
    print("INTEGRATION TEST SUMMARY")
    print("="*60)
    
    # Analyze daemon integration
    daemon_results = all_results.get('daemon_integration', {})
    if daemon_results.get('daemon_http_check', {}).get('running'):
        print("Daemon is running and accessible")
    else:
        print("Daemon is not running or not accessible")
    
    # Analyze server connection
    server_results = all_results.get('server_connection', {})
    server_found = False
    for key, value in server_results.items():
        if key.startswith('server_check_') and isinstance(value, dict) and value.get('running'):
            print(f"Server found on port {value['port']}")
            server_found = True
            break
    
    if not server_found:
        print("No Mothership server found running")
    
    # Analyze authentication
    auth_results = all_results.get('authentication_flow', {})
    auth_working = False
    for key, value in auth_results.items():
        if isinstance(value, dict) and value.get('success') and 'authentication' in value.get('stderr', '').lower():
            auth_working = True
            break
    
    if auth_working:
        print("Authentication flow is working")
    else:
        print(" Authentication requires server connection")
    
    print(f"\nDetailed results saved to: {output_dir}/integration_test_results.json")

if __name__ == "__main__":
    main()