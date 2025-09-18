#!/usr/bin/env python3
"""
Daemon functional tester for Mothership application.
Tests daemon startup, file watching, and IPC communication.
"""

import subprocess
import time
import requests
import json
import tempfile
import os
import signal
from pathlib import Path
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
import threading
import psutil

from logger import analysis_logger
from evidence_collector import evidence_collector, EvidenceType, ConfidenceLevel

@dataclass
class DaemonTestResult:
    """Represents a daemon test result."""
    test_name: str
    passed: bool
    description: str
    details: Dict[str, Any]
    evidence: Optional[str] = None

class DaemonFunctionalTester:
    """Tests daemon functionality."""
    
    def __init__(self):
        self.daemon_process = None
        self.test_results: List[DaemonTestResult] = []
        self.daemon_port = 7525
        self.daemon_url = f"http://localhost:{self.daemon_port}"
        self.test_dir = None
        
    def run_all_tests(self) -> Dict[str, Any]:
        """Run all daemon functional tests."""
        analysis_logger.log_analysis_start("daemon_functionality", "background_sync_testing")
        
        try:
            # Test 1: Daemon binary existence
            self._test_daemon_binary_exists()
            
            # Test 2: Daemon startup
            self._test_daemon_startup()
            
            # Test 3: IPC communication
            self._test_ipc_communication()
            
            # Test 4: File watching functionality
            self._test_file_watching()
            
            # Test 5: Daemon lifecycle management
            self._test_daemon_lifecycle()
            
        except Exception as e:
            analysis_logger.log_error(f"Daemon testing failed: {e}")
            self.test_results.append(DaemonTestResult(
                test_name="daemon_testing_error",
                passed=False,
                description="Daemon testing encountered an error",
                details={"error": str(e)}
            ))
        finally:
            self._cleanup()
            
        # Generate summary
        return self._generate_summary()
    
    def _test_daemon_binary_exists(self):
        """Test if daemon binary exists and is executable."""
        analysis_logger.log_step("Testing daemon binary existence")
        
        # Look for daemon binary in common locations
        possible_paths = [
            "mothership-daemon/target/release/mothership-daemon",
            "mothership-daemon/target/debug/mothership-daemon", 
            "target/release/mothership-daemon",
            "target/debug/mothership-daemon",
            "./mothership-daemon",
            "mothership-daemon.exe"
        ]
        
        daemon_path = None
        for path in possible_paths:
            if os.path.exists(path) and os.access(path, os.X_OK):
                daemon_path = path
                break
                
        if daemon_path:
            self.test_results.append(DaemonTestResult(
                test_name="daemon_binary_exists",
                passed=True,
                description="Daemon binary found and executable",
                details={"path": daemon_path}
            ))
            self.daemon_path = daemon_path
        else:
            self.test_results.append(DaemonTestResult(
                test_name="daemon_binary_exists", 
                passed=False,
                description="Daemon binary not found or not executable",
                details={"searched_paths": possible_paths}
            ))
            
    def _test_daemon_startup(self):
        """Test daemon startup process."""
        analysis_logger.log_step("Testing daemon startup")
        
        if not hasattr(self, 'daemon_path'):
            self.test_results.append(DaemonTestResult(
                test_name="daemon_startup",
                passed=False,
                description="Cannot test startup - daemon binary not found",
                details={}
            ))
            return
            
        try:
            # Start daemon process
            self.daemon_process = subprocess.Popen(
                [self.daemon_path],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True
            )
            
            # Wait for daemon to start
            time.sleep(3)
            
            # Check if process is still running
            if self.daemon_process.poll() is None:
                self.test_results.append(DaemonTestResult(
                    test_name="daemon_startup",
                    passed=True,
                    description="Daemon started successfully",
                    details={"pid": self.daemon_process.pid}
                ))
            else:
                stdout, stderr = self.daemon_process.communicate()
                self.test_results.append(DaemonTestResult(
                    test_name="daemon_startup",
                    passed=False,
                    description="Daemon failed to start or exited immediately",
                    details={"stdout": stdout, "stderr": stderr}
                ))
                
        except Exception as e:
            self.test_results.append(DaemonTestResult(
                test_name="daemon_startup",
                passed=False,
                description="Failed to start daemon process",
                details={"error": str(e)}
            ))
            
    def _test_ipc_communication(self):
        """Test IPC communication with daemon."""
        analysis_logger.log_step("Testing IPC communication")
        
        if not self.daemon_process or self.daemon_process.poll() is not None:
            self.test_results.append(DaemonTestResult(
                test_name="ipc_communication",
                passed=False,
                description="Cannot test IPC - daemon not running",
                details={}
            ))
            return
            
        # Wait a bit more for daemon to fully initialize
        time.sleep(2)
        
        # Test health endpoint
        try:
            response = requests.get(f"{self.daemon_url}/health", timeout=5)
            if response.status_code == 200:
                self.test_results.append(DaemonTestResult(
                    test_name="ipc_health_check",
                    passed=True,
                    description="Health check endpoint responding",
                    details={"status_code": response.status_code, "response": response.text}
                ))
            else:
                self.test_results.append(DaemonTestResult(
                    test_name="ipc_health_check",
                    passed=False,
                    description="Health check endpoint returned error",
                    details={"status_code": response.status_code, "response": response.text}
                ))
        except requests.exceptions.RequestException as e:
            self.test_results.append(DaemonTestResult(
                test_name="ipc_health_check",
                passed=False,
                description="Failed to connect to health check endpoint",
                details={"error": str(e)}
            ))
            
        # Test status endpoint
        try:
            response = requests.get(f"{self.daemon_url}/status", timeout=5)
            if response.status_code == 200:
                status_data = response.json() if response.headers.get('content-type', '').startswith('application/json') else response.text
                self.test_results.append(DaemonTestResult(
                    test_name="ipc_status_check",
                    passed=True,
                    description="Status endpoint responding",
                    details={"status_code": response.status_code, "data": status_data}
                ))
            else:
                self.test_results.append(DaemonTestResult(
                    test_name="ipc_status_check",
                    passed=False,
                    description="Status endpoint returned error",
                    details={"status_code": response.status_code, "response": response.text}
                ))
        except requests.exceptions.RequestException as e:
            self.test_results.append(DaemonTestResult(
                test_name="ipc_status_check",
                passed=False,
                description="Failed to connect to status endpoint",
                details={"error": str(e)}
            ))
            
    def _test_file_watching(self):
        """Test file watching and change detection."""
        analysis_logger.log_step("Testing file watching functionality")
        
        if not self.daemon_process or self.daemon_process.poll() is not None:
            self.test_results.append(DaemonTestResult(
                test_name="file_watching",
                passed=False,
                description="Cannot test file watching - daemon not running",
                details={}
            ))
            return
            
        # Create temporary directory for testing
        self.test_dir = tempfile.mkdtemp(prefix="daemon_test_")
        
        try:
            # Add project to daemon
            add_project_data = {
                "path": self.test_dir,
                "name": "test_project"
            }
            
            response = requests.post(
                f"{self.daemon_url}/projects/add",
                json=add_project_data,
                timeout=5
            )
            
            if response.status_code == 200:
                self.test_results.append(DaemonTestResult(
                    test_name="project_registration",
                    passed=True,
                    description="Successfully registered test project",
                    details={"response": response.text}
                ))
                
                # Test file creation detection
                test_file = os.path.join(self.test_dir, "test_file.txt")
                with open(test_file, 'w') as f:
                    f.write("Initial content")
                    
                # Wait for file watching to detect change
                time.sleep(2)
                
                # Test file modification detection
                with open(test_file, 'w') as f:
                    f.write("Modified content")
                    
                time.sleep(2)
                
                # Check if daemon is still responsive (basic test)
                health_response = requests.get(f"{self.daemon_url}/health", timeout=5)
                if health_response.status_code == 200:
                    self.test_results.append(DaemonTestResult(
                        test_name="file_change_detection",
                        passed=True,
                        description="File changes processed without daemon crash",
                        details={"test_file": test_file}
                    ))
                else:
                    self.test_results.append(DaemonTestResult(
                        test_name="file_change_detection",
                        passed=False,
                        description="Daemon became unresponsive after file changes",
                        details={"test_file": test_file}
                    ))
                    
            else:
                self.test_results.append(DaemonTestResult(
                    test_name="project_registration",
                    passed=False,
                    description="Failed to register test project",
                    details={"status_code": response.status_code, "response": response.text}
                ))
                
        except requests.exceptions.RequestException as e:
            self.test_results.append(DaemonTestResult(
                test_name="file_watching",
                passed=False,
                description="Failed to test file watching due to connection error",
                details={"error": str(e)}
            ))
        except Exception as e:
            self.test_results.append(DaemonTestResult(
                test_name="file_watching",
                passed=False,
                description="File watching test failed",
                details={"error": str(e)}
            ))
            
    def _test_daemon_lifecycle(self):
        """Test daemon lifecycle management."""
        analysis_logger.log_step("Testing daemon lifecycle management")
        
        if not self.daemon_process:
            self.test_results.append(DaemonTestResult(
                test_name="daemon_lifecycle",
                passed=False,
                description="Cannot test lifecycle - daemon process not available",
                details={}
            ))
            return
            
        # Test graceful shutdown via API
        try:
            if self.daemon_process.poll() is None:  # Still running
                response = requests.post(f"{self.daemon_url}/shutdown", timeout=5)
                
                # Wait for shutdown
                time.sleep(3)
                
                if self.daemon_process.poll() is not None:  # Process has exited
                    self.test_results.append(DaemonTestResult(
                        test_name="graceful_shutdown",
                        passed=True,
                        description="Daemon shut down gracefully via API",
                        details={"exit_code": self.daemon_process.poll()}
                    ))
                else:
                    self.test_results.append(DaemonTestResult(
                        test_name="graceful_shutdown",
                        passed=False,
                        description="Daemon did not shut down after API call",
                        details={}
                    ))
            else:
                self.test_results.append(DaemonTestResult(
                    test_name="graceful_shutdown",
                    passed=False,
                    description="Daemon was not running when shutdown test started",
                    details={}
                ))
                
        except requests.exceptions.RequestException as e:
            self.test_results.append(DaemonTestResult(
                test_name="graceful_shutdown",
                passed=False,
                description="Failed to send shutdown command",
                details={"error": str(e)}
            ))
            
    def _cleanup(self):
        """Clean up test resources."""
        analysis_logger.log_step("Cleaning up daemon test resources")
        
        # Kill daemon process if still running
        if self.daemon_process and self.daemon_process.poll() is None:
            try:
                self.daemon_process.terminate()
                self.daemon_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.daemon_process.kill()
                self.daemon_process.wait()
            except Exception as e:
                analysis_logger.log_error(f"Error terminating daemon: {e}")
                
        # Clean up test directory
        if self.test_dir and os.path.exists(self.test_dir):
            try:
                import shutil
                shutil.rmtree(self.test_dir)
            except Exception as e:
                analysis_logger.log_error(f"Error cleaning up test directory: {e}")
                
    def _generate_summary(self) -> Dict[str, Any]:
        """Generate test summary."""
        passed_tests = [r for r in self.test_results if r.passed]
        failed_tests = [r for r in self.test_results if not r.passed]
        
        summary = {
            "total_tests": len(self.test_results),
            "passed": len(passed_tests),
            "failed": len(failed_tests),
            "success_rate": len(passed_tests) / len(self.test_results) if self.test_results else 0,
            "test_results": [
                {
                    "test_name": r.test_name,
                    "passed": r.passed,
                    "description": r.description,
                    "details": r.details
                }
                for r in self.test_results
            ]
        }
        
        # Log evidence
        evidence_collector.add_evidence(
            EvidenceType.FUNCTIONAL_TEST,
            "daemon_functionality_test",
            summary,
            ConfidenceLevel.HIGH,
            "Comprehensive daemon functionality testing results"
        )
        
        analysis_logger.log_analysis_complete("daemon_functionality", summary)
        return summary


def main():
    """Main function to run daemon functional tests."""
    print("Starting daemon functional tests...")
    tester = DaemonFunctionalTester()
    results = tester.run_all_tests()
    
    print("\n" + "="*60)
    print("DAEMON FUNCTIONALITY TEST RESULTS")
    print("="*60)
    print(f"Total Tests: {results['total_tests']}")
    print(f"Passed: {results['passed']}")
    print(f"Failed: {results['failed']}")
    print(f"Success Rate: {results['success_rate']:.1%}")
    print()
    
    for test in results['test_results']:
        status = "✅ PASS" if test['passed'] else "❌ FAIL"
        print(f"{status} {test['test_name']}: {test['description']}")
        if test['details']:
            for key, value in test['details'].items():
                print(f"    {key}: {value}")
    
    return results


if __name__ == "__main__":
    try:
        print("Script starting...")
        results = main()
        print("Script completed successfully")
    except Exception as e:
        print(f"Script failed with error: {e}")
        import traceback
        traceback.print_exc()