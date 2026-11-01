#!/usr/bin/env python3
"""
Integration Test Executor for Mothership Analysis
Executes the provided integration test script and captures detailed results
"""

import subprocess
import sys
import os
import json
import time
from datetime import datetime
from pathlib import Path
import re

class IntegrationTestExecutor:
    def __init__(self, workspace_root):
        self.workspace_root = Path(workspace_root)
        self.script_path = self.workspace_root / "scripts" / "integration-test.ps1"
        self.results = {
            "execution_time": None,
            "start_time": None,
            "end_time": None,
            "exit_code": None,
            "stdout": "",
            "stderr": "",
            "test_steps": [],
            "passed_tests": [],
            "failed_tests": [],
            "features_tested": [],
            "analysis": {}
        }
        
    def execute_integration_test(self):
        """Execute the PowerShell integration test script"""
        print("Starting integration test execution...")
        
        if not self.script_path.exists():
            raise FileNotFoundError(f"Integration test script not found: {self.script_path}")
        
        # Record start time
        self.results["start_time"] = datetime.now().isoformat()
        start_time = time.time()
        
        try:
            # Execute PowerShell script
            process = subprocess.Popen(
                ["powershell", "-ExecutionPolicy", "Bypass", "-File", str(self.script_path)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                cwd=str(self.workspace_root)
            )
            
            stdout, stderr = process.communicate()
            
            # Record results
            end_time = time.time()
            self.results["end_time"] = datetime.now().isoformat()
            self.results["execution_time"] = end_time - start_time
            self.results["exit_code"] = process.returncode
            self.results["stdout"] = stdout
            self.results["stderr"] = stderr
            
            print(f"Integration test completed in {self.results['execution_time']:.2f} seconds")
            print(f"Exit code: {self.results['exit_code']}")
            
            return self.results
            
        except Exception as e:
            print(f"Error executing integration test: {e}")
            self.results["error"] = str(e)
            return self.results
    
    def parse_test_output(self):
        """Parse the test output to extract test steps and results"""
        print("Parsing test output...")
        
        stdout = self.results.get("stdout", "")
        stderr = self.results.get("stderr", "")
        
        # Extract test steps from output
        step_pattern = r"==== (.+?) ===="
        steps = re.findall(step_pattern, stdout)
        self.results["test_steps"] = steps
        
        # Extract success messages
        success_pattern = r"\[\+\] (.+)"
        successes = re.findall(success_pattern, stdout)
        self.results["passed_tests"] = successes
        
        # Extract error messages
        error_pattern = r"\[-\] (.+)"
        errors = re.findall(error_pattern, stdout)
        self.results["failed_tests"] = errors
        
        # Extract features tested from final summary
        if "Tested features:" in stdout:
            features_section = stdout.split("Tested features:")[1]
            feature_pattern = r"\[\+\] (.+)"
            features = re.findall(feature_pattern, features_section)
            self.results["features_tested"] = features
        
        print(f"Found {len(steps)} test steps")
        print(f"{len(successes)} passed tests")
        print(f"{len(errors)} failed tests")
        print(f"{len(self.results['features_tested'])} features tested")
        
        return self.results
    
    def analyze_test_coverage(self):
        """Analyze test coverage against advertised features"""
        print("Analyzing test coverage...")
        
        # Define advertised features from requirements
        advertised_features = {
            "real_time_collaboration": {
                "name": "Real-time Collaboration",
                "claims": ["millisecond synchronization", "collaborative editing", "conflict resolution"],
                "tested": False
            },
            "background_daemon": {
                "name": "Background Daemon",
                "claims": ["automatic file watching", "background sync", "daemon lifecycle"],
                "tested": False
            },
            "oauth_authentication": {
                "name": "OAuth Authentication", 
                "claims": ["Google/GitHub integration", "JWT tokens", "session persistence"],
                "tested": False
            },
            "database_persistence": {
                "name": "Database Persistence",
                "claims": ["PostgreSQL integration", "data persistence", "CRUD operations"],
                "tested": False
            },
            "cli_interface": {
                "name": "CLI Interface",
                "claims": ["command functionality", "error handling", "server integration"],
                "tested": False
            },
            "docker_deployment": {
                "name": "Docker Deployment",
                "claims": ["containerization", "service orchestration", "health checks"],
                "tested": False
            }
        }
        
        # Check which features were actually tested
        tested_features = self.results.get("features_tested", [])
        
        # Map tested features to advertised features
        for feature in tested_features:
            feature_lower = feature.lower()
            if "authentication" in feature_lower or "auth" in feature_lower:
                advertised_features["oauth_authentication"]["tested"] = True
            elif "daemon" in feature_lower:
                advertised_features["background_daemon"]["tested"] = True
            elif "sync" in feature_lower:
                # Note: sync != real-time collaboration
                pass
            elif "project" in feature_lower or "cli" in feature_lower:
                advertised_features["cli_interface"]["tested"] = True
        
        # Calculate coverage
        total_features = len(advertised_features)
        tested_count = sum(1 for f in advertised_features.values() if f["tested"])
        coverage_percentage = (tested_count / total_features) * 100
        
        self.results["analysis"] = {
            "advertised_features": advertised_features,
            "total_advertised_features": total_features,
            "features_actually_tested": tested_count,
            "coverage_percentage": coverage_percentage,
            "missing_tests": [
                f["name"] for f in advertised_features.values() if not f["tested"]
            ]
        }
        
        print(f"Test coverage: {coverage_percentage:.1f}% ({tested_count}/{total_features})")
        print(f"Missing tests: {', '.join(self.results['analysis']['missing_tests'])}")
        
        return self.results
    
    def save_results(self, output_file):
        """Save detailed results to file"""
        print(f"Saving results to {output_file}")
        
        with open(output_file, 'w', encoding='utf-8') as f:
            json.dump(self.results, f, indent=2)
        
        # Also create a human-readable report
        report_file = output_file.replace('.json', '_report.md')
        self.generate_report(report_file)
        
        print(f"Report saved to {report_file}")
    
    def generate_report(self, report_file):
        """Generate human-readable markdown report"""
        with open(report_file, 'w', encoding='utf-8') as f:
            f.write("# Integration Test Execution Report\n\n")
            f.write(f"**Execution Date:** {self.results['start_time']}\n")
            f.write(f"**Duration:** {self.results['execution_time']:.2f} seconds\n")
            f.write(f"**Exit Code:** {self.results['exit_code']}\n\n")
            
            # Test Results Summary
            f.write("## Test Results Summary\n\n")
            f.write(f"- **Total Test Steps:** {len(self.results['test_steps'])}\n")
            f.write(f"- **Passed Tests:** {len(self.results['passed_tests'])}\n")
            f.write(f"- **Failed Tests:** {len(self.results['failed_tests'])}\n")
            f.write(f"- **Overall Status:** {'PASSED' if self.results['exit_code'] == 0 else 'FAILED'}\n\n")
            
            # Test Steps
            if self.results['test_steps']:
                f.write("## Test Steps Executed\n\n")
                for i, step in enumerate(self.results['test_steps'], 1):
                    f.write(f"{i}. {step}\n")
                f.write("\n")
            
            # Passed Tests
            if self.results['passed_tests']:
                f.write("## Passed Tests\n\n")
                for test in self.results['passed_tests']:
                    f.write(f"{test}\n")
                f.write("\n")
            
            # Failed Tests
            if self.results['failed_tests']:
                f.write("## Failed Tests\n\n")
                for test in self.results['failed_tests']:
                    f.write(f"{test}\n")
                f.write("\n")
            
            # Coverage Analysis
            if 'analysis' in self.results:
                analysis = self.results['analysis']
                f.write("## Test Coverage Analysis\n\n")
                f.write(f"**Coverage:** {analysis['coverage_percentage']:.1f}% ")
                f.write(f"({analysis['features_actually_tested']}/{analysis['total_advertised_features']} features)\n\n")
                
                f.write("### Features Tested vs Advertised\n\n")
                for key, feature in analysis['advertised_features'].items():
                    status = "TESTED" if feature['tested'] else "NOT TESTED"
                    f.write(f"- **{feature['name']}:** {status}\n")
                    for claim in feature['claims']:
                        f.write(f"  - {claim}\n")
                f.write("\n")
                
                if analysis['missing_tests']:
                    f.write("### Missing Test Coverage\n\n")
                    for missing in analysis['missing_tests']:
                        f.write(f"- {missing}\n")
                    f.write("\n")
            
            # Raw Output
            f.write("## Raw Test Output\n\n")
            f.write("### STDOUT\n```\n")
            f.write(self.results['stdout'])
            f.write("\n```\n\n")
            
            if self.results['stderr']:
                f.write("### STDERR\n```\n")
                f.write(self.results['stderr'])
                f.write("\n```\n\n")

def main():
    if len(sys.argv) != 2:
        print("Usage: python integration_test_executor.py <workspace_root>")
        sys.exit(1)
    
    workspace_root = sys.argv[1]
    executor = IntegrationTestExecutor(workspace_root)
    
    try:
        # Execute the integration test
        results = executor.execute_integration_test()
        
        # Parse the output
        executor.parse_test_output()
        
        # Analyze coverage
        executor.analyze_test_coverage()
        
        # Save results
        output_file = Path(workspace_root) / "analysis" / "evidence" / "integration_test_results.json"
        output_file.parent.mkdir(parents=True, exist_ok=True)
        executor.save_results(str(output_file))
        
        print("\nIntegration test analysis complete!")
        print(f"Results saved to: {output_file}")
        
        # Return appropriate exit code
        sys.exit(0 if results['exit_code'] == 0 else 1)
        
    except Exception as e:
        print(f"Fatal error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()