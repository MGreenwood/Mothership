#!/usr/bin/env python3
"""
Functional testing framework for Mothership application.
Tests actual functionality against advertised claims.
"""

import requests
import websocket
import subprocess
import time
import json
from pathlib import Path
from typing import Dict, Any, List, Optional
from dataclasses import dataclass

from logger import analysis_logger
from evidence_collector import evidence_collector, EvidenceType, ConfidenceLevel

@dataclass
class TestResult:
    """Represents a functional test result."""
    test_name: str
    passed: bool
    description: str
    details: Dict[str, Any]
    evidence: Optional[str] = None

class FunctionalTester:
    """Functional testing framework for Mothership features."""
    
    def __init__(self, config_file: str = "../config/analysis_config.json"):
        self.config = self._load_config(config_file)
        self.test_results: List[TestResult] = []
        
    def _load_config(self, config_file: str) -> Dict[str, Any]:
        """Load configuration from file."""
        try:
            with open(config_file, 'r') as f:
                return json.load(f)
        except Exception as e:
            analysis_logger.log_error("config_load", f"Failed to load config: {e}")
            return {}
            
    def run_all_tests(self) -> Dict[str, Any]:
        """Run all functional tests."""
        analysis_logger.log_analysis_start("functional_testing", "all_features")
        
        # Test categories from config
        test_categories = self.config.get("functional_testing", {}).get("test_categories", [])
        
        results = {
            "total_tests": 0,
            "passed_tests": 0,
            "failed_tests": 0,
            "test_results": []
        }
        
        for category in test_categories:
            if category == "websocket_realtime":
                self._test_websocket_functionality()
            elif category == "daemon_functionality":
                self._test_daemon_functionality()
            elif category == "authentication_flow":
                self._test_authentication_flow()
            elif category == "database_operations":
                self._test_database_operations()
            elif category == "cli_commands":
                self._test_cli_commands()
                
        # Summarize results
        results["total_tests"] = len(self.test_results)
        results["passed_tests"] = sum(1 for r in self.test_results if r.passed)
        results["failed_tests"] = sum(1 for r in self.test_results if not r.passed)
        results["test_results"] = [
            {
                "test_name": r.test_name,
                "passed": r.passed,
                "description": r.description,
                "details": r.details
            }
            for r in self.test_results
        ]
        
        # Collect evidence for all test results
        self._collect_evidence_for_tests()
        
        analysis_logger.log_analysis_complete("functional_testing", "all_features", results)
        return results
        
    def _test_websocket_functionality(self):
        """Test WebSocket real-time collaboration functionality."""
        analysis_logger.test_logger.info("Testing WebSocket functionality")
        
        # Test 1: WebSocket connection
        result = self._test_websocket_connection()
        self.test_results.append(result)
        
        # Test 2: Real-time sync
        result = self._test_realtime_sync()
        self.test_results.append(result)
        
        # Test 3: Conflict resolution
        result = self._test_conflict_resolution()
        self.test_results.append(result)
        
    def _test_websocket_connection(self) -> TestResult:
        """Test basic WebSocket connection."""
        try:
            # Try to connect to WebSocket endpoint
            ws_url = "ws://localhost:8080/ws"  # Assumed endpoint
            
            def on_message(ws, message):
                pass
                
            def on_error(ws, error):
                pass
                
            def on_close(ws, close_status_code, close_msg):
                pass
                
            ws = websocket.WebSocketApp(ws_url,
                                      on_message=on_message,
                                      on_error=on_error,
                                      on_close=on_close)
            
            # Try to connect with timeout
            import threading
            
            connection_successful = False
            
            def run_websocket():
                nonlocal connection_successful
                try:
                    ws.run_forever()
                    connection_successful = True
                except Exception:
                    pass
                    
            thread = threading.Thread(target=run_websocket)
            thread.daemon = True
            thread.start()
            thread.join(timeout=5)
            
            if connection_successful:
                return TestResult(
                    test_name="websocket_connection",
                    passed=True,
                    description="WebSocket connection successful",
                    details={"endpoint": ws_url, "connection_time": "< 5s"}
                )
            else:
                return TestResult(
                    test_name="websocket_connection",
                    passed=False,
                    description="WebSocket connection failed",
                    details={"endpoint": ws_url, "error": "Connection timeout or refused"}
                )
                
        except Exception as e:
            return TestResult(
                test_name="websocket_connection",
                passed=False,
                description="WebSocket connection test failed",
                details={"error": str(e)}
            )
            
    def _test_realtime_sync(self) -> TestResult:
        """Test real-time synchronization."""
        # This would require a more complex setup with multiple clients
        # For now, return a placeholder result
        return TestResult(
            test_name="realtime_sync",
            passed=False,
            description="Real-time sync test not implemented - requires running server",
            details={"status": "requires_server", "test_type": "integration"}
        )
        
    def _test_conflict_resolution(self) -> TestResult:
        """Test conflict resolution in collaborative editing."""
        return TestResult(
            test_name="conflict_resolution",
            passed=False,
            description="Conflict resolution test not implemented - requires running server",
            details={"status": "requires_server", "test_type": "integration"}
        )
        
    def _test_daemon_functionality(self):
        """Test background daemon functionality."""
        analysis_logger.test_logger.info("Testing daemon functionality")
        
        # Test daemon startup
        result = self._test_daemon_startup()
        self.test_results.append(result)
        
        # Test file watching
        result = self._test_file_watching()
        self.test_results.append(result)
        
    def _test_daemon_startup(self) -> TestResult:
        """Test daemon startup process."""
        try:
            # Try to start the daemon
            daemon_path = Path("../../mothership-daemon/target/release/mothership-daemon")
            if not daemon_path.exists():
                daemon_path = Path("../../mothership-daemon/target/debug/mothership-daemon")
                
            if not daemon_path.exists():
                return TestResult(
                    test_name="daemon_startup",
                    passed=False,
                    description="Daemon binary not found",
                    details={"error": "Binary not compiled", "expected_path": str(daemon_path)}
                )
                
            # Try to run daemon with --help to see if it's functional
            result = subprocess.run(
                [str(daemon_path), "--help"],
                capture_output=True,
                text=True,
                timeout=10
            )
            
            if result.returncode == 0:
                return TestResult(
                    test_name="daemon_startup",
                    passed=True,
                    description="Daemon binary is functional",
                    details={"help_output": result.stdout[:200]}
                )
            else:
                return TestResult(
                    test_name="daemon_startup",
                    passed=False,
                    description="Daemon binary failed to run",
                    details={"error": result.stderr, "return_code": result.returncode}
                )
                
        except subprocess.TimeoutExpired:
            return TestResult(
                test_name="daemon_startup",
                passed=False,
                description="Daemon startup timeout",
                details={"error": "Process timed out after 10 seconds"}
            )
        except Exception as e:
            return TestResult(
                test_name="daemon_startup",
                passed=False,
                description="Daemon startup test failed",
                details={"error": str(e)}
            )
            
    def _test_file_watching(self) -> TestResult:
        """Test file watching functionality."""
        return TestResult(
            test_name="file_watching",
            passed=False,
            description="File watching test requires daemon integration",
            details={"status": "requires_daemon_running", "test_type": "integration"}
        )
        
    def _test_authentication_flow(self):
        """Test authentication functionality."""
        analysis_logger.test_logger.info("Testing authentication flow")
        
        # Test OAuth endpoints
        result = self._test_oauth_endpoints()
        self.test_results.append(result)
        
        # Test JWT handling
        result = self._test_jwt_handling()
        self.test_results.append(result)
        
    def _test_oauth_endpoints(self) -> TestResult:
        """Test OAuth authentication endpoints."""
        try:
            # Try to access OAuth endpoints
            base_url = "http://localhost:8080"  # Assumed server URL
            oauth_endpoints = ["/auth/google", "/auth/github", "/auth/callback"]
            
            endpoint_results = {}
            
            for endpoint in oauth_endpoints:
                try:
                    response = requests.get(f"{base_url}{endpoint}", timeout=5)
                    endpoint_results[endpoint] = {
                        "status_code": response.status_code,
                        "accessible": True
                    }
                except requests.exceptions.ConnectionError:
                    endpoint_results[endpoint] = {
                        "status_code": None,
                        "accessible": False,
                        "error": "Connection refused"
                    }
                except Exception as e:
                    endpoint_results[endpoint] = {
                        "status_code": None,
                        "accessible": False,
                        "error": str(e)
                    }
                    
            accessible_endpoints = sum(1 for r in endpoint_results.values() if r["accessible"])
            
            return TestResult(
                test_name="oauth_endpoints",
                passed=accessible_endpoints > 0,
                description=f"OAuth endpoints test: {accessible_endpoints}/{len(oauth_endpoints)} accessible",
                details=endpoint_results
            )
            
        except Exception as e:
            return TestResult(
                test_name="oauth_endpoints",
                passed=False,
                description="OAuth endpoints test failed",
                details={"error": str(e)}
            )
            
    def _test_jwt_handling(self) -> TestResult:
        """Test JWT token handling."""
        return TestResult(
            test_name="jwt_handling",
            passed=False,
            description="JWT handling test requires authentication server",
            details={"status": "requires_server", "test_type": "integration"}
        )
        
    def _test_database_operations(self):
        """Test database functionality."""
        analysis_logger.test_logger.info("Testing database operations")
        
        # Test database connection
        result = self._test_database_connection()
        self.test_results.append(result)
        
    def _test_database_connection(self) -> TestResult:
        """Test database connection."""
        try:
            # Try to connect to PostgreSQL
            import psycopg2
            
            # Default connection parameters
            conn_params = {
                "host": "localhost",
                "port": 3333,
                "database": "mothership",
                "user": "postgres",
                "password": "password"
            }
            
            conn = psycopg2.connect(**conn_params)
            conn.close()
            
            return TestResult(
                test_name="database_connection",
                passed=True,
                description="Database connection successful",
                details=conn_params
            )
            
        except psycopg2.OperationalError as e:
            return TestResult(
                test_name="database_connection",
                passed=False,
                description="Database connection failed",
                details={"error": str(e), "connection_params": conn_params}
            )
        except ImportError:
            return TestResult(
                test_name="database_connection",
                passed=False,
                description="Database driver not available",
                details={"error": "psycopg2 not installed"}
            )
        except Exception as e:
            return TestResult(
                test_name="database_connection",
                passed=False,
                description="Database connection test failed",
                details={"error": str(e)}
            )
            
    def _test_cli_commands(self):
        """Test CLI command functionality."""
        analysis_logger.test_logger.info("Testing CLI commands")
        
        # Test CLI binary
        result = self._test_cli_binary()
        self.test_results.append(result)
        
        # Test CLI commands
        result = self._test_cli_help()
        self.test_results.append(result)
        
    def _test_cli_binary(self) -> TestResult:
        """Test CLI binary existence and functionality."""
        try:
            cli_path = Path("../../mothership-cli/target/release/mothership-cli")
            if not cli_path.exists():
                cli_path = Path("../../mothership-cli/target/debug/mothership-cli")
                
            if not cli_path.exists():
                return TestResult(
                    test_name="cli_binary",
                    passed=False,
                    description="CLI binary not found",
                    details={"error": "Binary not compiled", "expected_path": str(cli_path)}
                )
                
            return TestResult(
                test_name="cli_binary",
                passed=True,
                description="CLI binary exists",
                details={"path": str(cli_path)}
            )
            
        except Exception as e:
            return TestResult(
                test_name="cli_binary",
                passed=False,
                description="CLI binary test failed",
                details={"error": str(e)}
            )
            
    def _test_cli_help(self) -> TestResult:
        """Test CLI help command."""
        try:
            cli_path = Path("../../mothership-cli/target/release/mothership-cli")
            if not cli_path.exists():
                cli_path = Path("../../mothership-cli/target/debug/mothership-cli")
                
            if not cli_path.exists():
                return TestResult(
                    test_name="cli_help",
                    passed=False,
                    description="CLI binary not found for help test",
                    details={"error": "Binary not compiled"}
                )
                
            result = subprocess.run(
                [str(cli_path), "--help"],
                capture_output=True,
                text=True,
                timeout=10
            )
            
            if result.returncode == 0:
                return TestResult(
                    test_name="cli_help",
                    passed=True,
                    description="CLI help command works",
                    details={"help_output": result.stdout[:200]}
                )
            else:
                return TestResult(
                    test_name="cli_help",
                    passed=False,
                    description="CLI help command failed",
                    details={"error": result.stderr, "return_code": result.returncode}
                )
                
        except subprocess.TimeoutExpired:
            return TestResult(
                test_name="cli_help",
                passed=False,
                description="CLI help command timeout",
                details={"error": "Process timed out after 10 seconds"}
            )
        except Exception as e:
            return TestResult(
                test_name="cli_help",
                passed=False,
                description="CLI help test failed",
                details={"error": str(e)}
            )
            
    def _collect_evidence_for_tests(self):
        """Collect evidence for all test results."""
        for test_result in self.test_results:
            confidence = ConfidenceLevel.HIGH if test_result.passed else ConfidenceLevel.MEDIUM
            
            evidence_collector.collect_evidence(
                evidence_type=EvidenceType.FUNCTIONAL_TEST,
                description=f"Functional test: {test_result.description}",
                test_result={
                    "test_name": test_result.test_name,
                    "passed": test_result.passed,
                    "details": test_result.details
                },
                confidence_level=confidence,
                metadata={
                    "test_category": "functional",
                    "test_name": test_result.test_name
                }
            )
            
    def generate_report(self) -> str:
        """Generate a functional testing report."""
        report = []
        report.append("# Functional Testing Report")
        report.append(f"**Total Tests:** {len(self.test_results)}")
        
        passed = sum(1 for r in self.test_results if r.passed)
        failed = len(self.test_results) - passed
        
        report.append(f"**Passed:** {passed}")
        report.append(f"**Failed:** {failed}")
        report.append("")
        
        # Test results by category
        categories = {}
        for result in self.test_results:
            category = result.test_name.split('_')[0]
            if category not in categories:
                categories[category] = {"passed": 0, "failed": 0}
            if result.passed:
                categories[category]["passed"] += 1
            else:
                categories[category]["failed"] += 1
                
        report.append("## Results by Category")
        for category, counts in categories.items():
            total = counts["passed"] + counts["failed"]
            report.append(f"- **{category}**: {counts['passed']}/{total} passed")
            
        report.append("")
        report.append("## Failed Tests")
        for result in self.test_results:
            if not result.passed:
                report.append(f"- **{result.test_name}**: {result.description}")
                
        return "\n".join(report)

def main():
    """Main function to run functional tests."""
    print("Starting Mothership functional testing...")
    
    tester = FunctionalTester()
    results = tester.run_all_tests()
    
    print(f"\nFunctional testing completed!")
    print(f"Total tests: {results['total_tests']}")
    print(f"Passed: {results['passed_tests']}")
    print(f"Failed: {results['failed_tests']}")
    
    # Generate and save report
    report = tester.generate_report()
    report_path = Path("../reports/functional_testing_report.md")
    
    with open(report_path, 'w') as f:
        f.write(report)
        
    print(f"Report saved to: {report_path}")
    
    # Save detailed results
    results_path = Path("../evidence/functional_tests/test_results.json")
    
    with open(results_path, 'w') as f:
        json.dump(results, f, indent=2)
        
    print(f"Detailed results saved to: {results_path}")

if __name__ == "__main__":
    main()