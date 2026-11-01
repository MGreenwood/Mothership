#!/usr/bin/env python3
"""
Simple daemon test to verify basic functionality.
"""

import subprocess
import time
import requests
import os
import sys

def test_daemon():
    print("=== DAEMON FUNCTIONALITY TEST ===")
    
    # Test 1: Check if daemon binary exists
    print("\n1. Checking for daemon binary...")
    daemon_paths = [
        "../target/release/mothership-daemon.exe",
        "../target/debug/mothership-daemon.exe",
        "target/release/mothership-daemon.exe", 
        "target/debug/mothership-daemon.exe"
    ]
    
    daemon_path = None
    for path in daemon_paths:
        if os.path.exists(path):
            daemon_path = path
            print(f"   Found daemon at: {path}")
            break
    
    if not daemon_path:
        print("   Daemon binary not found")
        return
    
    # Test 2: Start daemon
    print("\n2. Starting daemon...")
    try:
        daemon_process = subprocess.Popen(
            [daemon_path],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
        print(f"   Daemon started with PID: {daemon_process.pid}")
        
        # Wait for startup
        time.sleep(3)
        
        if daemon_process.poll() is not None:
            stdout, stderr = daemon_process.communicate()
            print(f"   Daemon exited immediately")
            print(f"   STDOUT: {stdout}")
            print(f"   STDERR: {stderr}")
            return
            
    except Exception as e:
        print(f"   Failed to start daemon: {e}")
        return
    
    # Test 3: Test IPC communication
    print("\n3. Testing IPC communication...")
    daemon_url = "http://localhost:7525"
    
    try:
        # Test health endpoint
        response = requests.get(f"{daemon_url}/health", timeout=5)
        if response.status_code == 200:
            print(f"   Health check: {response.status_code}")
        else:
            print(f"   Health check failed: {response.status_code}")
            
        # Test status endpoint  
        response = requests.get(f"{daemon_url}/status", timeout=5)
        if response.status_code == 200:
            print(f"   Status check: {response.status_code}")
            print(f"   Response: {response.text[:100]}...")
        else:
            print(f"   Status check failed: {response.status_code}")
            
    except requests.exceptions.RequestException as e:
        print(f"   IPC communication failed: {e}")
    
    # Test 4: Shutdown
    print("\n4. Shutting down daemon...")
    try:
        response = requests.post(f"{daemon_url}/shutdown", timeout=5)
        time.sleep(2)
        
        if daemon_process.poll() is not None:
            print(f"   Daemon shut down gracefully")
        else:
            print(f"   Daemon did not shut down, terminating...")
            daemon_process.terminate()
            daemon_process.wait(timeout=5)
            
    except Exception as e:
        print(f"   Shutdown failed: {e}")
        try:
            daemon_process.terminate()
            daemon_process.wait(timeout=5)
        except:
            pass
    
    print("\n=== TEST COMPLETE ===")

if __name__ == "__main__":
    test_daemon()