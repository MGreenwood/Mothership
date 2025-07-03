#!/bin/bash

# Mothership Integration Test Script
# This script tests most features of the Mothership CLI in sequence

set -e  # Exit on any error
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color
CHECKMARK="\xE2\x9C\x94"
XMARK="\xE2\x9C\x98"

# Test project settings
TEST_PROJECT="test-project-$(date +%s)"
TEST_RIFT="feature-test"
SERVER_URL="http://localhost:7523"  # Change this to your server URL

log_step() {
    echo -e "\n${YELLOW}==== $1 ====${NC}"
}

log_success() {
    echo -e "${GREEN}${CHECKMARK} $1${NC}"
}

log_error() {
    echo -e "${RED}${XMARK} $1${NC}"
    exit 1
}

check_command() {
    if ! command -v $1 &> /dev/null; then
        log_error "$1 could not be found. Please install it first."
    fi
}

# Check prerequisites
log_step "Checking prerequisites"
check_command "mothership"
check_command "curl"
log_success "All prerequisites found"

# Test server connection
log_step "Testing server connection"
curl -s "$SERVER_URL/health" > /dev/null || log_error "Server is not running at $SERVER_URL"
log_success "Server is running"

# Clean up any existing connection
log_step "Cleaning up existing connections"
mothership server disconnect || true
mothership logout || true
log_success "Environment cleaned"

# Connect to server
log_step "Connecting to server"
mothership connect "$SERVER_URL" || log_error "Failed to connect to server"
mothership server status | grep "Connected" || log_error "Server connection failed"
log_success "Connected to server"

# Authenticate using OAuth
log_step "Authenticating"
mothership auth google || log_error "Authentication failed"
log_success "Authentication successful"

# List existing projects
log_step "Listing projects (pre-creation)"
mothership gateway list || log_error "Failed to list projects"
log_success "Project listing successful"

# Create test directory
log_step "Creating test project directory"
TEST_DIR="/tmp/$TEST_PROJECT"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"
cd "$TEST_DIR"

# Initialize test project
log_step "Initializing test project"
echo "# Test Project" > README.md
echo "print('Hello Mothership')" > main.py
git init
log_success "Test files created"

# Deploy project
log_step "Deploying project"
mothership deploy "$TEST_PROJECT" || log_error "Project deployment failed"
log_success "Project deployed"

# Verify project exists
log_step "Verifying project creation"
mothership gateway list | grep "$TEST_PROJECT" || log_error "Project not found in gateway"
log_success "Project verified in gateway"

# Test rift operations
log_step "Testing rift operations"

# Create new rift
mothership create-rift "$TEST_RIFT" --description "Test rift for integration testing" || log_error "Failed to create rift"
log_success "Created test rift"

# List rifts
mothership rifts --detailed || log_error "Failed to list rifts"
log_success "Listed rifts"

# Switch to new rift
mothership switch-rift "$TEST_RIFT" || log_error "Failed to switch rift"
log_success "Switched to test rift"

# Check rift status
mothership rift-status || log_error "Failed to get rift status"
log_success "Rift status checked"

# Make some changes
log_step "Making test changes"
echo "# Additional changes" >> README.md
echo "print('More changes')" >> main.py

# Create checkpoint
log_step "Creating checkpoint"
mothership checkpoint "Test checkpoint" || log_error "Failed to create checkpoint"
log_success "Checkpoint created"

# View history
log_step "Viewing history"
mothership history --limit 5 || log_error "Failed to view history"
log_success "History viewed"

# Test sync
log_step "Testing sync"
mothership sync || log_error "Sync failed"
log_success "Sync successful"

# Compare rifts
log_step "Comparing rifts"
mothership rift-diff --to "main" || log_error "Failed to compare rifts"
log_success "Rift comparison successful"

# Test daemon operations
log_step "Testing daemon operations"
mothership daemon status || log_error "Failed to get daemon status"
mothership daemon restart || log_error "Failed to restart daemon"
log_success "Daemon operations successful"

# Check for updates
log_step "Checking for updates"
mothership update --check-only || log_error "Update check failed"
mothership update --list-versions || log_error "Failed to list versions"
log_success "Update check successful"

# Clean up
log_step "Cleaning up"

# Disconnect from project
mothership project-disconnect "$TEST_PROJECT" || log_error "Failed to disconnect from project"
log_success "Disconnected from project"

# Delete test project
mothership delete "$TEST_PROJECT" --force || log_error "Failed to delete project"
log_success "Deleted test project"

# Disconnect from server
mothership server disconnect || log_error "Failed to disconnect from server"
log_success "Disconnected from server"

# Logout
mothership logout || log_error "Logout failed"
log_success "Logged out"

# Clean up test directory
cd ..
rm -rf "$TEST_DIR"
log_success "Test directory cleaned up"

# Final status
log_step "Test Summary"
echo -e "${GREEN}All tests completed successfully!${NC}"
echo "Tested features:"
echo "✓ Server connection"
echo "✓ Authentication"
echo "✓ Project creation and management"
echo "✓ Rift operations"
echo "✓ File synchronization"
echo "✓ Checkpoint creation"
echo "✓ History viewing"
echo "✓ Daemon operations"
echo "✓ Update checking"
echo "✓ Cleanup operations" 