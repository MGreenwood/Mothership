#!/usr/bin/env python3
"""
Service Configuration Verifier for Mothership Analysis

This tool verifies service configuration and dependencies according to task 4.2:
- Check PostgreSQL database setup and connectivity
- Verify environment variable handling
- Test service-to-service communication
- Validate health check endpoints and monitoring
"""

import subprocess
import time
import requests
import json
import os
import sys
import psycopg2
import yaml
from datetime import datetime
from pathlib import Path

class ServiceConfigVerifier:
    def __init__(self, project_root="."):
        self.project_root = Path(project_root).resolve()
        self.evidence_dir = self.project_root / "analysis" / "evidence"
        self.logs_dir = self.project_root / "analysis" / "logs"
        self.evidence_dir.mkdir(parents=True, exist_ok=True)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        
        self.results = {
            "timestamp": datetime.now().isoformat(),
            "postgres_setup": {},
            "environment_variables": {},
            "service_communication": {},
            "health_endpoints": {},
            "configuration_analysis": {},
            "errors": [],
            "warnings": []
        }
    
    def log(self, message, level="INFO"):
        """Log message with timestamp"""
        timestamp = datetime.now().strftime("%H:%M:%S")
        print(f"[{timestamp}] {level}: {message}")
    
    def run_command(self, command, cwd=None, timeout=60):
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
    
    def analyze_configuration_files(self):
        """Analyze configuration files for completeness and correctness"""
        self.log("Analyzing configuration files...")
        
        config_files = [
            ".env",
            "server.config",
            "docker-compose.yml",
            "docker-compose.dev.yml",
            "docker-compose.override.yml"
        ]
        
        config_analysis = {}
        
        for config_file in config_files:
            config_path = self.project_root / config_file
            if config_path.exists():
                try:
                    with open(config_path, 'r', encoding='utf-8') as f:
                        content = f.read()
                    
                    analysis = {
                        "exists": True,
                        "size": len(content),
                        "lines": len(content.splitlines()),
                        "issues": []
                    }
                    
                    # Check for common configuration issues
                    if config_file == ".env":
                        self.analyze_env_file(content, analysis)
                    elif config_file.startswith("docker-compose"):
                        self.analyze_docker_compose(content, analysis, config_file)
                    elif config_file == "server.config":
                        self.analyze_server_config(content, analysis)
                    
                    config_analysis[config_file] = analysis
                    
                except Exception as e:
                    config_analysis[config_file] = {
                        "exists": True,
                        "error": str(e),
                        "issues": [f"Failed to read file: {e}"]
                    }
            else:
                config_analysis[config_file] = {
                    "exists": False,
                    "issues": ["File does not exist"]
                }
        
        self.results["configuration_analysis"] = config_analysis
    
    def analyze_env_file(self, content, analysis):
        """Analyze .env file for required variables and security issues"""
        lines = content.splitlines()
        required_vars = [
            "JWT_SECRET", "ADMIN_SECRET", "ENCRYPTION_KEY",
            "POSTGRES_PASSWORD", "DATABASE_URL",
            "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"
        ]
        
        found_vars = []
        for line in lines:
            if "=" in line and not line.strip().startswith("#"):
                var_name = line.split("=")[0].strip()
                var_value = line.split("=", 1)[1].strip()
                found_vars.append(var_name)
                
                # Check for security issues
                if "secret" in var_name.lower() or "password" in var_name.lower():
                    if len(var_value) < 16:
                        analysis["issues"].append(f"Weak {var_name}: too short")
                    if var_value in ["password", "secret", "changeme", "admin"]:
                        analysis["issues"].append(f"Insecure {var_name}: using default/weak value")
        
        # Check for missing required variables
        for req_var in required_vars:
            if req_var not in found_vars:
                analysis["issues"].append(f"Missing required variable: {req_var}")
        
        analysis["variables_found"] = len(found_vars)
        analysis["required_variables"] = len(required_vars)
    
    def analyze_docker_compose(self, content, analysis, filename):
        """Analyze docker-compose file for service configuration issues"""
        try:
            # Try to parse as YAML
            compose_data = yaml.safe_load(content)
            
            if "services" in compose_data:
                services = compose_data["services"]
                analysis["services"] = list(services.keys())
                
                # Check for common issues
                for service_name, service_config in services.items():
                    if "depends_on" in service_config:
                        deps = service_config["depends_on"]
                        if isinstance(deps, dict):
                            for dep, condition in deps.items():
                                if dep not in services:
                                    analysis["issues"].append(f"Service {service_name} depends on non-existent service {dep}")
                        elif isinstance(deps, list):
                            for dep in deps:
                                if dep not in services:
                                    analysis["issues"].append(f"Service {service_name} depends on non-existent service {dep}")
                    
                    # Check for health checks
                    if "healthcheck" not in service_config and service_name != "tunnel":
                        analysis["issues"].append(f"Service {service_name} missing health check")
                    
                    # Check for exposed ports
                    if "ports" in service_config:
                        ports = service_config["ports"]
                        for port in ports:
                            if ":" not in str(port):
                                analysis["issues"].append(f"Service {service_name} has malformed port mapping: {port}")
            
        except yaml.YAMLError as e:
            analysis["issues"].append(f"Invalid YAML syntax: {e}")
        except Exception as e:
            analysis["issues"].append(f"Analysis error: {e}")
    
    def analyze_server_config(self, content, analysis):
        """Analyze server.config file for configuration issues"""
        lines = content.splitlines()
        sections = {}
        current_section = None
        
        for line in lines:
            line = line.strip()
            if line.startswith("[") and line.endswith("]"):
                current_section = line[1:-1]
                sections[current_section] = {}
            elif "=" in line and current_section:
                key, value = line.split("=", 1)
                sections[current_section][key.strip()] = value.strip()
        
        analysis["sections"] = list(sections.keys())
        
        # Check for required sections
        required_sections = ["server", "features", "auth"]
        for section in required_sections:
            if section not in sections:
                analysis["issues"].append(f"Missing required section: [{section}]")
        
        # Check server configuration
        if "server" in sections:
            server_config = sections["server"]
            if "host" not in server_config:
                analysis["issues"].append("Missing server host configuration")
            if "port" not in server_config:
                analysis["issues"].append("Missing server port configuration")
    
    def test_postgres_setup(self):
        """Test PostgreSQL database setup and connectivity"""
        self.log("Testing PostgreSQL setup...")
        
        # First, try to start just the database service
        self.log("Starting PostgreSQL service...")
        start_db = self.run_command("docker-compose up -d db", timeout=120)
        
        if not start_db["success"]:
            self.results["postgres_setup"]["start_error"] = start_db["stderr"]
            self.results["errors"].append(f"Failed to start PostgreSQL: {start_db['stderr']}")
            return False
        
        # Wait for database to be ready
        self.log("Waiting for PostgreSQL to be ready...")
        time.sleep(10)
        
        # Check if database container is running
        container_status = self.run_command("docker-compose ps db")
        self.results["postgres_setup"]["container_status"] = container_status
        
        if container_status["success"]:
            self.log("PostgreSQL container status:")
            self.log(container_status["stdout"])
        
        # Test database connectivity
        try:
            # Get database connection details from environment
            db_host = "localhost"
            db_port = 5433
            db_name = "mothership"
            db_user = "postgres"
            db_password = "mothership_dev"  # From .env file
            
            self.log(f"Testing database connection to {db_host}:{db_port}")
            
            # Try to connect to the database
            conn = psycopg2.connect(
                host=db_host,
                port=db_port,
                database=db_name,
                user=db_user,
                password=db_password,
                connect_timeout=10
            )
            
            # Test basic operations
            cursor = conn.cursor()
            cursor.execute("SELECT version();")
            version = cursor.fetchone()
            
            cursor.execute("SELECT current_database();")
            current_db = cursor.fetchone()
            
            # Check if migrations have been applied
            cursor.execute("""
                SELECT table_name 
                FROM information_schema.tables 
                WHERE table_schema = 'public'
            """)
            tables = cursor.fetchall()
            
            conn.close()
            
            self.results["postgres_setup"]["connectivity"] = {
                "success": True,
                "version": version[0] if version else "Unknown",
                "database": current_db[0] if current_db else "Unknown",
                "tables": [table[0] for table in tables],
                "table_count": len(tables)
            }
            
            self.log(f"PostgreSQL connection successful - Version: {version[0] if version else 'Unknown'}")
            self.log(f"Database: {current_db[0] if current_db else 'Unknown'}")
            self.log(f"Tables found: {len(tables)}")
            
            return True
            
        except psycopg2.Error as e:
            self.results["postgres_setup"]["connectivity"] = {
                "success": False,
                "error": str(e),
                "error_type": "PostgreSQL Error"
            }
            self.results["errors"].append(f"PostgreSQL connection failed: {e}")
            self.log(f"PostgreSQL connection failed: {e}", "ERROR")
            return False
            
        except Exception as e:
            self.results["postgres_setup"]["connectivity"] = {
                "success": False,
                "error": str(e),
                "error_type": "Connection Error"
            }
            self.results["errors"].append(f"Database connection error: {e}")
            self.log(f"Database connection error: {e}", "ERROR")
            return False
    
    def verify_environment_variables(self):
        """Verify environment variable handling"""
        self.log("Verifying environment variable handling...")
        
        # Check if .env file exists and is readable
        env_file = self.project_root / ".env"
        if env_file.exists():
            try:
                with open(env_file, 'r') as f:
                    env_content = f.read()
                
                # Parse environment variables
                env_vars = {}
                for line in env_content.splitlines():
                    if "=" in line and not line.strip().startswith("#"):
                        key, value = line.split("=", 1)
                        env_vars[key.strip()] = value.strip()
                
                self.results["environment_variables"]["env_file"] = {
                    "exists": True,
                    "variables_count": len(env_vars),
                    "variables": list(env_vars.keys())
                }
                
                # Check for critical variables
                critical_vars = [
                    "DATABASE_URL", "JWT_SECRET", "ADMIN_SECRET",
                    "POSTGRES_PASSWORD", "ENCRYPTION_KEY"
                ]
                
                missing_vars = []
                for var in critical_vars:
                    if var not in env_vars:
                        missing_vars.append(var)
                
                if missing_vars:
                    self.results["environment_variables"]["missing_critical"] = missing_vars
                    self.results["warnings"].append(f"Missing critical environment variables: {missing_vars}")
                
                # Test if Docker Compose can read the variables
                env_test = self.run_command("docker-compose config")
                if env_test["success"]:
                    self.results["environment_variables"]["docker_compose_config"] = {
                        "success": True,
                        "can_parse": True
                    }
                    self.log("Docker Compose can successfully parse configuration with environment variables")
                else:
                    self.results["environment_variables"]["docker_compose_config"] = {
                        "success": False,
                        "error": env_test["stderr"]
                    }
                    self.results["errors"].append(f"Docker Compose config parsing failed: {env_test['stderr']}")
                
            except Exception as e:
                self.results["environment_variables"]["env_file"] = {
                    "exists": True,
                    "error": str(e)
                }
                self.results["errors"].append(f"Failed to read .env file: {e}")
        else:
            self.results["environment_variables"]["env_file"] = {
                "exists": False
            }
            self.results["warnings"].append(".env file does not exist")
    
    def test_service_communication(self):
        """Test service-to-service communication"""
        self.log("Testing service-to-service communication...")
        
        # Start all services
        self.log("Starting all services...")
        start_all = self.run_command("docker-compose up -d", timeout=180)
        
        if not start_all["success"]:
            self.results["service_communication"]["startup"] = {
                "success": False,
                "error": start_all["stderr"]
            }
            self.results["errors"].append(f"Failed to start services: {start_all['stderr']}")
            return False
        
        # Wait for services to start
        self.log("Waiting for services to start...")
        time.sleep(30)
        
        # Check service status
        status_result = self.run_command("docker-compose ps")
        self.results["service_communication"]["service_status"] = status_result
        
        if status_result["success"]:
            self.log("Service status:")
            self.log(status_result["stdout"])
        
        # Test internal network connectivity
        network_tests = [
            {
                "name": "mothership-to-db",
                "command": "docker-compose exec -T mothership ping -c 3 db",
                "description": "Test if mothership service can reach database"
            },
            {
                "name": "db-accessibility",
                "command": "docker-compose exec -T db pg_isready -U postgres",
                "description": "Test if database is ready to accept connections"
            }
        ]
        
        communication_results = {}
        for test in network_tests:
            self.log(f"Running network test: {test['description']}")
            result = self.run_command(test["command"], timeout=30)
            communication_results[test["name"]] = {
                "success": result["success"],
                "command": result["command"],
                "stdout": result["stdout"],
                "stderr": result["stderr"],
                "description": test["description"]
            }
            
            if result["success"]:
                self.log(f"Network test {test['name']} passed")
            else:
                self.log(f"Network test {test['name']} failed: {result['stderr']}", "WARNING")
        
        self.results["service_communication"]["network_tests"] = communication_results
        
        return True
    
    def validate_health_endpoints(self):
        """Validate health check endpoints and monitoring"""
        self.log("Validating health check endpoints...")
        
        # Wait a bit more for services to be fully ready
        time.sleep(15)
        
        health_endpoints = [
            {
                "name": "mothership-health",
                "url": "http://localhost:7523/health",
                "expected_status": [200],
                "timeout": 10
            },
            {
                "name": "mothership-api-root",
                "url": "http://localhost:7523/",
                "expected_status": [200, 404],
                "timeout": 10
            },
            {
                "name": "mothership-web-ui",
                "url": "http://localhost:8080/",
                "expected_status": [200],
                "timeout": 10
            }
        ]
        
        health_results = {}
        for endpoint in health_endpoints:
            try:
                self.log(f"Testing health endpoint: {endpoint['url']}")
                response = requests.get(
                    endpoint["url"], 
                    timeout=endpoint["timeout"]
                )
                
                success = response.status_code in endpoint["expected_status"]
                health_results[endpoint["name"]] = {
                    "success": success,
                    "status_code": response.status_code,
                    "response_time": response.elapsed.total_seconds(),
                    "url": endpoint["url"],
                    "response_body": response.text[:500],  # First 500 chars
                    "headers": dict(response.headers)
                }
                
                if success:
                    self.log(f"Health endpoint {endpoint['name']} is healthy (status: {response.status_code})")
                else:
                    self.log(f"Health endpoint {endpoint['name']} returned unexpected status: {response.status_code}", "WARNING")
                    
            except requests.exceptions.RequestException as e:
                self.log(f"Failed to reach health endpoint {endpoint['name']}: {e}", "ERROR")
                health_results[endpoint["name"]] = {
                    "success": False,
                    "error": str(e),
                    "url": endpoint["url"]
                }
        
        self.results["health_endpoints"] = health_results
        
        # Test Docker health checks
        self.log("Checking Docker container health status...")
        health_status = self.run_command("docker-compose ps --format json")
        if health_status["success"]:
            try:
                containers = json.loads(health_status["stdout"])
                container_health = {}
                for container in containers:
                    container_health[container.get("Service", "unknown")] = {
                        "status": container.get("State", "unknown"),
                        "health": container.get("Health", "unknown")
                    }
                self.results["health_endpoints"]["container_health"] = container_health
            except json.JSONDecodeError:
                self.results["warnings"].append("Could not parse container health status JSON")
    
    def cleanup_services(self):
        """Clean up running services"""
        self.log("Cleaning up services...")
        cleanup = self.run_command("docker-compose down -v", timeout=120)
        if cleanup["success"]:
            self.log("Services cleaned up successfully")
        else:
            self.log(f"Cleanup warning: {cleanup['stderr']}", "WARNING")
    
    def save_results(self):
        """Save verification results to evidence file"""
        evidence_file = self.evidence_dir / "service_configuration_analysis.md"
        
        with open(evidence_file, "w", encoding="utf-8") as f:
            f.write("# Service Configuration Analysis\n\n")
            f.write(f"**Analysis Date:** {self.results['timestamp']}\n\n")
            
            # Summary
            f.write("## Summary\n\n")
            
            # Count successes and failures
            total_tests = 0
            successful_tests = 0
            
            # Count configuration analysis
            if "configuration_analysis" in self.results:
                for config_file, analysis in self.results["configuration_analysis"].items():
                    total_tests += 1
                    if analysis.get("exists") and not analysis.get("issues"):
                        successful_tests += 1
            
            # Count other test categories
            for category in ["postgres_setup", "environment_variables", "service_communication", "health_endpoints"]:
                if category in self.results:
                    category_data = self.results[category]
                    if isinstance(category_data, dict):
                        for key, value in category_data.items():
                            if isinstance(value, dict) and "success" in value:
                                total_tests += 1
                                if value["success"]:
                                    successful_tests += 1
            
            if total_tests > 0:
                success_rate = (successful_tests / total_tests) * 100
                f.write(f"**Overall Success Rate:** {successful_tests}/{total_tests} ({success_rate:.1f}%)\n\n")
            
            # Configuration Analysis
            f.write("## Configuration File Analysis\n\n")
            if "configuration_analysis" in self.results:
                for config_file, analysis in self.results["configuration_analysis"].items():
                    f.write(f"### {config_file}\n\n")
                    f.write(f"**Exists:** {'Yes' if analysis.get('exists') else 'No'}\n")
                    
                    if analysis.get("exists"):
                        if analysis.get("size"):
                            f.write(f"**Size:** {analysis['size']} bytes\n")
                        if analysis.get("lines"):
                            f.write(f"**Lines:** {analysis['lines']}\n")
                        if analysis.get("variables_found"):
                            f.write(f"**Variables Found:** {analysis['variables_found']}\n")
                        if analysis.get("services"):
                            f.write(f"**Services:** {', '.join(analysis['services'])}\n")
                        if analysis.get("sections"):
                            f.write(f"**Sections:** {', '.join(analysis['sections'])}\n")
                    
                    if analysis.get("issues"):
                        f.write(f"**Issues Found:** {len(analysis['issues'])}\n")
                        for issue in analysis["issues"]:
                            f.write(f"- {issue}\n")
                    else:
                        f.write("**Issues:** None\n")
                    
                    f.write("\n")
            
            # PostgreSQL Setup
            f.write("## PostgreSQL Setup Analysis\n\n")
            if "postgres_setup" in self.results:
                postgres = self.results["postgres_setup"]
                
                if "connectivity" in postgres:
                    conn = postgres["connectivity"]
                    f.write(f"**Database Connectivity:** {'SUCCESS' if conn.get('success') else 'FAILED'}\n")
                    if conn.get("success"):
                        f.write(f"- Version: {conn.get('version', 'Unknown')}\n")
                        f.write(f"- Database: {conn.get('database', 'Unknown')}\n")
                        f.write(f"- Tables Found: {conn.get('table_count', 0)}\n")
                        if conn.get("tables"):
                            f.write(f"- Table Names: {', '.join(conn['tables'])}\n")
                    else:
                        f.write(f"- Error: {conn.get('error', 'Unknown error')}\n")
                    f.write("\n")
                
                if "container_status" in postgres:
                    status = postgres["container_status"]
                    f.write(f"**Container Status:** {'SUCCESS' if status.get('success') else 'FAILED'}\n")
                    if status.get("stdout"):
                        f.write("```\n")
                        f.write(status["stdout"][:1000])
                        f.write("\n```\n\n")
            
            # Environment Variables
            f.write("## Environment Variables Analysis\n\n")
            if "environment_variables" in self.results:
                env_vars = self.results["environment_variables"]
                
                if "env_file" in env_vars:
                    env_file = env_vars["env_file"]
                    f.write(f"**.env File:** {'EXISTS' if env_file.get('exists') else 'MISSING'}\n")
                    if env_file.get("exists"):
                        f.write(f"- Variables Count: {env_file.get('variables_count', 0)}\n")
                        if env_file.get("variables"):
                            f.write(f"- Variables: {', '.join(env_file['variables'][:10])}{'...' if len(env_file['variables']) > 10 else ''}\n")
                    f.write("\n")
                
                if "missing_critical" in env_vars:
                    f.write(f"**Missing Critical Variables:** {', '.join(env_vars['missing_critical'])}\n\n")
                
                if "docker_compose_config" in env_vars:
                    config = env_vars["docker_compose_config"]
                    f.write(f"**Docker Compose Config Parsing:** {'SUCCESS' if config.get('success') else 'FAILED'}\n")
                    if not config.get("success") and config.get("error"):
                        f.write(f"- Error: {config['error']}\n")
                    f.write("\n")
            
            # Service Communication
            f.write("## Service Communication Analysis\n\n")
            if "service_communication" in self.results:
                comm = self.results["service_communication"]
                
                if "startup" in comm:
                    startup = comm["startup"]
                    f.write(f"**Service Startup:** {'SUCCESS' if startup.get('success') else 'FAILED'}\n")
                    if not startup.get("success") and startup.get("error"):
                        f.write(f"- Error: {startup['error'][:500]}...\n")
                    f.write("\n")
                
                if "network_tests" in comm:
                    f.write("**Network Connectivity Tests:**\n")
                    for test_name, result in comm["network_tests"].items():
                        f.write(f"- {test_name}: {'PASS' if result.get('success') else 'FAIL'}\n")
                        f.write(f"  - Description: {result.get('description', 'N/A')}\n")
                        if not result.get("success") and result.get("stderr"):
                            f.write(f"  - Error: {result['stderr'][:200]}...\n")
                    f.write("\n")
            
            # Health Endpoints
            f.write("## Health Endpoints Analysis\n\n")
            if "health_endpoints" in self.results:
                health = self.results["health_endpoints"]
                
                for endpoint_name, result in health.items():
                    if endpoint_name == "container_health":
                        f.write("**Container Health Status:**\n")
                        for container, status in result.items():
                            f.write(f"- {container}: {status.get('status', 'unknown')} (Health: {status.get('health', 'unknown')})\n")
                        f.write("\n")
                    elif isinstance(result, dict) and "url" in result:
                        f.write(f"**{endpoint_name}:** {'HEALTHY' if result.get('success') else 'UNHEALTHY'}\n")
                        f.write(f"- URL: {result['url']}\n")
                        if result.get("status_code"):
                            f.write(f"- Status Code: {result['status_code']}\n")
                        if result.get("response_time"):
                            f.write(f"- Response Time: {result['response_time']:.3f}s\n")
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
    
    def run_full_verification(self):
        """Run the complete service configuration verification"""
        self.log("Starting service configuration verification...")
        
        try:
            # Analyze configuration files
            self.analyze_configuration_files()
            
            # Verify environment variables
            self.verify_environment_variables()
            
            # Test PostgreSQL setup
            postgres_success = self.test_postgres_setup()
            
            # Test service communication
            if postgres_success:
                comm_success = self.test_service_communication()
                
                if comm_success:
                    # Validate health endpoints
                    self.validate_health_endpoints()
                else:
                    self.log("Skipping health endpoint tests due to service communication failure", "WARNING")
            else:
                self.log("Skipping service tests due to PostgreSQL setup failure", "WARNING")
            
            return True
            
        except Exception as e:
            self.log(f"Verification failed with exception: {e}", "ERROR")
            self.results["errors"].append(f"Verification exception: {str(e)}")
            return False
        
        finally:
            # Always save results and cleanup
            self.save_results()
            self.cleanup_services()

def main():
    """Main function"""
    if len(sys.argv) > 1:
        project_root = sys.argv[1]
    else:
        project_root = "."
    
    verifier = ServiceConfigVerifier(project_root)
    success = verifier.run_full_verification()
    
    if success:
        print("\nSUCCESS: Service configuration verification completed")
        return 0
    else:
        print("\nERROR: Service configuration verification completed with errors")
        return 1

if __name__ == "__main__":
    sys.exit(main())