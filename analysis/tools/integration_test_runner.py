#!/usr/bin/env python3
"""
Integration test runner for Mothership application.
Executes provided integration tests and analyzes results.
"""

import subprocess
import json
import time
from pathlib import Path
from typing import Dict, Any, List
from dataclasses import dataclass

from logger import analysis_logger
from evidence_collector import evidence_collector, EvidenceType, ConfidenceLevel

@dataclass
class IntegrationTestResult:
    """Represents an integration test execution result."""
    script_name: str
    executed: bool
    passed: bool
    execution_time: float
    output: str
    error_output: str
    return_code: int

class IntegrationTestRunner:
    """Runner for integration tests."""
    
    def __init__(self, config_file: str = "../config/analysis_config.json"):
        self.config = self._load_config(config_file)
        self.test_results: List[IntegrationTestResult] = []
        
    def _load_config(self, config_file: str) -> Dict[str, Any]:
        """Load configuration from file."""
        try:
            with open(config_file, 'r') as f:
                return json.load(f)
        except Exception as e:
            analysis_logger.log_error("config_load", f"Failed to load config: {e}")
            return {}
            
    def run_integration_tests(self) -> Dict[str, Any]:
        """Run all configured integration tests."""
        analysis_logger.log_analysis_start("integration_testing", "all_scripts")
        
        # Get test scripts from config
        test_scripts = self.config.get("integration_testing", {}).get("test_scripts", [])
        
        results = {
            "total_scripts": len(test_scripts),
            "executed_scripts": 0,
            "passed_scripts": 0,
            "failed_scripts": 0,
            "script_results": []
        }
        
        for script_path in test_scripts:
            result = self._run_test_script(script_path)
            self.test_results.append(result)
            
            if result.executed:
                results["executed_scripts"] += 1
                if result.passed:
                    results["passed_scripts"] += 1
                else:
                    results["failed_scripts"] += 1
                    
        # Add detailed results
        results["script_results"] = [
            {
                "script_name": r.script_name,
                "executed": r.executed,
                "passed": r.passed,
                "execution_time": r.execution_time,
                "return_code": r.return_code,
                "output_length": len(r.output),
                "error_length": len(r.error_output)
            }
            for r in self.test_results
        ]
        
        # Collect evidence
        self._collect_evidence_for_tests()
        
        analysis_logger.log_analysis_complete("integration_testing", "all_scripts", results)
        return results
        
    def _run_test_script(self, script_path: str) -> IntegrationTestResult:
        """Run a single integration test script."""
        script_name = Path(script_path).name
        analysis_logger.integration_logger.info(f"Running integration test: {script_name}")
        
        # Check if script exists
        full_script_path = Path(script_path)
        if not full_script_path.exists():
            return IntegrationTestResult(
                script_name=script_name,
                executed=False,
                passed=False,
                execution_time=0.0,
                output="",
                error_output=f"Script not found: {script_path}",
                return_code=-1
            )
            
        try:
            start_time = time.time()
            
            # Determine how to run the script
            if script_path.endswith('.ps1'):
                # PowerShell script
                cmd = ["powershell", "-ExecutionPolicy", "Bypass", "-File", str(full_script_path)]
            elif script_path.endswith('.sh'):
                # Bash script
                cmd = ["bash", str(full_script_path)]
            else:
                # Try to execute directly
                cmd = [str(full_script_path)]
                
            # Run the script with timeout
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300,  # 5 minute timeout
                cwd=full_script_path.parent
            )
            
            execution_time = time.time() - start_time
            
            # Determine if test passed based on return code
            passed = result.returncode == 0
            
            return IntegrationTestResult(
                script_name=script_name,
                executed=True,
                passed=passed,
                execution_time=execution_time,
                output=result.stdout,
                error_output=result.stderr,
                return_code=result.returncode
            )
            
        except subprocess.TimeoutExpired:
            execution_time = time.time() - start_time
            return IntegrationTestResult(
                script_name=script_name,
                executed=True,
                passed=False,
                execution_time=execution_time,
                output="",
                error_output="Test script timed out after 5 minutes",
                return_code=-2
            )
            
        except Exception as e:
            return IntegrationTestResult(
                script_name=script_name,
                executed=False,
                passed=False,
                execution_time=0.0,
                output="",
                error_output=f"Failed to execute script: {str(e)}",
                return_code=-3
            )
            
    def _collect_evidence_for_tests(self):
        """Collect evidence for all integration test results."""
        for test_result in self.test_results:
            if test_result.executed:
                confidence = ConfidenceLevel.HIGH
                description = f"Integration test {test_result.script_name}: {'PASSED' if test_result.passed else 'FAILED'}"
            else:
                confidence = ConfidenceLevel.MEDIUM
                description = f"Integration test {test_result.script_name}: NOT EXECUTED"
                
            evidence_collector.collect_evidence(
                evidence_type=EvidenceType.INTEGRATION_TEST,
                description=description,
                test_result={
                    "script_name": test_result.script_name,
                    "executed": test_result.executed,
                    "passed": test_result.passed,
                    "execution_time": test_result.execution_time,
                    "return_code": test_result.return_code,
                    "output_preview": test_result.output[:500] if test_result.output else "",
                    "error_preview": test_result.error_output[:500] if test_result.error_output else ""
                },
                confidence_level=confidence,
                metadata={
                    "test_category": "integration",
                    "script_name": test_result.script_name
                }
            )
            
    def analyze_docker_deployment(self) -> Dict[str, Any]:
        """Analyze Docker deployment capability."""
        analysis_logger.integration_logger.info("Analyzing Docker deployment")
        
        docker_compose_file = self.config.get("integration_testing", {}).get("docker_compose_file", "../docker-compose.yml")
        
        results = {
            "docker_compose_exists": False,
            "docker_build_successful": False,
            "services_started": False,
            "health_checks_passed": 0,
            "total_health_checks": 0
        }
        
        # Check if docker-compose file exists
        compose_path = Path(docker_compose_file)
        if compose_path.exists():
            results["docker_compose_exists"] = True
            
            # Try to build Docker images
            build_result = self._test_docker_build(compose_path)
            results["docker_build_successful"] = build_result
            
            if build_result:
                # Try to start services
                start_result = self._test_docker_start(compose_path)
                results["services_started"] = start_result
                
                if start_result:
                    # Test health endpoints
                    health_results = self._test_health_endpoints()
                    results["health_checks_passed"] = health_results["passed"]
                    results["total_health_checks"] = health_results["total"]
                    
        return results
        
    def _test_docker_build(self, compose_path: Path) -> bool:
        """Test Docker image building."""
        try:
            result = subprocess.run(
                ["docker", "compose", "-f", str(compose_path), "build"],
                capture_output=True,
                text=True,
                timeout=600,  # 10 minute timeout for building
                cwd=compose_path.parent
            )
            
            return result.returncode == 0
            
        except subprocess.TimeoutExpired:
            analysis_logger.log_error("docker_build", "Docker build timed out after 10 minutes")
            return False
        except Exception as e:
            analysis_logger.log_error("docker_build", f"Docker build failed: {str(e)}", e)
            return False
            
    def _test_docker_start(self, compose_path: Path) -> bool:
        """Test Docker service startup."""
        try:
            # Start services
            result = subprocess.run(
                ["docker", "compose", "-f", str(compose_path), "up", "-d"],
                capture_output=True,
                text=True,
                timeout=120,  # 2 minute timeout for startup
                cwd=compose_path.parent
            )
            
            if result.returncode == 0:
                # Wait a bit for services to start
                time.sleep(10)
                
                # Check if services are running
                status_result = subprocess.run(
                    ["docker", "compose", "-f", str(compose_path), "ps"],
                    capture_output=True,
                    text=True,
                    cwd=compose_path.parent
                )
                
                # Clean up - stop services
                subprocess.run(
                    ["docker", "compose", "-f", str(compose_path), "down"],
                    capture_output=True,
                    cwd=compose_path.parent
                )
                
                return status_result.returncode == 0
            else:
                return False
                
        except subprocess.TimeoutExpired:
            analysis_logger.log_error("docker_start", "Docker startup timed out after 2 minutes")
            return False
        except Exception as e:
            analysis_logger.log_error("docker_start", f"Docker startup failed: {str(e)}", e)
            return False
            
    def _test_health_endpoints(self) -> Dict[str, int]:
        """Test health check endpoints."""
        import requests
        
        health_endpoints = self.config.get("integration_testing", {}).get("health_check_endpoints", [])
        
        results = {"passed": 0, "total": len(health_endpoints)}
        
        for endpoint in health_endpoints:
            try:
                response = requests.get(endpoint, timeout=5)
                if response.status_code == 200:
                    results["passed"] += 1
            except Exception:
                pass  # Health check failed
                
        return results
        
    def generate_report(self) -> str:
        """Generate integration testing report."""
        report = []
        report.append("# Integration Testing Report")
        
        executed = sum(1 for r in self.test_results if r.executed)
        passed = sum(1 for r in self.test_results if r.passed)
        
        report.append(f"**Total Scripts:** {len(self.test_results)}")
        report.append(f"**Executed:** {executed}")
        report.append(f"**Passed:** {passed}")
        report.append(f"**Failed:** {executed - passed}")
        report.append("")
        
        # Script results
        report.append("## Script Results")
        for result in self.test_results:
            status = "✓ PASSED" if result.passed else "✗ FAILED"
            if not result.executed:
                status = "- NOT EXECUTED"
                
            report.append(f"- **{result.script_name}**: {status}")
            if result.executed:
                report.append(f"  - Execution time: {result.execution_time:.2f}s")
                report.append(f"  - Return code: {result.return_code}")
                
        # Failed tests details
        failed_tests = [r for r in self.test_results if r.executed and not r.passed]
        if failed_tests:
            report.append("")
            report.append("## Failed Test Details")
            for result in failed_tests:
                report.append(f"### {result.script_name}")
                report.append(f"**Return Code:** {result.return_code}")
                if result.error_output:
                    report.append("**Error Output:**")
                    report.append("```")
                    report.append(result.error_output[:1000])  # Limit output
                    if len(result.error_output) > 1000:
                        report.append("... (truncated)")
                    report.append("```")
                    
        return "\n".join(report)

def main():
    """Main function to run integration tests."""
    print("Starting Mothership integration testing...")
    
    runner = IntegrationTestRunner()
    results = runner.run_integration_tests()
    
    print(f"\nIntegration testing completed!")
    print(f"Total scripts: {results['total_scripts']}")
    print(f"Executed: {results['executed_scripts']}")
    print(f"Passed: {results['passed_scripts']}")
    print(f"Failed: {results['failed_scripts']}")
    
    # Test Docker deployment
    print("\nTesting Docker deployment...")
    docker_results = runner.analyze_docker_deployment()
    print(f"Docker compose exists: {docker_results['docker_compose_exists']}")
    print(f"Docker build successful: {docker_results['docker_build_successful']}")
    print(f"Services started: {docker_results['services_started']}")
    
    # Generate and save report
    report = runner.generate_report()
    report_path = Path("../reports/integration_testing_report.md")
    
    with open(report_path, 'w') as f:
        f.write(report)
        
    print(f"Report saved to: {report_path}")
    
    # Save detailed results
    results_path = Path("../evidence/integration_tests/test_results.json")
    
    with open(results_path, 'w') as f:
        json.dump(results, f, indent=2)
        
    print(f"Detailed results saved to: {results_path}")

if __name__ == "__main__":
    main()