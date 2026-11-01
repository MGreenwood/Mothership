#!/usr/bin/env python3
"""
Simple WebSocket test for real-time collaboration functionality.
Tests basic connectivity and measures latency.
"""

import asyncio
import websockets
import json
import time
from pathlib import Path

async def test_websocket_connection():
    """Test basic WebSocket connection."""
    print("Testing WebSocket connection...")
    
    try:
        # Connect to WebSocket without timeout parameter in connect()
        uri = "ws://localhost:8080/ws"
        
        async with websockets.connect(uri) as websocket:
            print(f"Connected to {uri}")
            
            # Send a test message
            test_message = {
                "type": "ping",
                "timestamp": time.time(),
                "message": "test connection"
            }
            
            send_time = time.time()
            await websocket.send(json.dumps(test_message))
            print("Sent test message")
            
            # Try to receive a response
            try:
                response = await asyncio.wait_for(websocket.recv(), timeout=5.0)
                receive_time = time.time()
                latency_ms = (receive_time - send_time) * 1000
                
                print(f"Received response in {latency_ms:.2f}ms")
                print(f"Response: {response[:100]}...")
                
                return {
                    "success": True,
                    "latency_ms": latency_ms,
                    "response": response
                }
                
            except asyncio.TimeoutError:
                print("No response received within 5 seconds")
                return {
                    "success": True,  # Connection worked, just no response
                    "latency_ms": None,
                    "response": None,
                    "note": "Connection successful but no response"
                }
                
    except Exception as e:
        print(f"Connection failed: {e}")
        return {
            "success": False,
            "error": str(e)
        }

async def test_multiple_messages():
    """Test sending multiple messages and measure latency."""
    print("\nTesting multiple messages for latency measurement...")
    
    try:
        uri = "ws://localhost:8080/ws"
        
        async with websockets.connect(uri) as websocket:
            latencies = []
            
            for i in range(5):
                message = {
                    "type": "test_message",
                    "id": i,
                    "timestamp": time.time(),
                    "content": f"Test message {i}"
                }
                
                send_time = time.time()
                await websocket.send(json.dumps(message))
                
                try:
                    response = await asyncio.wait_for(websocket.recv(), timeout=2.0)
                    receive_time = time.time()
                    latency_ms = (receive_time - send_time) * 1000
                    latencies.append(latency_ms)
                    print(f"  Message {i}: {latency_ms:.2f}ms")
                    
                except asyncio.TimeoutError:
                    print(f"  Message {i}: No response")
                    
                await asyncio.sleep(0.1)  # Small delay between messages
                
            if latencies:
                avg_latency = sum(latencies) / len(latencies)
                min_latency = min(latencies)
                max_latency = max(latencies)
                
                print(f"\nLatency Summary:")
                print(f"  Average: {avg_latency:.2f}ms")
                print(f"  Minimum: {min_latency:.2f}ms")
                print(f"  Maximum: {max_latency:.2f}ms")
                print(f"  Millisecond sync claim: {'ACHIEVED' if avg_latency < 10 else 'NOT ACHIEVED'}")
                
                return {
                    "success": True,
                    "latencies": latencies,
                    "avg_latency": avg_latency,
                    "min_latency": min_latency,
                    "max_latency": max_latency,
                    "millisecond_sync": avg_latency < 10
                }
            else:
                return {
                    "success": False,
                    "error": "No successful message responses"
                }
                
    except Exception as e:
        print(f"Multiple message test failed: {e}")
        return {
            "success": False,
            "error": str(e)
        }

async def test_simultaneous_connections():
    """Test multiple simultaneous connections."""
    print("\nTesting multiple simultaneous connections...")
    
    async def single_connection(client_id):
        try:
            uri = "ws://localhost:8080/ws"
            async with websockets.connect(uri) as websocket:
                message = {
                    "type": "client_identify",
                    "client_id": client_id,
                    "timestamp": time.time()
                }
                
                await websocket.send(json.dumps(message))
                
                # Keep connection alive briefly
                await asyncio.sleep(1)
                
                return {"client_id": client_id, "success": True}
                
        except Exception as e:
            return {"client_id": client_id, "success": False, "error": str(e)}
    
    # Create 3 simultaneous connections
    tasks = [single_connection(i) for i in range(3)]
    results = await asyncio.gather(*tasks, return_exceptions=True)
    
    successful = sum(1 for r in results if isinstance(r, dict) and r.get("success", False))
    
    print(f"Simultaneous connections: {successful}/3 successful")
    
    for result in results:
        if isinstance(result, dict):
            status = "" if result.get("success", False) else ""
            print(f"  Client {result.get('client_id', '?')}: {status}")
        else:
            print(f"  Exception: {result}")
    
    return {
        "success": successful > 0,
        "successful_connections": successful,
        "total_attempted": 3,
        "results": results
    }

async def main():
    """Run all WebSocket tests."""
    print("=== Real-Time Collaboration WebSocket Tests ===\n")
    
    results = {}
    
    # Test 1: Basic connection
    results["connection"] = await test_websocket_connection()
    
    # Test 2: Multiple messages (latency)
    if results["connection"]["success"]:
        results["latency"] = await test_multiple_messages()
    else:
        print("Skipping latency test due to connection failure")
        results["latency"] = {"success": False, "error": "Connection failed"}
    
    # Test 3: Simultaneous connections
    results["simultaneous"] = await test_simultaneous_connections()
    
    # Summary
    print("\n=== Test Summary ===")
    print(f"Connection Test: {'PASSED' if results['connection']['success'] else 'FAILED'}")
    print(f"Latency Test: {'PASSED' if results['latency']['success'] else 'FAILED'}")
    print(f"Simultaneous Test: {'PASSED' if results['simultaneous']['success'] else 'FAILED'}")
    
    if results["latency"]["success"]:
        avg_latency = results["latency"]["avg_latency"]
        print(f"\nLatency Analysis:")
        print(f"  Average latency: {avg_latency:.2f}ms")
        print(f"  Millisecond sync claim: {'ACHIEVED' if avg_latency < 10 else 'NOT ACHIEVED'}")
        
        if avg_latency >= 100:
            print(f"  Note: Latency suggests 100ms batching (as found in code analysis)")
    
    # Save results
    results_path = Path("../evidence/simple_websocket_test_results.json")
    with open(results_path, 'w', encoding='utf-8') as f:
        json.dump(results, f, indent=2, default=str)
    
    print(f"\nResults saved to: {results_path}")
    
    return results

if __name__ == "__main__":
    asyncio.run(main())