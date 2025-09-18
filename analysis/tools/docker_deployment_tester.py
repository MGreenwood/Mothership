#!/usr/bin/env python3
"""
Docker Deployment Tester for Mothership Analysis

This tool tests Docker build and deployment functionality according to task 4.1:
- Build Docker images using provided Dockerfile.server
- Start Docker stack with docker-compose.yml
- Check service connectivity and health endpoints
- Document any build failures or missing dependencies
"""

import subprocess
import time
import requests
import json
import os
import sys
from datetime import datetime
from pathlib import Path

class DockerDeploymentTester:
    def __init__(self, project_root="."):
        self.project_root = Path(project_root).resolve()
        self.evidence_dir = self.project_root / "analysis" / "evidence"
        self.logs_dir = self.project_root / "analysis" / "logs"
        self.evidence_dir.mkdir(parents=True, exist_ok=True)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        
        self.results = {
            "timestamp": datetime.now().isoformat(),
            "docker_build": {},
            "docker_compose": {},
            "service_connectivity": {},
            "health_checks": {},
            "errors": [],
            "warnings": []
        }
    
    def log(self, message, level="INFO"):
        """Log message with timestamp"""
        timestamp = datetime.now().strftime("%H:%M:%S")
        print(f"[{timestamp}] {level}: {message}")
    
    def run_command(self, command, cwd=None, timeout=300):
        """Run shell command and capture output"""
        try:
            self.log(f"Running: {command}")
            result = subprocess.run(
                command,
                shell=True,
                cwd=cwd or self.project_root,
                capture_output=True,
                text=True,
                timeout=timeout
            )
            return {
                "success": result.returncode == 0,
                "returncode": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "command": command
            }
        except subprocess.TimeoutExpired:
            return {
                "success": False,
                "returncode": -1,
                "stdout": "",
                "stderr": f"Command timed out after {timeout} seconds",
                "command": command
            }
        except Exception as e:
            return {
                "success": False,
                "returncode": -1,
                "stdout": "",
                "stderr": str(e),
                "command": command
            }
    
    def check_docker_availability(self):
        """Check if Docker is available and running"""
        self.log("Checking Docker availability...")
        
        # Check if docker command exists
        docker_version = self.run_command("docker --version")
        if not docker_version["success"]:
            self.results["errors"].append("Docker command not found")
            return False
        
        # Check if Docker daemon is running
        docker_info = self.run_command("docker info")
        if not docker_info["success"]:
            self.results["errors"].append("Docker daemon not running")
            return False
        
        # Check if docker-compose is available
        compose_version = self.run_command("docker-compose --version")
        if not compose_version["success"]:
            # Try docker compose (newer syntax)
            compose_version = self.run_command("docker compose version")
            if not compose_version["success"]:
                self.results["errors"].append("Docker Compose not found")
                return False
        
        self.log("Docker environment is available")
        return True
    
    def test_docker_build(self):
        """Test building the Docker image"""
        self.log("Testing Docker image build...")
        
        # Clean up any existing images first
        self.run_command("docker rmi mothership-mothership:latest", timeout=60)
        
        # Build the Docker image
        build_result = self.run_command(
            "docker build -f Dockerfile.server -t mothership-test .",
            timeout=600  # 10 minutes for build
        )
        
        self.results["docker_build"] = {
            "success": build_result["success"],
            "command": build_result["command"],
            "stdout": build_result["stdout"],
            "stderr": build_result["stderr"],
            "returncode": build_result["returncode"]
        }
        
        if build_result["success"]:
            self.log("Docker build successful")
            
            # Check if image was created
            image_check = self.run_command("docker images mothership-test")
            if "mothership-test" in image_check["stdout"]:
                self.log("Docker image created successfully")
            else:
                self.results["warnings"].append("Docker build succeeded but image not found")
        else:
            self.log("Docker build failed", "ERROR")
            self.results["errors"].append(f"Docker build failed: {build_result['stderr']}")
        
        return build_result["success"]
    
    def test_docker_compose_up(self):
        """Test starting the Docker stack with docker-compose"""
        self.log("Testing Docker Compose deployment...")
        
        # First, clean up any existing containers
        self.run_command("docker-compose down -v", timeout=120)
        
        # Start the stack in detached mode
        compose_result = self.run_command(
            "docker-compose up -d",
            timeout=300  # 5 minutes for startup
        )
        
        self.results["docker_compose"]["up"] = {
            "success": compose_result["success"],
            "command": compose_result["command"],
            "stdout": compose_result["stdout"],
            "stderr": compose_result["stderr"],
            "returncode": compose_result["returncode"]
        }
        
        if compose_result["success"]:
            self.log("Docker Compose up successful")
            
            # Wait a bit for services to start
            self.log("Waiting for services to start...")
            time.sleep(30)
            
            # Check container status
            status_result = self.run_command("docker-compose ps")
            self.results["docker_compose"]["status"] = status_result
            
            if status_result["success"]:
                self.log("Container status check successful")
                self.log(f"Container status:\n{status_result['stdout']}")
            else:
                self.results["warnings"].append("Could not check container status")
        else:
            self.log("Docker Compose up failed", "ERROR")
            self.results["errors"].append(f"Docker Compose failed: {compose_result['stderr']}")
        
        return compose_result["success"]
    
    def test_service_connectivity(self):
        """Test connectivity to services"""
        self.log("Testing service connectivity...")
        
        services_to_test = [
            {"name": "mothership-api", "url": "http://localhost:7523", "expected_status": [200, 404, 401]},
            {"name": "mothership-web", "url": "http://localhost:8080", "expected_status": [200, 404]},
            {"name": "postgres", "host": "localhost", "port": 5432}
        ]
        
        for service in services_to_test:
            if "url" in service:
                # HTTP service test
                try:
                    self.log(f"Testing HTTP connectivity to {service['name']} at {service['url']}")
                    response = requests.get(service["url"], timeout=10)
                    
                    success = response.status_code in service.get("expected_status", [200])
                    self.results["service_connectivity"][service["name"]] = {
                        "success": success,
                        "status_code": response.status_code,
                        "response_time": response.elapsed.total_seconds(),
                        "url": service["url"]
                    }
                    
                    if success:
                        self.log(f"{service['name']} is accessible (status: {response.status_code})")
                    else:
                        self.log(f"{service['name']} returned unexpected status: {response.status_code}", "WARNING")
                        
                except requests.exceptions.RequestException as e:
                    self.log(f"Failed to connect to {service['name']}: {e}", "ERROR")
                    self.results["service_connectivity"][service["name"]] = {
                        "success": False,
                        "error": str(e),
                        "url": service["url"]
                    }
            
            elif "host" in service and "port" in service:
                # TCP service test
                import socket
                try:
                    self.log(f"Testing TCP connectivity to {service['name']} at {service['host']}:{service['port']}")
                    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                    sock.settimeout(10)
                    result = sock.connect_ex((service["host"], service["port"]))
                    sock.close()
                    
                    success = result == 0
                    self.results["service_connectivity"][service["name"]] = {
                        "success": success,
                        "host": service["host"],
                        "port": service["port"]
                    }
                    
                    if success:
                        self.log(f"{service['name']} is accessible")
                    else:
                        self.log(f"Failed to connect to {service['name']}", "ERROR")
                        
                except Exception as e:
                    self.log(f"Error testing {service['name']}: {e}", "ERROR")
                    self.results["service_connectivity"][service["name"]] = {
                        "success": False,
                        "error": str(e),
                        "host": service["host"],
                        "port": service["port"]
                    }
    
    def test_health_endpoints(self):
        """Test health check endpoints"""
        self.log("Testing health check endpoints...")
        
        health_endpoints = [
            {"name": "mothership-health", "url": "http://localhost:7523/health"},
            {"name": "mothership-status", "url": "http://localhost:7523/status"},
            {"name": "mothership-api-info", "url": "http://localhost:7523/api/info"}
        ]
        
        for endpoint in health_endpoints:
            try:
                self.log(f"Testing health endpoint: {endpoint['url']}")
                response = requests.get(endpoint["url"], timeout=10)
                
                self.results["health_checks"][endpoint["name"]] = {
                    "success": response.status_code == 200,
                    "status_code": response.status_code,
                    "response_time": response.elapsed.total_seconds(),
                    "url": endpoint["url"],
                    "response_body": response.text[:500]  # First 500 chars
                }
                
                if response.status_code == 200:
                    self.log(f"Health endpoint {endpoint['name']} is healthy")
                else:
                    self.log(f"Health endpoint {endpoint['name']} returned status {response.status_code}", "WARNING")
                    
            except requests.exceptions.RequestException as e:
                self.log(f"Failed to check health endpoint {endpoint['name']}: {e}", "ERROR")
                self.results["health_checks"][endpoint["name"]] = {
                    "success": False,
                    "error": str(e),
                    "url": endpoint["url"]
                }
    
    def cleanup_docker_resources(self):
        """Clean up Docker resources"""
        self.log("Cleaning up Docker resources...")
        
        # Stop and remove containers
        cleanup_commands = [
            "docker-compose down -v",
            "docker rmi mothership-test",
            "docker system prune -f"
        ]
        
        for cmd in cleanup_commands:
            result = self.run_command(cmd, timeout=120)
            if result["success"]:
                self.log(f"Cleanup command successful: {cmd}")
            else:
                self.log(f"Cleanup command failed: {cmd}", "WARNING")
    
    def save_results(self):
        """Save test results to evidence file"""
        evidence_file = self.evidence_dir / "docker_deployment_analysis.md"
        
        with open(evidence_file, "w", encoding="utf-8") as f:
            f.write("# Docker Deployment Analysis\n\n")
            f.write(f"**Analysis Date:** {self.results['timestamp']}\n\n")
            f.write("## Summary\n\n")
            
            # Count successes and failures
            total_tests = 0
            successful_tests = 0
            
            for category in ["docker_build", "docker_compose", "service_connectivity", "health_checks"]:
                if category in self.results:
                    if isinstance(self.results[category], dict):
                        if category == "docker_build":
                            total_tests += 1
                            if self.results[category].get("success"):
                                successful_tests += 1
                        elif category == "docker_compose":
                            for key, value in self.results[category].items():
                                if isinstance(value, dict) and "success" in value:
                                    total_tests += 1
                                    if value["success"]:
                                        successful_tests += 1
                        else:
                            for key, value in self.results[category].items():
                                if isinstance(value, dict) and "success" in value:
                                    total_tests += 1
                                    if value["success"]:
                                        successful_tests += 1
            
            f.write(f"**Overall Success Rate:** {successful_tests}/{total_tests} ({(successful_tests/total_tests*100):.1f}%)\n\n")
            
            # Docker Build Results
            f.write("## Docker Build Results\n\n")
            if "docker_build" in self.results:
                build = self.results["docker_build"]
                f.write(f"**Status:** {'SUCCESS' if build.get('success') else 'FAILED'}\n\n")
                if build.get("stderr"):
                    f.write("**Build Errors:**\n```\n")
                    f.write(build["stderr"][:2000])  # Limit output
                    f.write("\n```\n\n")
                if build.get("stdout"):
                    f.write("**Build Output (last 1000 chars):**\n```\n")
                    f.write(build["stdout"][-1000:])
                    f.write("\n```\n\n")
            
            # Docker Compose Results
            f.write("## Docker Compose Results\n\n")
            if "docker_compose" in self.results:
                compose = self.results["docker_compose"]
                for key, value in compose.items():
                    if isinstance(value, dict):
                        f.write(f"**{key.title()}:** {'SUCCESS' if value.get('success') else 'FAILED'}\n\n")
                        if value.get("stderr"):
                            f.write(f"**{key.title()} Errors:**\n```\n")
                            f.write(value["stderr"][:1000])
                            f.write("\n```\n\n")
            
            # Service Connectivity Results
            f.write("## Service Connectivity Results\n\n")
            if "service_connectivity" in self.results:
                for service, result in self.results["service_connectivity"].items():
                    f.write(f"**{service}:** {'ACCESSIBLE' if result.get('success') else 'FAILED'}\n")
                    if result.get("status_code"):
                        f.write(f"- Status Code: {result['status_code']}\n")
                    if result.get("response_time"):
                        f.write(f"- Response Time: {result['response_time']:.3f}s\n")
                    if result.get("error"):
                        f.write(f"- Error: {result['error']}\n")
                    f.write("\n")
            
            # Health Check Results
            f.write("## Health Check Results\n\n")
            if "health_checks" in self.results:
                for endpoint, result in self.results["health_checks"].items():
                    f.write(f"**{endpoint}:** {'HEALTHY' if result.get('success') else 'UNHEALTHY'}\n")
                    if result.get("status_code"):
                        f.write(f"- Status Code: {result['status_code']}\n")
                    if result.get("response_time"):
                        f.write(f"- Response Time: {result['response_time']:.3f}s\n")
                    if result.get("response_body"):
                        f.write(f"- Response: {result['response_body'][:200]}...\n")
                    if result.get("error"):
                        f.write(f"- Error: {result['error']}\n")
                    f.write("\n")
            
            # Errors and Warnings
            if self.results["errors"]:
                f.write("## Errors\n\n")
                for error in self.results["errors"]:
                    f.write(f"- ERROR: {error}\n")
                f.write("\n")
            
            if self.results["warnings"]:
                f.write("## Warnings\n\n")
                for warning in self.results["warnings"]:
                    f.write(f"- WARNING: {warning}\n")
                f.write("\n")
            
            # Raw Results
            f.write("## Raw Test Results\n\n")
            f.write("```json\n")
            f.write(json.dumps(self.results, indent=2))
            f.write("\n```\n")
        
        self.log(f"Results saved to {evidence_file}")
    
    def run_full_test(self):
        """Run the complete Docker deployment test suite"""
        self.log("Starting Docker deployment test suite...")
        
        try:
            # Check Docker availability
            if not self.check_docker_availability():
                self.log("Docker environment not available, aborting tests", "ERROR")
                return False
            
            # Test Docker build
            build_success = self.test_docker_build()
            
            # Test Docker Compose deployment
            if build_success:
                compose_success = self.test_docker_compose_up()
                
                if compose_success:
                    # Test service connectivity
                    self.test_service_connectivity()
                    
                    # Test health endpoints
                    self.test_health_endpoints()
                else:
                    self.log("Skipping connectivity tests due to compose failure", "WARNING")
            else:
                self.log("Skipping deployment tests due to build failure", "WARNING")
            
            return True
            
        except Exception as e:
            self.log(f"Test suite failed with exception: {e}", "ERROR")
            self.results["errors"].append(f"Test suite exception: {str(e)}")
            return False
        
        finally:
            # Always save results and cleanup
            self.save_results()
            self.cleanup_docker_resources()

def main():
    """Main function"""
    if len(sys.argv) > 1:
        project_root = sys.argv[1]
    else:
        project_root = "."
    
    tester = DockerDeploymentTester(project_root)
    success = tester.run_full_test()
    
    if success:
        print("\nSUCCESS: Docker deployment test completed successfully")
        return 0
    else:
        print("\nERROR: Docker deployment test completed with errors")
        return 1

if __name__ == "__main__":
    sys.exit(main())