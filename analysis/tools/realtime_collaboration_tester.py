#!/usr/bin/env python3
"""
Real-time collaboration functional tester for Mothership application.
Tests WebSocket functionality, latency, and conflict resolution.
"""

import asyncio
import websockets
import json
import time
import threading
import subprocess
import signal
import os
from pathlib import Path
from typing import Dict, Any, List, Optional, Tuple
from dataclasses import dataclass, asdict
import tempfile
import uuid

from logger import analysis_logger
from evidence_collector import evidence_collector, EvidenceType, ConfidenceLevel

@dataclass
class LatencyMeasurement:
    """Represents a latency measurement."""
    send_time: float
    receive_time: float
    latency_ms: float
    message_type: str
    success: bool

@dataclass
class CollaborationTestResult:
    """Represents a collaboration test result."""
    test_name: str
    passed: bool
    description: str
    latency_measurements: List[LatencyMeasurement]
    details: Dict[str, Any]
    evidence: Optional[str] = None

class RealtimeCollaborationTester:
    """Tests real-time collaboration functionality."""
    
    def __init__(self):
        self.server_process = None
        self.test_results: List[CollaborationTestResult] = []
        self.websocket_connections: List = []
        self.server_url = "ws://localhost:8080/ws"
        self.http_url = "http://localhost:8080"
        
    async def run_all_tests(self) -> Dict[str, Any]:
        """Run all real-time collaboration tests."""
        analysis_logger.log_analysis_start("realtime_collaboration", "websocket_testing")
        
        # Start server if needed
        server_started = await self._start_server_if_needed()
        
        if not server_started:
            return self._create_server_failure_result()
            
        try:
            # Test 1: Basic WebSocket connection
            await self._test_websocket_connection()
            
            # Test 2: Multiple client connections
            await self._test_multiple_connections()
            
            # Test 3: Message latency measurement
            await self._test_message_latency()
            
            # Test 4: Simultaneous editing simulation
            await self._test_simultaneous_editing()
            
            # Test 5: Conflict resolution testing
            await self._test_conflict_resolution()
            
            # Test 6: Connection stability
            await self._test_connection_stability()
            
        finally:
            await self._cleanup_connections()
            self._stop_server()
            
        return self._summarize_results()
        
    async def _start_server_if_needed(self) -> bool:
        """Start the Mothership server if it's not running."""
        try:
            # Check if server is already running
            import aiohttp
            async with aiohttp.ClientSession() as session:
                try:
                    async with session.get(f"{self.http_url}/health", timeout=2) as response:
                        if response.status == 200:
                            analysis_logger.test_logger.info("Server already running")
                            return True
                except:
                    pass
                    
            # Try to start the server
            analysis_logger.test_logger.info("Starting Mothership server...")
            
            server_binary = Path("../../mothership-server/target/release/mothership-server")
            if not server_binary.exists():
                server_binary = Path("../../mothership-server/target/debug/mothership-server")
                
            if not server_binary.exists():
                analysis_logger.test_logger.error("Server binary not found")
                return False
                
            # Start server in background
            self.server_process = subprocess.Popen(
                [str(server_binary)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                preexec_fn=os.setsid if os.name != 'nt' else None
            )
            
            # Wait for server to start
            await asyncio.sleep(3)
            
            # Check if server is responding
            async with aiohttp.ClientSession() as session:
                try:
                    async with session.get(f"{self.http_url}/health", timeout=5) as response:
                        if response.status == 200:
                            analysis_logger.test_logger.info("Server started successfully")
                            return True
                except Exception as e:
                    analysis_logger.test_logger.error(f"Server health check failed: {e}")
                    
            return False
            
        except Exception as e:
            analysis_logger.test_logger.error(f"Failed to start server: {e}")
            return False
            
    def _stop_server(self):
        """Stop the server if we started it."""
        if self.server_process:
            try:
                if os.name == 'nt':
                    self.server_process.terminate()
                else:
                    os.killpg(os.getpgid(self.server_process.pid), signal.SIGTERM)
                self.server_process.wait(timeout=5)
                analysis_logger.test_logger.info("Server stopped")
            except Exception as e:
                analysis_logger.test_logger.error(f"Failed to stop server: {e}")
                
    async def _test_websocket_connection(self):
        """Test basic WebSocket connection."""
        test_name = "websocket_connection"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        try:
            # Try to connect to WebSocket
            start_time = time.time()
            
            async with websockets.connect(self.server_url, timeout=10) as websocket:
                connection_time = (time.time() - start_time) * 1000
                
                # Send a ping message
                ping_message = {
                    "type": "ping",
                    "timestamp": time.time()
                }
                
                await websocket.send(json.dumps(ping_message))
                
                # Wait for response
                try:
                    response = await asyncio.wait_for(websocket.recv(), timeout=5)
                    response_data = json.loads(response)
                    
                    self.test_results.append(CollaborationTestResult(
                        test_name=test_name,
                        passed=True,
                        description="WebSocket connection successful",
                        latency_measurements=[],
                        details={
                            "connection_time_ms": connection_time,
                            "ping_response": response_data,
                            "server_url": self.server_url
                        }
                    ))
                    
                except asyncio.TimeoutError:
                    self.test_results.append(CollaborationTestResult(
                        test_name=test_name,
                        passed=False,
                        description="WebSocket connection established but no ping response",
                        latency_measurements=[],
                        details={
                            "connection_time_ms": connection_time,
                            "error": "No ping response within 5 seconds"
                        }
                    ))
                    
        except Exception as e:
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=False,
                description="WebSocket connection failed",
                latency_measurements=[],
                details={
                    "error": str(e),
                    "server_url": self.server_url
                }
            ))
            
    async def _test_multiple_connections(self):
        """Test multiple simultaneous WebSocket connections."""
        test_name = "multiple_connections"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        connection_count = 5
        successful_connections = 0
        connection_details = []
        
        async def connect_client(client_id: int):
            try:
                async with websockets.connect(self.server_url, timeout=10) as websocket:
                    # Send identification message
                    ident_message = {
                        "type": "identify",
                        "client_id": client_id,
                        "timestamp": time.time()
                    }
                    
                    await websocket.send(json.dumps(ident_message))
                    
                    # Wait briefly to maintain connection
                    await asyncio.sleep(2)
                    
                    return {"client_id": client_id, "success": True, "error": None}
                    
            except Exception as e:
                return {"client_id": client_id, "success": False, "error": str(e)}
                
        # Create multiple connections concurrently
        tasks = [connect_client(i) for i in range(connection_count)]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        
        for result in results:
            if isinstance(result, dict):
                connection_details.append(result)
                if result["success"]:
                    successful_connections += 1
            else:
                connection_details.append({
                    "client_id": "unknown",
                    "success": False,
                    "error": str(result)
                })
                
        self.test_results.append(CollaborationTestResult(
            test_name=test_name,
            passed=successful_connections >= connection_count * 0.8,  # 80% success rate
            description=f"Multiple connections test: {successful_connections}/{connection_count} successful",
            latency_measurements=[],
            details={
                "attempted_connections": connection_count,
                "successful_connections": successful_connections,
                "success_rate": successful_connections / connection_count,
                "connection_details": connection_details
            }
        ))
        
    async def _test_message_latency(self):
        """Test message latency to verify 'millisecond synchronization' claims."""
        test_name = "message_latency"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        latency_measurements = []
        
        try:
            async with websockets.connect(self.server_url, timeout=10) as websocket:
                # Send multiple messages and measure latency
                for i in range(10):
                    message = {
                        "type": "sync_message",
                        "content": f"Test message {i}",
                        "timestamp": time.time(),
                        "message_id": str(uuid.uuid4())
                    }
                    
                    send_time = time.time()
                    await websocket.send(json.dumps(message))
                    
                    try:
                        response = await asyncio.wait_for(websocket.recv(), timeout=5)
                        receive_time = time.time()
                        latency_ms = (receive_time - send_time) * 1000
                        
                        latency_measurements.append(LatencyMeasurement(
                            send_time=send_time,
                            receive_time=receive_time,
                            latency_ms=latency_ms,
                            message_type="sync_message",
                            success=True
                        ))
                        
                    except asyncio.TimeoutError:
                        latency_measurements.append(LatencyMeasurement(
                            send_time=send_time,
                            receive_time=0,
                            latency_ms=5000,  # Timeout = 5000ms
                            message_type="sync_message",
                            success=False
                        ))
                        
                    # Small delay between messages
                    await asyncio.sleep(0.1)
                    
        except Exception as e:
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=False,
                description="Message latency test failed",
                latency_measurements=latency_measurements,
                details={"error": str(e)}
            ))
            return
            
        # Analyze latency results
        successful_measurements = [m for m in latency_measurements if m.success]
        
        if successful_measurements:
            avg_latency = sum(m.latency_ms for m in successful_measurements) / len(successful_measurements)
            min_latency = min(m.latency_ms for m in successful_measurements)
            max_latency = max(m.latency_ms for m in successful_measurements)
            
            # Check if it meets "millisecond synchronization" claim (< 10ms would be impressive)
            millisecond_sync = avg_latency < 10
            
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=len(successful_measurements) > 0,
                description=f"Message latency test: avg {avg_latency:.2f}ms",
                latency_measurements=latency_measurements,
                details={
                    "average_latency_ms": avg_latency,
                    "min_latency_ms": min_latency,
                    "max_latency_ms": max_latency,
                    "successful_messages": len(successful_measurements),
                    "total_messages": len(latency_measurements),
                    "meets_millisecond_claim": millisecond_sync,
                    "millisecond_threshold": 10
                }
            ))
        else:
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=False,
                description="No successful latency measurements",
                latency_measurements=latency_measurements,
                details={"error": "All messages timed out"}
            ))
            
    async def _test_simultaneous_editing(self):
        """Test simultaneous editing by multiple clients."""
        test_name = "simultaneous_editing"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        client_count = 3
        edit_results = []
        
        async def simulate_client_editing(client_id: int):
            try:
                async with websockets.connect(self.server_url, timeout=10) as websocket:
                    # Simulate file editing
                    edit_message = {
                        "type": "file_edit",
                        "rift_id": "test_rift",
                        "file_path": "test_file.txt",
                        "edit": {
                            "line": client_id,
                            "content": f"Line edited by client {client_id}",
                            "timestamp": time.time()
                        },
                        "client_id": client_id
                    }
                    
                    send_time = time.time()
                    await websocket.send(json.dumps(edit_message))
                    
                    # Wait for acknowledgment or broadcast
                    try:
                        response = await asyncio.wait_for(websocket.recv(), timeout=10)
                        receive_time = time.time()
                        
                        return {
                            "client_id": client_id,
                            "success": True,
                            "latency_ms": (receive_time - send_time) * 1000,
                            "response": json.loads(response)
                        }
                        
                    except asyncio.TimeoutError:
                        return {
                            "client_id": client_id,
                            "success": False,
                            "error": "No response to edit message"
                        }
                        
            except Exception as e:
                return {
                    "client_id": client_id,
                    "success": False,
                    "error": str(e)
                }
                
        # Simulate simultaneous edits
        tasks = [simulate_client_editing(i) for i in range(client_count)]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        
        successful_edits = 0
        for result in results:
            if isinstance(result, dict):
                edit_results.append(result)
                if result.get("success", False):
                    successful_edits += 1
            else:
                edit_results.append({
                    "client_id": "unknown",
                    "success": False,
                    "error": str(result)
                })
                
        self.test_results.append(CollaborationTestResult(
            test_name=test_name,
            passed=successful_edits > 0,
            description=f"Simultaneous editing test: {successful_edits}/{client_count} successful",
            latency_measurements=[],
            details={
                "attempted_edits": client_count,
                "successful_edits": successful_edits,
                "edit_results": edit_results
            }
        ))
        
    async def _test_conflict_resolution(self):
        """Test conflict resolution when multiple clients edit the same content."""
        test_name = "conflict_resolution"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        try:
            # Create two connections that will edit the same line
            async with websockets.connect(self.server_url, timeout=10) as ws1, \
                       websockets.connect(self.server_url, timeout=10) as ws2:
                
                # Both clients edit the same line simultaneously
                conflict_edit_1 = {
                    "type": "file_edit",
                    "rift_id": "test_rift",
                    "file_path": "conflict_test.txt",
                    "edit": {
                        "line": 1,
                        "content": "Content from client 1",
                        "timestamp": time.time()
                    },
                    "client_id": 1
                }
                
                conflict_edit_2 = {
                    "type": "file_edit",
                    "rift_id": "test_rift",
                    "file_path": "conflict_test.txt",
                    "edit": {
                        "line": 1,
                        "content": "Content from client 2",
                        "timestamp": time.time() + 0.001  # Slightly later
                    },
                    "client_id": 2
                }
                
                # Send both edits nearly simultaneously
                await asyncio.gather(
                    ws1.send(json.dumps(conflict_edit_1)),
                    ws2.send(json.dumps(conflict_edit_2))
                )
                
                # Collect responses
                responses = []
                try:
                    response1 = await asyncio.wait_for(ws1.recv(), timeout=5)
                    responses.append({"client": 1, "response": json.loads(response1)})
                except asyncio.TimeoutError:
                    responses.append({"client": 1, "response": None, "error": "timeout"})
                    
                try:
                    response2 = await asyncio.wait_for(ws2.recv(), timeout=5)
                    responses.append({"client": 2, "response": json.loads(response2)})
                except asyncio.TimeoutError:
                    responses.append({"client": 2, "response": None, "error": "timeout"})
                    
                # Check if conflict was detected/resolved
                conflict_detected = any(
                    r.get("response", {}).get("type") == "conflict_detected" 
                    for r in responses if r.get("response")
                )
                
                self.test_results.append(CollaborationTestResult(
                    test_name=test_name,
                    passed=conflict_detected,
                    description=f"Conflict resolution test: {'detected' if conflict_detected else 'not detected'}",
                    latency_measurements=[],
                    details={
                        "conflict_detected": conflict_detected,
                        "responses": responses,
                        "test_scenario": "simultaneous_line_edit"
                    }
                ))
                
        except Exception as e:
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=False,
                description="Conflict resolution test failed",
                latency_measurements=[],
                details={"error": str(e)}
            ))
            
    async def _test_connection_stability(self):
        """Test WebSocket connection stability over time."""
        test_name = "connection_stability"
        analysis_logger.test_logger.info(f"Running test: {test_name}")
        
        try:
            async with websockets.connect(self.server_url, timeout=10) as websocket:
                # Send periodic messages for 30 seconds
                start_time = time.time()
                message_count = 0
                successful_messages = 0
                
                while time.time() - start_time < 30:  # 30 second test
                    message = {
                        "type": "heartbeat",
                        "timestamp": time.time(),
                        "message_id": message_count
                    }
                    
                    try:
                        await websocket.send(json.dumps(message))
                        
                        # Try to receive response
                        response = await asyncio.wait_for(websocket.recv(), timeout=2)
                        successful_messages += 1
                        
                    except asyncio.TimeoutError:
                        pass  # No response, but connection might still be alive
                    except Exception:
                        break  # Connection broken
                        
                    message_count += 1
                    await asyncio.sleep(1)  # Send message every second
                    
                stability_score = successful_messages / message_count if message_count > 0 else 0
                
                self.test_results.append(CollaborationTestResult(
                    test_name=test_name,
                    passed=stability_score > 0.8,  # 80% success rate
                    description=f"Connection stability: {stability_score:.2%} success rate",
                    latency_measurements=[],
                    details={
                        "test_duration_seconds": 30,
                        "messages_sent": message_count,
                        "successful_responses": successful_messages,
                        "stability_score": stability_score
                    }
                ))
                
        except Exception as e:
            self.test_results.append(CollaborationTestResult(
                test_name=test_name,
                passed=False,
                description="Connection stability test failed",
                latency_measurements=[],
                details={"error": str(e)}
            ))
            
    async def _cleanup_connections(self):
        """Clean up any remaining WebSocket connections."""
        for connection in self.websocket_connections:
            try:
                await connection.close()
            except:
                pass
        self.websocket_connections.clear()
        
    def _create_server_failure_result(self) -> Dict[str, Any]:
        """Create result for server startup failure."""
        self.test_results.append(CollaborationTestResult(
            test_name="server_startup",
            passed=False,
            description="Failed to start Mothership server",
            latency_measurements=[],
            details={"error": "Server binary not found or failed to start"}
        ))
        
        return self._summarize_results()
        
    def _summarize_results(self) -> Dict[str, Any]:
        """Summarize all test results."""
        total_tests = len(self.test_results)
        passed_tests = sum(1 for r in self.test_results if r.passed)
        failed_tests = total_tests - passed_tests
        
        # Calculate average latency from all measurements
        all_latencies = []
        for result in self.test_results:
            all_latencies.extend([m.latency_ms for m in result.latency_measurements if m.success])
            
        avg_latency = sum(all_latencies) / len(all_latencies) if all_latencies else 0
        
        # Collect evidence
        self._collect_evidence_for_tests()
        
        summary = {
            "total_tests": total_tests,
            "passed_tests": passed_tests,
            "failed_tests": failed_tests,
            "success_rate": passed_tests / total_tests if total_tests > 0 else 0,
            "average_latency_ms": avg_latency,
            "millisecond_sync_achieved": avg_latency < 10 if avg_latency > 0 else False,
            "test_results": [
                {
                    "test_name": r.test_name,
                    "passed": r.passed,
                    "description": r.description,
                    "details": r.details,
                    "latency_count": len(r.latency_measurements)
                }
                for r in self.test_results
            ]
        }
        
        analysis_logger.log_analysis_complete("realtime_collaboration", "websocket_testing", summary)
        return summary
        
    def _collect_evidence_for_tests(self):
        """Collect evidence for all test results."""
        for test_result in self.test_results:
            confidence = ConfidenceLevel.HIGH if test_result.passed else ConfidenceLevel.MEDIUM
            
            evidence_collector.collect_evidence(
                evidence_type=EvidenceType.FUNCTIONAL_TEST,
                description=f"Real-time collaboration test: {test_result.description}",
                test_result={
                    "test_name": test_result.test_name,
                    "passed": test_result.passed,
                    "details": test_result.details,
                    "latency_measurements": [asdict(m) for m in test_result.latency_measurements]
                },
                confidence_level=confidence,
                metadata={
                    "test_category": "realtime_collaboration",
                    "test_name": test_result.test_name
                }
            )
            
    def generate_report(self) -> str:
        """Generate a detailed report of real-time collaboration testing."""
        report = []
        report.append("# Real-Time Collaboration Functional Test Report")
        report.append("")
        
        # Summary
        total_tests = len(self.test_results)
        passed_tests = sum(1 for r in self.test_results if r.passed)
        
        report.append("## Executive Summary")
        report.append(f"- **Total Tests:** {total_tests}")
        report.append(f"- **Passed:** {passed_tests}")
        report.append(f"- **Failed:** {total_tests - passed_tests}")
        report.append(f"- **Success Rate:** {passed_tests/total_tests:.1%}" if total_tests > 0 else "- **Success Rate:** N/A")
        report.append("")
        
        # Latency analysis
        all_latencies = []
        for result in self.test_results:
            all_latencies.extend([m.latency_ms for m in result.latency_measurements if m.success])
            
        if all_latencies:
            avg_latency = sum(all_latencies) / len(all_latencies)
            min_latency = min(all_latencies)
            max_latency = max(all_latencies)
            
            report.append("## Latency Analysis")
            report.append(f"- **Average Latency:** {avg_latency:.2f}ms")
            report.append(f"- **Minimum Latency:** {min_latency:.2f}ms")
            report.append(f"- **Maximum Latency:** {max_latency:.2f}ms")
            report.append(f"- **Millisecond Sync Claim:** {'ACHIEVED' if avg_latency < 10 else 'NOT ACHIEVED'}")
            report.append("")
            
        # Individual test results
        report.append("## Test Results")
        for result in self.test_results:
            status = "PASSED" if result.passed else "FAILED"
            report.append(f"### {result.test_name} - {status}")
            report.append(f"**Description:** {result.description}")
            
            if result.latency_measurements:
                successful_measurements = [m for m in result.latency_measurements if m.success]
                if successful_measurements:
                    avg_test_latency = sum(m.latency_ms for m in successful_measurements) / len(successful_measurements)
                    report.append(f"**Average Latency:** {avg_test_latency:.2f}ms")
                    
            if result.details:
                report.append("**Details:**")
                for key, value in result.details.items():
                    if isinstance(value, (int, float, str, bool)):
                        report.append(f"- {key}: {value}")
                        
            report.append("")
            
        return "\n".join(report)

async def main():
    """Main function to run real-time collaboration tests."""
    print("Starting real-time collaboration functional testing...")
    
    tester = RealtimeCollaborationTester()
    results = await tester.run_all_tests()
    
    print(f"\nReal-time collaboration testing completed!")
    print(f"Total tests: {results['total_tests']}")
    print(f"Passed: {results['passed_tests']}")
    print(f"Failed: {results['failed_tests']}")
    print(f"Average latency: {results['average_latency_ms']:.2f}ms")
    print(f"Millisecond sync achieved: {results['millisecond_sync_achieved']}")
    
    # Generate and save report
    report = tester.generate_report()
    report_path = Path("../evidence/realtime_collaboration_test.md")
    
    with open(report_path, 'w', encoding='utf-8') as f:
        f.write(report)
        
    print(f"Report saved to: {report_path}")
    
    # Save detailed results
    results_path = Path("../evidence/realtime_collaboration_results.json")
    
    with open(results_path, 'w', encoding='utf-8') as f:
        json.dump(results, f, indent=2, default=str)
        
    print(f"Detailed results saved to: {results_path}")

if __name__ == "__main__":
    asyncio.run(main())