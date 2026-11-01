#!/usr/bin/env python3
"""
CLI Command Tester for Mothership Analysis
Tests all CLI commands to determine actual functionality vs placeholders
"""

import subprocess
import json
import os
import sys
import time
from pathlib import Path
from typing import Dict, List, Tuple, Optional, Any
import tempfile
import shutil

from logger import analysis_logger
import logging

class CLITester:
    def __init__(self, cli_path: str = None):
        self.logger = analysis_logger.main_logger
        
        # Try to find CLI binary
        self.cli_path = self._find_cli_binary(cli_path)
        if not self.cli_path:
            raise RuntimeError("Could not find mothership CLI binary")
            
        self.logger.info(f"Using CLI binary: {self.cli_path}")
        
        # Test results storage
        self.test_results = {
            'commands_tested': 0,
            'commands_working': 0,
            'commands_broken': 0,
            'commands_placeholder': 0,
            'detailed_results': {}
        }
        
    def _find_cli_binary(self, cli_path: str = None) -> Optional[str]:
        """Find the mothership CLI binary"""
        if cli_path and os.path.exists(cli_path):
            return cli_path
            
        # Common locations to check
        search_paths = [
            "mothership-cli/target/debug/mothership-cli.exe",
            "mothership-cli/target/release/mothership-cli.exe", 
            "target/debug/mothership-cli.exe",
            "target/release/mothership-cli.exe",
            "mothership.exe",
            "mothership-cli.exe"
        ]
        
        # Check in current directory and parent directories
        current_dir = Path.cwd()
        for i in range(3):  # Check up to 3 levels up
            for search_path in search_paths:
                full_path = current_dir / search_path
                if full_path.exists():
                    return str(full_path)
            current_dir = current_dir.parent
            
        # Check PATH
        try:
            result = subprocess.run(['where', 'mothership'], 
                                  capture_output=True, text=True, shell=True)
            if result.returncode == 0:
                return result.stdout.strip().split('\n')[0]
        except:
            pass
            
        return None
        
    def run_command(self, args: List[str], timeout: int = 30, 
                   input_text: str = None) -> Dict[str, Any]:
        """Run a CLI command and capture results"""
        cmd = [self.cli_path] + args
        self.logger.info(f"Running: {' '.join(cmd)}")
        
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
                'success': True,
                'returncode': result.returncode,
                'stdout': result.stdout,
                'stderr': result.stderr,
                'command': ' '.join(cmd)
            }
        except subprocess.TimeoutExpired:
            return {
                'success': False,
                'error': 'timeout',
                'command': ' '.join(cmd)
            }
        except Exception as e:
            return {
                'success': False,
                'error': str(e),
                'command': ' '.join(cmd)
            }
            
    def analyze_command_output(self, result: Dict[str, Any]) -> Dict[str, Any]:
        """Analyze command output to determine functionality level"""
        if not result['success']:
            return {
                'status': 'broken',
                'reason': f"Command failed: {result.get('error', 'unknown error')}",
                'evidence': result
            }
            
        stdout = result['stdout'].lower()
        stderr = result['stderr'].lower()
        combined = stdout + stderr
        
        # Check for placeholder indicators
        placeholder_indicators = [
            'not yet implemented',
            'not implemented',
            'placeholder',
            'todo',
            'coming soon',
            'feature not available',
            'not supported yet'
        ]
        
        for indicator in placeholder_indicators:
            if indicator in combined:
                return {
                    'status': 'placeholder',
                    'reason': f"Contains placeholder text: '{indicator}'",
                    'evidence': result
                }
                
        # Check for error indicators
        error_indicators = [
            'error',
            'failed',
            'could not',
            'cannot',
            'not found',
            'invalid',
            'unauthorized',
            'authentication required'
        ]
        
        # Only consider it broken if it's a hard error, not auth-related
        auth_related = any(auth in combined for auth in [
            'not authenticated',
            'please run',
            'auth',
            'login'
        ])
        
        if result['returncode'] != 0 and not auth_related:
            for indicator in error_indicators:
                if indicator in combined:
                    return {
                        'status': 'broken',
                        'reason': f"Command failed with error: {indicator}",
                        'evidence': result
                    }
                    
        # Check for working indicators
        working_indicators = [
            'success',
            'completed',
            'created',
            'connected',
            'authenticated',
            'synced',
            'beaming',
            'gateway',
            'project',
            'rift'
        ]
        
        for indicator in working_indicators:
            if indicator in combined:
                return {
                    'status': 'working',
                    'reason': f"Shows working functionality: '{indicator}'",
                    'evidence': result
                }
                
        # If we get help output, command structure exists
        if 'usage:' in combined or 'commands:' in combined or '--help' in combined:
            return {
                'status': 'partial',
                'reason': 'Command structure exists (shows help)',
                'evidence': result
            }
            
        # Default to unknown if we can't determine
        return {
            'status': 'unknown',
            'reason': 'Could not determine functionality level',
            'evidence': result
        }
        
    def test_basic_commands(self) -> Dict[str, Any]:
        """Test basic CLI commands"""
        self.logger.info("Testing basic CLI commands...")
        
        basic_tests = {
            'help': ['--help'],
            'version': ['--version'],
            'verbose_help': ['-h', '-v']
        }
        
        results = {}
        for test_name, args in basic_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_auth_commands(self) -> Dict[str, Any]:
        """Test authentication commands"""
        self.logger.info("Testing authentication commands...")
        
        auth_tests = {
            'auth_help': ['auth', '--help'],
            'auth_google': ['auth', 'google'],  # Will likely fail without server
            'auth_github': ['auth', 'github'],  # Will likely fail without server
            'logout': ['logout']
        }
        
        results = {}
        for test_name, args in auth_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            
            # For auth commands that might hang, use shorter timeout
            timeout = 10 if 'auth' in args and len(args) > 1 else 30
            
            result = self.run_command(args, timeout=timeout)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_server_commands(self) -> Dict[str, Any]:
        """Test server management commands"""
        self.logger.info("Testing server management commands...")
        
        server_tests = {
            'connect_help': ['connect', '--help'],
            'connect_invalid': ['connect', 'http://invalid-server.test'],
            'server_status': ['server', 'status'],
            'server_list': ['server', 'list'],
            'server_disconnect': ['server', 'disconnect']
        }
        
        results = {}
        for test_name, args in server_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            
            # Connect commands might take longer
            timeout = 15 if 'connect' in args else 30
            
            result = self.run_command(args, timeout=timeout)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_project_commands(self) -> Dict[str, Any]:
        """Test project management commands"""
        self.logger.info("Testing project management commands...")
        
        project_tests = {
            'gateway_help': ['gateway', '--help'],
            'gateway_list': ['gateway', 'list'],
            'init_help': ['init', '--help'],
            'beam_help': ['beam', '--help'],
            'status': ['status'],
            'history': ['history'],
            'sync': ['sync']
        }
        
        results = {}
        for test_name, args in project_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_rift_commands(self) -> Dict[str, Any]:
        """Test rift (collaboration) commands"""
        self.logger.info("Testing rift commands...")
        
        rift_tests = {
            'rift_help': ['rift', '--help'],
            'rift_list': ['rift', 'list'],
            'rift_status': ['rift', 'status'],
            'rift_new_help': ['rift', 'new', '--help'],
            'rift_switch_help': ['rift', 'switch', '--help'],
            'rift_diff_help': ['rift', 'diff', '--help']
        }
        
        results = {}
        for test_name, args in rift_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_daemon_commands(self) -> Dict[str, Any]:
        """Test daemon management commands"""
        self.logger.info("Testing daemon commands...")
        
        daemon_tests = {
            'daemon_help': ['daemon', '--help'],
            'daemon_status': ['daemon', 'status'],
            'daemon_stop': ['daemon', 'stop'],
            'daemon_restart': ['daemon', 'restart']
        }
        
        results = {}
        for test_name, args in daemon_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_checkpoint_commands(self) -> Dict[str, Any]:
        """Test checkpoint (version control) commands"""
        self.logger.info("Testing checkpoint commands...")
        
        checkpoint_tests = {
            'checkpoint_help': ['checkpoint', '--help'],
            'restore_help': ['restore', '--help'],
            'delete_help': ['delete', '--help']
        }
        
        results = {}
        for test_name, args in checkpoint_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_update_commands(self) -> Dict[str, Any]:
        """Test update commands"""
        self.logger.info("Testing update commands...")
        
        update_tests = {
            'update_help': ['update', '--help']
        }
        
        results = {}
        for test_name, args in update_tests.items():
            self.logger.info(f"Testing {test_name}: {args}")
            result = self.run_command(args)
            analysis = self.analyze_command_output(result)
            results[test_name] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def test_integration_scenarios(self) -> Dict[str, Any]:
        """Test integration scenarios that require multiple commands"""
        self.logger.info("Testing integration scenarios...")
        
        results = {}
        
        # Test 1: Try to create a project in a temp directory
        with tempfile.TemporaryDirectory() as temp_dir:
            os.chdir(temp_dir)
            
            # Test init command
            self.logger.info("Testing init in temporary directory...")
            result = self.run_command(['init', 'test-project'])
            analysis = self.analyze_command_output(result)
            results['init_integration'] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
            # Test status in the directory
            self.logger.info("Testing status after init...")
            result = self.run_command(['status'])
            analysis = self.analyze_command_output(result)
            results['status_after_init'] = analysis
            
            self.test_results['commands_tested'] += 1
            if analysis['status'] == 'working':
                self.test_results['commands_working'] += 1
            elif analysis['status'] == 'broken':
                self.test_results['commands_broken'] += 1
            elif analysis['status'] == 'placeholder':
                self.test_results['commands_placeholder'] += 1
                
        return results
        
    def run_comprehensive_test(self) -> Dict[str, Any]:
        """Run comprehensive CLI testing"""
        self.logger.info("Starting comprehensive CLI testing...")
        
        all_results = {}
        
        # Test all command categories
        test_categories = [
            ('basic', self.test_basic_commands),
            ('auth', self.test_auth_commands),
            ('server', self.test_server_commands),
            ('project', self.test_project_commands),
            ('rift', self.test_rift_commands),
            ('daemon', self.test_daemon_commands),
            ('checkpoint', self.test_checkpoint_commands),
            ('update', self.test_update_commands),
            ('integration', self.test_integration_scenarios)
        ]
        
        for category_name, test_func in test_categories:
            self.logger.info(f"Testing {category_name} commands...")
            try:
                category_results = test_func()
                all_results[category_name] = category_results
            except Exception as e:
                self.logger.error(f"Error testing {category_name}: {e}")
                all_results[category_name] = {
                    'error': str(e)
                }
                
        # Store detailed results
        self.test_results['detailed_results'] = all_results
        
        return self.test_results
        
    def generate_report(self) -> str:
        """Generate a comprehensive test report"""
        report = []
        report.append("# CLI Command Analysis Report")
        report.append("")
        report.append("## Summary")
        report.append(f"- Total commands tested: {self.test_results['commands_tested']}")
        report.append(f"- Working commands: {self.test_results['commands_working']}")
        report.append(f"- Broken commands: {self.test_results['commands_broken']}")
        report.append(f"- Placeholder commands: {self.test_results['commands_placeholder']}")
        
        if self.test_results['commands_tested'] > 0:
            working_pct = (self.test_results['commands_working'] / self.test_results['commands_tested']) * 100
            broken_pct = (self.test_results['commands_broken'] / self.test_results['commands_tested']) * 100
            placeholder_pct = (self.test_results['commands_placeholder'] / self.test_results['commands_tested']) * 100
            
            report.append(f"- Working: {working_pct:.1f}%")
            report.append(f"- Broken: {broken_pct:.1f}%")
            report.append(f"- Placeholder: {placeholder_pct:.1f}%")
            
        report.append("")
        report.append("## Detailed Results")
        
        for category, results in self.test_results['detailed_results'].items():
            report.append(f"### {category.title()} Commands")
            
            if 'error' in results:
                report.append(f"Category failed: {results['error']}")
                continue
                
            for test_name, analysis in results.items():
                status = analysis['status']
                reason = analysis['reason']
                
                if status == 'working':
                    icon = "[OK]"
                elif status == 'broken':
                    icon = "[FAIL]"
                elif status == 'placeholder':
                    icon = "[TODO]"
                else:
                    icon = "[?]"
                    
                report.append(f"{icon} **{test_name}**: {reason}")
                
                # Add command details
                if 'evidence' in analysis and 'command' in analysis['evidence']:
                    report.append(f"   - Command: `{analysis['evidence']['command']}`")
                    
                if 'evidence' in analysis and 'returncode' in analysis['evidence']:
                    report.append(f"   - Return code: {analysis['evidence']['returncode']}")
                    
            report.append("")
            
        return "\n".join(report)
        
    def save_evidence(self, output_dir: str):
        """Save all test evidence to files"""
        evidence_dir = Path(output_dir) / "cli_testing"
        evidence_dir.mkdir(parents=True, exist_ok=True)
        
        # Save main report
        report = self.generate_report()
        with open(evidence_dir / "cli_analysis_report.md", 'w', encoding='utf-8') as f:
            f.write(report)
            
        # Save raw test results
        with open(evidence_dir / "raw_test_results.json", 'w', encoding='utf-8') as f:
            json.dump(self.test_results, f, indent=2, default=str)
            
        # Save individual command outputs
        for category, results in self.test_results['detailed_results'].items():
            if 'error' in results:
                continue
                
            category_dir = evidence_dir / category
            category_dir.mkdir(exist_ok=True)
            
            for test_name, analysis in results.items():
                if 'evidence' in analysis:
                    evidence_file = category_dir / f"{test_name}.json"
                    with open(evidence_file, 'w', encoding='utf-8') as f:
                        json.dump(analysis['evidence'], f, indent=2, default=str)
                        
        self.logger.info(f"CLI testing evidence saved to {evidence_dir}")


def main():
    """Main CLI testing function"""
    import argparse
    
    parser = argparse.ArgumentParser(description="Test Mothership CLI commands")
    parser.add_argument('--cli-path', help="Path to mothership CLI binary")
    parser.add_argument('--output-dir', default='analysis/evidence', 
                       help="Output directory for evidence")
    
    args = parser.parse_args()
    
    try:
        tester = CLITester(args.cli_path)
        results = tester.run_comprehensive_test()
        
        print("\n" + "="*60)
        print("CLI TESTING COMPLETE")
        print("="*60)
        print(f"Commands tested: {results['commands_tested']}")
        print(f"Working: {results['commands_working']}")
        print(f"Broken: {results['commands_broken']}")
        print(f"Placeholder: {results['commands_placeholder']}")
        
        if results['commands_tested'] > 0:
            working_pct = (results['commands_working'] / results['commands_tested']) * 100
            print(f"Success rate: {working_pct:.1f}%")
            
        # Save evidence
        tester.save_evidence(args.output_dir)
        
        print(f"\nDetailed evidence saved to: {args.output_dir}/cli_testing/")
        
    except Exception as e:
        print(f"CLI testing failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()