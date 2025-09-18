#!/usr/bin/env python3
"""
Integration Test Reliability Tester for Mothership Analysis
Runs integration tests multiple times to check for consistency and reliability
"""

import subprocess
import sys
import os
import json
import time
from datetime import datetime
from pathlib import Path
import statistics
from collections import defaultdict

class IntegrationTestReliabilityTester:
    def __init__(self, workspace_root, num_runs=3):
        self.workspace_root = Path(workspace_root)
        self.script_path = self.workspace_root / "scripts" / "integration-test.ps1"
        self.num_runs = num_runs
        self.results = {
            "test_runs": [],
            "reliability_analysis": {},
            "consistency_analysis": {},
            "summary": {}
        }
        
    def run_multiple_tests(self):
        """Run integration tests multiple times"""
        print(f"🔄 Running integration tests {self.num_runs} times for reliability analysis...")
        
        for run_num in range(1, self.num_runs + 1):
            print(f"\n📋 Starting test run {run_num}/{self.num_runs}")
            
            run_result = self.execute_single_test(run_num)
            self.results["test_runs"].append(run_result)
            
            # Wait between runs to avoid interference
            if run_num < self.num_runs:
                print("⏳ Waiting 2 seconds before next run...")
                time.sleep(2)
        
        print(f"\n✅ Completed {self.num_runs} test runs")
        return self.results
    
    def execute_single_test(self, run_number):
        """Execute a single integration test run"""
        start_time = time.time()
        
        try:
            # Execute PowerShell script
            process = subprocess.Popen(
                ["powershell", "-ExecutionPolicy", "Bypass", "-File", str(self.script_path)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                cwd=str(self.workspace_root)
            )
            
            stdout, stderr = process.communicate()
            end_time = time.time()
            
            # Parse output for this run
            run_result = {
                "run_number": run_number,
                "start_time": datetime.now().isoformat(),
                "execution_time": end_time - start_time,
                "exit_code": process.returncode,
                "stdout": stdout,
                "stderr": stderr,
                "test_steps": self.extract_test_steps(stdout),
                "passed_tests": self.extract_passed_tests(stdout),
                "failed_tests": self.extract_failed_tests(stdout),
                "features_tested": self.extract_features_tested(stdout)
            }
            
            print(f"  ⏱️  Duration: {run_result['execution_time']:.2f}s")
            print(f"  📊 Exit code: {run_result['exit_code']}")
            print(f"  ✅ Passed: {len(run_result['passed_tests'])}")
            print(f"  ❌ Failed: {len(run_result['failed_tests'])}")
            
            return run_result
            
        except Exception as e:
            print(f"  💥 Error in run {run_number}: {e}")
            return {
                "run_number": run_number,
                "error": str(e),
                "execution_time": 0,
                "exit_code": -1,
                "stdout": "",
                "stderr": "",
                "test_steps": [],
                "passed_tests": [],
                "failed_tests": [],
                "features_tested": []
            }
    
    def extract_test_steps(self, stdout):
        """Extract test steps from stdout"""
        import re
        step_pattern = r"==== (.+?) ===="
        return re.findall(step_pattern, stdout)
    
    def extract_passed_tests(self, stdout):
        """Extract passed tests from stdout"""
        import re
        success_pattern = r"\[\+\] (.+)"
        return re.findall(success_pattern, stdout)
    
    def extract_failed_tests(self, stdout):
        """Extract failed tests from stdout"""
        import re
        error_pattern = r"\[-\] (.+)"
        return re.findall(error_pattern, stdout)
    
    def extract_features_tested(self, stdout):
        """Extract features tested from stdout"""
        import re
        if "Tested features:" in stdout:
            features_section = stdout.split("Tested features:")[1]
            feature_pattern = r"\[\+\] (.+)"
            return re.findall(feature_pattern, features_section)
        return []
    
    def analyze_reliability(self):
        """Analyze test reliability across multiple runs"""
        print("🔍 Analyzing test reliability...")
        
        if not self.results["test_runs"]:
            print("❌ No test runs to analyze")
            return
        
        # Collect metrics
        exit_codes = [run.get("exit_code", -1) for run in self.results["test_runs"]]
        execution_times = [run.get("execution_time", 0) for run in self.results["test_runs"]]
        passed_counts = [len(run.get("passed_tests", [])) for run in self.results["test_runs"]]
        failed_counts = [len(run.get("failed_tests", [])) for run in self.results["test_runs"]]
        
        # Calculate statistics
        self.results["reliability_analysis"] = {
            "total_runs": len(self.results["test_runs"]),
            "successful_runs": sum(1 for code in exit_codes if code == 0),
            "failed_runs": sum(1 for code in exit_codes if code != 0),
            "success_rate": (sum(1 for code in exit_codes if code == 0) / len(exit_codes)) * 100,
            "execution_time_stats": {
                "min": min(execution_times) if execution_times else 0,
                "max": max(execution_times) if execution_times else 0,
                "mean": statistics.mean(execution_times) if execution_times else 0,
                "stdev": statistics.stdev(execution_times) if len(execution_times) > 1 else 0
            },
            "passed_tests_stats": {
                "min": min(passed_counts) if passed_counts else 0,
                "max": max(passed_counts) if passed_counts else 0,
                "mean": statistics.mean(passed_counts) if passed_counts else 0,
                "consistent": len(set(passed_counts)) == 1
            },
            "failed_tests_stats": {
                "min": min(failed_counts) if failed_counts else 0,
                "max": max(failed_counts) if failed_counts else 0,
                "mean": statistics.mean(failed_counts) if failed_counts else 0,
                "consistent": len(set(failed_counts)) == 1
            }
        }
        
        print(f"📊 Success rate: {self.results['reliability_analysis']['success_rate']:.1f}%")
        print(f"⏱️  Avg execution time: {self.results['reliability_analysis']['execution_time_stats']['mean']:.2f}s")
        print(f"✅ Passed tests consistent: {self.results['reliability_analysis']['passed_tests_stats']['consistent']}")
        print(f"❌ Failed tests consistent: {self.results['reliability_analysis']['failed_tests_stats']['consistent']}")
    
    def analyze_consistency(self):
        """Analyze consistency of test results across runs"""
        print("🔍 Analyzing test consistency...")
        
        # Track which tests passed/failed in each run
        all_passed_tests = set()
        all_failed_tests = set()
        
        for run in self.results["test_runs"]:
            all_passed_tests.update(run.get("passed_tests", []))
            all_failed_tests.update(run.get("failed_tests", []))
        
        # Analyze consistency for each test
        test_consistency = {}
        
        for test in all_passed_tests | all_failed_tests:
            pass_count = 0
            fail_count = 0
            
            for run in self.results["test_runs"]:
                if test in run.get("passed_tests", []):
                    pass_count += 1
                elif test in run.get("failed_tests", []):
                    fail_count += 1
            
            total_appearances = pass_count + fail_count
            if total_appearances > 0:
                test_consistency[test] = {
                    "pass_count": pass_count,
                    "fail_count": fail_count,
                    "total_appearances": total_appearances,
                    "consistency_rate": max(pass_count, fail_count) / total_appearances * 100,
                    "is_flaky": pass_count > 0 and fail_count > 0
                }
        
        # Identify flaky tests
        flaky_tests = [test for test, stats in test_consistency.items() if stats["is_flaky"]]
        
        self.results["consistency_analysis"] = {
            "test_consistency": test_consistency,
            "flaky_tests": flaky_tests,
            "total_unique_tests": len(test_consistency),
            "flaky_test_count": len(flaky_tests),
            "flaky_test_percentage": (len(flaky_tests) / len(test_consistency) * 100) if test_consistency else 0
        }
        
        print(f"🧪 Total unique tests: {len(test_consistency)}")
        print(f"🔄 Flaky tests: {len(flaky_tests)}")
        if flaky_tests:
            print("⚠️  Flaky tests found:")
            for test in flaky_tests:
                stats = test_consistency[test]
                print(f"   - {test} (passed {stats['pass_count']}/{stats['total_appearances']} times)")
    
    def generate_summary(self):
        """Generate overall summary of reliability testing"""
        print("📋 Generating reliability summary...")
        
        reliability = self.results.get("reliability_analysis", {})
        consistency = self.results.get("consistency_analysis", {})
        
        self.results["summary"] = {
            "overall_reliability": "HIGH" if reliability.get("success_rate", 0) >= 90 else 
                                 "MEDIUM" if reliability.get("success_rate", 0) >= 70 else "LOW",
            "test_consistency": "HIGH" if consistency.get("flaky_test_percentage", 100) <= 10 else
                              "MEDIUM" if consistency.get("flaky_test_percentage", 100) <= 30 else "LOW",
            "execution_stability": "STABLE" if reliability.get("execution_time_stats", {}).get("stdev", 999) < 1.0 else "UNSTABLE",
            "key_findings": [],
            "recommendations": []
        }
        
        # Add key findings
        if reliability.get("success_rate", 0) < 100:
            self.results["summary"]["key_findings"].append(
                f"Tests failed in {100 - reliability.get('success_rate', 0):.1f}% of runs"
            )
        
        if consistency.get("flaky_tests"):
            self.results["summary"]["key_findings"].append(
                f"Found {len(consistency['flaky_tests'])} flaky tests with inconsistent results"
            )
        
        if reliability.get("execution_time_stats", {}).get("stdev", 0) > 1.0:
            self.results["summary"]["key_findings"].append(
                f"Execution time varies significantly (stdev: {reliability['execution_time_stats']['stdev']:.2f}s)"
            )
        
        # Add recommendations
        if consistency.get("flaky_tests"):
            self.results["summary"]["recommendations"].append(
                "Investigate and fix flaky tests for reliable CI/CD integration"
            )
        
        if reliability.get("success_rate", 0) < 90:
            self.results["summary"]["recommendations"].append(
                "Improve test reliability before using for automated testing"
            )
        
        print(f"🎯 Overall reliability: {self.results['summary']['overall_reliability']}")
        print(f"🔄 Test consistency: {self.results['summary']['test_consistency']}")
        print(f"⚡ Execution stability: {self.results['summary']['execution_stability']}")
    
    def save_results(self, output_file):
        """Save reliability test results"""
        print(f"💾 Saving reliability results to {output_file}")
        
        with open(output_file, 'w', encoding='utf-8') as f:
            json.dump(self.results, f, indent=2)
        
        # Generate human-readable report
        report_file = output_file.replace('.json', '_report.md')
        self.generate_report(report_file)
        
        print(f"📄 Report saved to {report_file}")
    
    def generate_report(self, report_file):
        """Generate human-readable reliability report"""
        with open(report_file, 'w', encoding='utf-8') as f:
            f.write("# Integration Test Reliability Report\n\n")
            f.write(f"**Test Date:** {datetime.now().isoformat()}\n")
            f.write(f"**Number of Runs:** {self.results['reliability_analysis']['total_runs']}\n\n")
            
            # Summary
            summary = self.results.get("summary", {})
            f.write("## Summary\n\n")
            f.write(f"- **Overall Reliability:** {summary.get('overall_reliability', 'UNKNOWN')}\n")
            f.write(f"- **Test Consistency:** {summary.get('test_consistency', 'UNKNOWN')}\n")
            f.write(f"- **Execution Stability:** {summary.get('execution_stability', 'UNKNOWN')}\n\n")
            
            # Reliability Analysis
            reliability = self.results.get("reliability_analysis", {})
            f.write("## Reliability Analysis\n\n")
            f.write(f"- **Success Rate:** {reliability.get('success_rate', 0):.1f}%\n")
            f.write(f"- **Successful Runs:** {reliability.get('successful_runs', 0)}/{reliability.get('total_runs', 0)}\n")
            f.write(f"- **Failed Runs:** {reliability.get('failed_runs', 0)}/{reliability.get('total_runs', 0)}\n\n")
            
            # Execution Time Statistics
            time_stats = reliability.get("execution_time_stats", {})
            f.write("### Execution Time Statistics\n\n")
            f.write(f"- **Min:** {time_stats.get('min', 0):.2f}s\n")
            f.write(f"- **Max:** {time_stats.get('max', 0):.2f}s\n")
            f.write(f"- **Mean:** {time_stats.get('mean', 0):.2f}s\n")
            f.write(f"- **Standard Deviation:** {time_stats.get('stdev', 0):.2f}s\n\n")
            
            # Consistency Analysis
            consistency = self.results.get("consistency_analysis", {})
            f.write("## Consistency Analysis\n\n")
            f.write(f"- **Total Unique Tests:** {consistency.get('total_unique_tests', 0)}\n")
            f.write(f"- **Flaky Tests:** {consistency.get('flaky_test_count', 0)}\n")
            f.write(f"- **Flaky Test Percentage:** {consistency.get('flaky_test_percentage', 0):.1f}%\n\n")
            
            # Flaky Tests Details
            if consistency.get("flaky_tests"):
                f.write("### Flaky Tests\n\n")
                for test in consistency["flaky_tests"]:
                    stats = consistency["test_consistency"][test]
                    f.write(f"- **{test}**\n")
                    f.write(f"  - Passed: {stats['pass_count']}/{stats['total_appearances']} times\n")
                    f.write(f"  - Failed: {stats['fail_count']}/{stats['total_appearances']} times\n")
                    f.write(f"  - Consistency Rate: {stats['consistency_rate']:.1f}%\n\n")
            
            # Key Findings
            if summary.get("key_findings"):
                f.write("## Key Findings\n\n")
                for finding in summary["key_findings"]:
                    f.write(f"- {finding}\n")
                f.write("\n")
            
            # Recommendations
            if summary.get("recommendations"):
                f.write("## Recommendations\n\n")
                for rec in summary["recommendations"]:
                    f.write(f"- {rec}\n")
                f.write("\n")
            
            # Individual Run Results
            f.write("## Individual Run Results\n\n")
            for run in self.results["test_runs"]:
                f.write(f"### Run {run['run_number']}\n\n")
                f.write(f"- **Exit Code:** {run.get('exit_code', 'N/A')}\n")
                f.write(f"- **Duration:** {run.get('execution_time', 0):.2f}s\n")
                f.write(f"- **Passed Tests:** {len(run.get('passed_tests', []))}\n")
                f.write(f"- **Failed Tests:** {len(run.get('failed_tests', []))}\n\n")

def main():
    if len(sys.argv) < 2:
        print("Usage: python integration_test_reliability.py <workspace_root> [num_runs]")
        sys.exit(1)
    
    workspace_root = sys.argv[1]
    num_runs = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    
    tester = IntegrationTestReliabilityTester(workspace_root, num_runs)
    
    try:
        # Run multiple tests
        tester.run_multiple_tests()
        
        # Analyze reliability
        tester.analyze_reliability()
        
        # Analyze consistency
        tester.analyze_consistency()
        
        # Generate summary
        tester.generate_summary()
        
        # Save results
        output_file = Path(workspace_root) / "analysis" / "evidence" / "integration_test_reliability.json"
        output_file.parent.mkdir(parents=True, exist_ok=True)
        tester.save_results(str(output_file))
        
        print("\n🎉 Reliability analysis complete!")
        print(f"📊 Results saved to: {output_file}")
        
    except Exception as e:
        print(f"💥 Fatal error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()