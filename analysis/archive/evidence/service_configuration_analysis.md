# Service Configuration Analysis

**Analysis Date:** 2025-08-15T19:15:48.078146

## Summary

**Overall Success Rate:** 10/12 (83.3%)

## Configuration File Analysis

### .env

**Exists:** Yes
**Size:** 1169 bytes
**Lines:** 34
**Variables Found:** 13
**Issues Found:** 1
- Weak POSTGRES_PASSWORD: too short

### server.config

**Exists:** Yes
**Size:** 795 bytes
**Lines:** 38
**Sections:** server, features, auth, collaboration, cli_distribution
**Issues:** None

### docker-compose.yml

**Exists:** Yes
**Size:** 2190 bytes
**Lines:** 78
**Services:** mothership, tunnel, db
**Issues:** None

### docker-compose.dev.yml

**Exists:** Yes
**Size:** 1657 bytes
**Lines:** 64
**Services:** mothership, db
**Issues:** None

### docker-compose.override.yml

**Exists:** Yes
**Size:** 386 bytes
**Lines:** 9
**Services:** mothership
**Issues Found:** 1
- Service mothership missing health check

## PostgreSQL Setup Analysis

**Database Connectivity:** SUCCESS
- Version: PostgreSQL 13.21 on x86_64-pc-linux-musl, compiled by gcc (Alpine 14.2.0) 14.2.0, 64-bit
- Database: mothership
- Tables Found: 6
- Table Names: projects, project_members, users, rifts, rift_collaborators, project_settings

**Container Status:** SUCCESS
```
NAME              IMAGE                COMMAND                  SERVICE   CREATED          STATUS                   PORTS
mothership-db-1   postgres:13-alpine   "docker-entrypoint.sâ€¦"   db        10 seconds ago   Up 8 seconds (healthy)   0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp

```

## Environment Variables Analysis

**.env File:** EXISTS
- Variables Count: 13
- Variables: MOTHERSHIP_PORT, RUST_LOG, JWT_SECRET, ADMIN_SECRET, ENCRYPTION_KEY, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, GITHUB_CLIENT_ID, GITHUB_CLIENT_SECRET, POSTGRES_PASSWORD...

**Docker Compose Config Parsing:** SUCCESS

## Service Communication Analysis

**Network Connectivity Tests:**
- mothership-to-db: FAIL
  - Description: Test if mothership service can reach database
- db-accessibility: PASS
  - Description: Test if database is ready to accept connections

## Health Endpoints Analysis

**mothership-health:** HEALTHY
- URL: http://localhost:7523/health
- Status Code: 200
- Response Time: 0.021s

**mothership-api-root:** HEALTHY
- URL: http://localhost:7523/
- Status Code: 404
- Response Time: 0.015s

**mothership-web-ui:** HEALTHY
- URL: http://localhost:8080/
- Status Code: 200
- Response Time: 0.014s

## Warnings

- WARNING: Could not parse container health status JSON

## Raw Test Results

```json
{
  "timestamp": "2025-08-15T19:15:48.078146",
  "postgres_setup": {
    "container_status": {
      "success": true,
      "returncode": 0,
      "stdout": "NAME              IMAGE                COMMAND                  SERVICE   CREATED          STATUS                   PORTS\nmothership-db-1   postgres:13-alpine   \"docker-entrypoint.s\u00e2\u20ac\u00a6\"   db        10 seconds ago   Up 8 seconds (healthy)   0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp\n",
      "stderr": "",
      "command": "docker-compose ps db"
    },
    "connectivity": {
      "success": true,
      "version": "PostgreSQL 13.21 on x86_64-pc-linux-musl, compiled by gcc (Alpine 14.2.0) 14.2.0, 64-bit",
      "database": "mothership",
      "tables": [
        "projects",
        "project_members",
        "users",
        "rifts",
        "rift_collaborators",
        "project_settings"
      ],
      "table_count": 6
    }
  },
  "environment_variables": {
    "env_file": {
      "exists": true,
      "variables_count": 13,
      "variables": [
        "MOTHERSHIP_PORT",
        "RUST_LOG",
        "JWT_SECRET",
        "ADMIN_SECRET",
        "ENCRYPTION_KEY",
        "GOOGLE_CLIENT_ID",
        "GOOGLE_CLIENT_SECRET",
        "GITHUB_CLIENT_ID",
        "GITHUB_CLIENT_SECRET",
        "POSTGRES_PASSWORD",
        "POSTGRES_PORT",
        "DATABASE_URL",
        "OAUTH_BASE_URL"
      ]
    },
    "docker_compose_config": {
      "success": true,
      "can_parse": true
    }
  },
  "service_communication": {
    "service_status": {
      "success": true,
      "returncode": 0,
      "stdout": "NAME                      IMAGE                           COMMAND                  SERVICE      CREATED          STATUS                    PORTS\nmothership-db-1           postgres:13-alpine              \"docker-entrypoint.s\u00e2\u20ac\u00a6\"   db           42 seconds ago   Up 40 seconds (healthy)   0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp\nmothership-mothership-1   mothership-mothership           \"./mothership-server\"    mothership   31 seconds ago   Up 30 seconds (healthy)   0.0.0.0:7523->7523/tcp, [::]:7523->7523/tcp, 0.0.0.0:8080->8080/tcp, [::]:8080->8080/tcp\nmothership-tunnel-1       cloudflare/cloudflared:latest   \"cloudflared --no-au\u00e2\u20ac\u00a6\"   tunnel       31 seconds ago   Up 30 seconds             \n",
      "stderr": "",
      "command": "docker-compose ps"
    },
    "network_tests": {
      "mothership-to-db": {
        "success": false,
        "command": "docker-compose exec -T mothership ping -c 3 db",
        "stdout": "OCI runtime exec failed: exec failed: unable to start container process: exec: \"ping\": executable file not found in $PATH: unknown\n",
        "stderr": "",
        "description": "Test if mothership service can reach database"
      },
      "db-accessibility": {
        "success": true,
        "command": "docker-compose exec -T db pg_isready -U postgres",
        "stdout": "/var/run/postgresql:5432 - accepting connections\n",
        "stderr": "",
        "description": "Test if database is ready to accept connections"
      }
    }
  },
  "health_endpoints": {
    "mothership-health": {
      "success": true,
      "status_code": 200,
      "response_time": 0.020778,
      "url": "http://localhost:7523/health",
      "response_body": "{\"success\":true,\"data\":\"Mothership is operational\",\"error\":null,\"message\":null}",
      "headers": {
        "content-type": "application/json",
        "vary": "origin, access-control-request-method, access-control-request-headers",
        "access-control-allow-origin": "https://app.mothership.rest",
        "access-control-allow-credentials": "true",
        "content-length": "79",
        "date": "Fri, 15 Aug 2025 23:16:45 GMT"
      }
    },
    "mothership-api-root": {
      "success": true,
      "status_code": 404,
      "response_time": 0.014871,
      "url": "http://localhost:7523/",
      "response_body": "",
      "headers": {
        "vary": "origin, access-control-request-method, access-control-request-headers",
        "access-control-allow-origin": "https://app.mothership.rest",
        "access-control-allow-credentials": "true",
        "content-length": "0",
        "date": "Fri, 15 Aug 2025 23:16:45 GMT"
      }
    },
    "mothership-web-ui": {
      "success": true,
      "status_code": 200,
      "response_time": 0.014153,
      "url": "http://localhost:8080/",
      "response_body": "\n<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n    <meta charset=\"UTF-8\">\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n    <title>Mothership Server</title>\n    <link rel=\"icon\" type=\"image/png\" href=\"/static/icon.png\">\n    <style>\n        body {\n            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;\n            line-height: 1.6;\n            margin: 0;\n            padding: 2rem;\n            background: linear-gradient(135deg, #667eea 0%",
      "headers": {
        "content-type": "text/html; charset=utf-8",
        "content-length": "4987",
        "date": "Fri, 15 Aug 2025 23:16:45 GMT"
      }
    }
  },
  "configuration_analysis": {
    ".env": {
      "exists": true,
      "size": 1169,
      "lines": 34,
      "issues": [
        "Weak POSTGRES_PASSWORD: too short"
      ],
      "variables_found": 13,
      "required_variables": 7
    },
    "server.config": {
      "exists": true,
      "size": 795,
      "lines": 38,
      "issues": [],
      "sections": [
        "server",
        "features",
        "auth",
        "collaboration",
        "cli_distribution"
      ]
    },
    "docker-compose.yml": {
      "exists": true,
      "size": 2190,
      "lines": 78,
      "issues": [],
      "services": [
        "mothership",
        "tunnel",
        "db"
      ]
    },
    "docker-compose.dev.yml": {
      "exists": true,
      "size": 1657,
      "lines": 64,
      "issues": [],
      "services": [
        "mothership",
        "db"
      ]
    },
    "docker-compose.override.yml": {
      "exists": true,
      "size": 386,
      "lines": 9,
      "issues": [
        "Service mothership missing health check"
      ],
      "services": [
        "mothership"
      ]
    }
  },
  "errors": [],
  "warnings": [
    "Could not parse container health status JSON"
  ]
}
```
