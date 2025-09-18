#!/usr/bin/env python3
"""
Quick CLI Test for Mothership Analysis
Tests CLI commands with short timeouts to avoid hanging
"""

import subprocess
import json
import os
import sys
from pathlib import Path
from typing import Dict, List, Any

def run_cli_command(cli_path: str, args: List[str], timeout: int = 5) -> Dict[str, Any]:
    """Run a CLI command with short timeout"""
    cmd = [cli_path] + args
    print(f"Testing: {' '.join(cmd)}")
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
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

def analyze_output(result: Dict[str, Any]) -> str:
    """Analyze command output to determine status"""
    if not result['success']:
        if result.get('error') == 'timeout':
            return 'TIMEOUT'
        return 'ERROR'
    
    if result['returncode'] != 0:
        return 'FAILED'
    
    output = (result.get('stdout', '') + result.get('stderr', '')).lower()
    
    # Check for placeholder indicators
    if any(indicator in output for indicator in [
        'not yet implemented', 'not implemented', 'placeholder', 'todo', 'coming soon'
    ]):
        return 'PLACEHOLDER'
    
    # Check for working indicators
    if any(indicator in output for indicator in [
        'usage:', 'commands:', 'help', 'options:', 'subcommands:'
    ]):
        return 'WORKING'
    
    return 'UNKNOWN'

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
    
    # Test commands with short timeouts
    test_commands = [
        (['--help'], 5),
        (['--version'], 5),
        (['auth', '--help'], 5),
        (['gateway', '--help'], 5),
        (['gateway', 'list'], 10),
        (['init', '--help'], 5),
        (['beam', '--help'], 5),
        (['status'], 10),
        (['rift', '--help'], 5),
        (['rift', 'list'], 10),
        (['daemon', '--help'], 5),
        (['daemon', 'status'], 8),  # Shorter timeout for daemon
        (['checkpoint', '--help'], 5),
        (['sync'], 8),
        (['history'], 10),
        (['server', '--help'], 5),
        (['server', 'status'], 8),
        (['connect', '--help'], 5),
        (['update', '--help'], 5)
    ]
    
    results = {}
    working = 0
    failed = 0
    timeout = 0
    placeholder = 0
    
    for args, cmd_timeout in test_commands:
        result = run_cli_command(cli_path, args, cmd_timeout)
        status = analyze_output(result)
        
        results[' '.join(args)] = {
            'status': status,
            'result': result
        }
        
        print(f"  Status: {status}")
        
        if status == 'WORKING':
            working += 1
        elif status in ['FAILED', 'ERROR']:
            failed += 1
        elif status == 'TIMEOUT':
            timeout += 1
        elif status == 'PLACEHOLDER':
            placeholder += 1
        
        print()
    
    # Summary
    total = len(test_commands)
    print("="*60)
    print("SUMMARY")
    print("="*60)
    print(f"Total commands tested: {total}")
    print(f"Working: {working}")
    print(f"Failed/Error: {failed}")
    print(f"Timeout: {timeout}")
    print(f"Placeholder: {placeholder}")
    print(f"Success rate: {(working/total)*100:.1f}%")
    
    # Save results
    output_dir = Path("analysis/evidence/cli_testing")
    output_dir.mkdir(parents=True, exist_ok=True)
    
    with open(output_dir / "quick_test_results.json", 'w', encoding='utf-8') as f:
        json.dump(results, f, indent=2, default=str)
    
    # Generate simple report
    report = []
    report.append("# Quick CLI Test Results")
    report.append("")
    report.append("## Summary")
    report.append(f"- Total: {total}")
    report.append(f"- Working: {working}")
    report.append(f"- Failed: {failed}")
    report.append(f"- Timeout: {timeout}")
    report.append(f"- Placeholder: {placeholder}")
    report.append("")
    report.append("## Command Details")
    
    for cmd, data in results.items():
        status = data['status']
        result = data['result']
        
        report.append(f"### {cmd}")
        report.append(f"Status: **{status}**")
        
        if result['success']:
            report.append(f"Return code: {result['returncode']}")
            if result['stdout']:
                report.append("Output preview:")
                report.append("```")
                report.append(result['stdout'][:200] + ("..." if len(result['stdout']) > 200 else ""))
                report.append("```")
        else:
            report.append(f"Error: {result.get('error', 'unknown')}")
        
        report.append("")
    
    with open(output_dir / "quick_test_report.md", 'w', encoding='utf-8') as f:
        f.write('\n'.join(report))
    
    print(f"\nResults saved to: {output_dir}")

if __name__ == "__main__":
    main()