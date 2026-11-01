#!/usr/bin/env python3
"""
Main analysis orchestrator for Mothership application.
Coordinates all analysis activities and generates comprehensive reports.
"""

import json
import time
from pathlib import Path
from typing import Dict, Any

from logger import analysis_logger
from evidence_collector import evidence_collector
from code_analyzer import CodeAnalyzer
from functional_tester import FunctionalTester
from integration_test_runner import IntegrationTestRunner

class MothershipAnalyzer:
    """Main analyzer that orchestrates all analysis activities."""
    
    def __init__(self):
        self.results = {
            "analysis_timestamp": time.time(),
            "code_analysis": {},
            "functional_testing": {},
            "integration_testing": {},
            "docker_analysis": {},
            "overall_assessment": {}
        }
        
    def run_complete_analysis(self) -> Dict[str, Any]:
        """Run complete analysis of Mothership application."""
        analysis_logger.log_analysis_start("complete_analysis", "mothership_application")
        
        print("Starting comprehensive Mothership analysis...")
        print("=" * 50)
        
        # Phase 1: Code Analysis
        print("\n1. Running static code analysis...")
        self.results["code_analysis"] = self._run_code_analysis()
        
        # Phase 2: Functional Testing
        print("\n2. Running functional tests...")
        self.results["functional_testing"] = self._run_functional_testing()
        
        # Phase 3: Integration Testing
        print("\n3. Running integration tests...")
        self.results["integration_testing"] = self._run_integration_testing()
        
        # Phase 4: Docker Analysis
        print("\n4. Analyzing Docker deployment...")
        self.results["docker_analysis"] = self._run_docker_analysis()
        
        # Phase 5: Overall Assessment
        print("\n5. Generating overall assessment...")
        self.results["overall_assessment"] = self._generate_overall_assessment()
        
        # Save comprehensive results
        self._save_comprehensive_results()
        
        # Generate final report
        self._generate_final_report()
        
        analysis_logger.log_analysis_complete("complete_analysis", "mothership_application", self.results)
        
        print("\n" + "=" * 50)
        print("Analysis completed! Check the reports/ directory for results.")
        
        return self.results
        
    def _run_code_analysis(self) -> Dict[str, Any]:
        """Run static code analysis."""
        try:
            analyzer = CodeAnalyzer()
            results = analyzer.analyze_project()
            
            print(f"  Analyzed {results['rust_files_analyzed']} Rust files")
            print(f"  Found {results['total_issues']} issues")
            print(f"  Compilation status: {results['compilation_status']}")
            
            return results
            
        except Exception as e:
            analysis_logger.log_error("code_analysis_orchestration", f"Code analysis failed: {str(e)}", e)
            return {"error": str(e), "completed": False}
            
    def _run_functional_testing(self) -> Dict[str, Any]:
        """Run functional testing."""
        try:
            tester = FunctionalTester()
            results = tester.run_all_tests()
            
            print(f"  Ran {results['total_tests']} functional tests")
            print(f"  Passed: {results['passed_tests']}")
            print(f"  Failed: {results['failed_tests']}")
            
            return results
            
        except Exception as e:
            analysis_logger.log_error("functional_testing_orchestration", f"Functional testing failed: {str(e)}", e)
            return {"error": str(e), "completed": False}
            
    def _run_integration_testing(self) -> Dict[str, Any]:
        """Run integration testing."""
        try:
            runner = IntegrationTestRunner()
            results = runner.run_integration_tests()
            
            print(f"  Executed {results['executed_scripts']} integration scripts")
            print(f"  Passed: {results['passed_scripts']}")
            print(f"  Failed: {results['failed_scripts']}")
            
            return results
            
        except Exception as e:
            analysis_logger.log_error("integration_testing_orchestration", f"Integration testing failed: {str(e)}", e)
            return {"error": str(e), "completed": False}
            
    def _run_docker_analysis(self) -> Dict[str, Any]:
        """Run Docker deployment analysis."""
        try:
            runner = IntegrationTestRunner()
            results = runner.analyze_docker_deployment()
            
            print(f"  Docker compose exists: {results['docker_compose_exists']}")
            print(f"  Build successful: {results['docker_build_successful']}")
            print(f"  Services started: {results['services_started']}")
            
            return results
            
        except Exception as e:
            analysis_logger.log_error("docker_analysis_orchestration", f"Docker analysis failed: {str(e)}", e)
            return {"error": str(e), "completed": False}
            
    def _generate_overall_assessment(self) -> Dict[str, Any]:
        """Generate overall assessment of the application."""
        assessment = {
            "implementation_completeness": 0.0,
            "functionality_score": 0.0,
            "deployment_readiness": 0.0,
            "overall_score": 0.0,
            "major_issues": [],
            "missing_features": [],
            "working_features": []
        }
        
        # Calculate implementation completeness based on code analysis
        code_results = self.results.get("code_analysis", {})
        if code_results and not code_results.get("error"):
            total_files = code_results.get("rust_files_analyzed", 0)
            total_issues = code_results.get("total_issues", 0)
            
            if total_files > 0:
                # Lower score for more issues per file
                issues_per_file = total_issues / total_files
                implementation_completeness = max(0, 1.0 - (issues_per_file / 10.0))  # Normalize
                assessment["implementation_completeness"] = implementation_completeness
                
        # Calculate functionality score based on functional tests
        func_results = self.results.get("functional_testing", {})
        if func_results and not func_results.get("error"):
            total_tests = func_results.get("total_tests", 0)
            passed_tests = func_results.get("passed_tests", 0)
            
            if total_tests > 0:
                functionality_score = passed_tests / total_tests
                assessment["functionality_score"] = functionality_score
                
        # Calculate deployment readiness based on Docker analysis
        docker_results = self.results.get("docker_analysis", {})
        if docker_results and not docker_results.get("error"):
            deployment_factors = [
                docker_results.get("docker_compose_exists", False),
                docker_results.get("docker_build_successful", False),
                docker_results.get("services_started", False)
            ]
            deployment_readiness = sum(deployment_factors) / len(deployment_factors)
            assessment["deployment_readiness"] = deployment_readiness
            
        # Calculate overall score
        scores = [
            assessment["implementation_completeness"],
            assessment["functionality_score"],
            assessment["deployment_readiness"]
        ]
        assessment["overall_score"] = sum(scores) / len(scores)
        
        # Identify major issues
        if code_results:
            issues_by_severity = code_results.get("issues_by_severity", {})
            high_severity_issues = issues_by_severity.get("high", 0)
            if high_severity_issues > 0:
                assessment["major_issues"].append(f"{high_severity_issues} high-severity code issues")
                
        if func_results:
            failed_tests = func_results.get("failed_tests", 0)
            if failed_tests > 0:
                assessment["major_issues"].append(f"{failed_tests} functional tests failed")
                
        # Identify missing features based on evidence
        evidence_summary = evidence_collector.get_evidence_summary()
        missing_auth_features = 0
        missing_websocket_features = 0
        
        for evidence in evidence_collector.load_evidence():
            if "missing_auth_feature" in evidence.description:
                missing_auth_features += 1
            elif "missing_websocket_feature" in evidence.description:
                missing_websocket_features += 1
                
        if missing_auth_features > 0:
            assessment["missing_features"].append(f"Authentication features ({missing_auth_features} missing)")
        if missing_websocket_features > 0:
            assessment["missing_features"].append(f"WebSocket features ({missing_websocket_features} missing)")
            
        return assessment
        
    def _save_comprehensive_results(self):
        """Save comprehensive analysis results."""
        results_path = Path("../reports/comprehensive_analysis.json")
        results_path.parent.mkdir(exist_ok=True)
        
        with open(results_path, 'w') as f:
            json.dump(self.results, f, indent=2, default=str)
            
    def _generate_final_report(self):
        """Generate final comprehensive report."""
        report = []
        report.append("# Mothership Application Analysis Report")
        report.append(f"**Analysis Date:** {time.strftime('%Y-%m-%d %H:%M:%S')}")
        report.append("")
        
        # Executive Summary
        assessment = self.results.get("overall_assessment", {})
        report.append("## Executive Summary")
        report.append(f"**Overall Score:** {assessment.get('overall_score', 0):.1%}")
        report.append(f"**Implementation Completeness:** {assessment.get('implementation_completeness', 0):.1%}")
        report.append(f"**Functionality Score:** {assessment.get('functionality_score', 0):.1%}")
        report.append(f"**Deployment Readiness:** {assessment.get('deployment_readiness', 0):.1%}")
        report.append("")
        
        # Major Issues
        major_issues = assessment.get("major_issues", [])
        if major_issues:
            report.append("## Major Issues")
            for issue in major_issues:
                report.append(f"- {issue}")
            report.append("")
            
        # Missing Features
        missing_features = assessment.get("missing_features", [])
        if missing_features:
            report.append("## Missing Features")
            for feature in missing_features:
                report.append(f"- {feature}")
            report.append("")
            
        # Code Analysis Summary
        code_results = self.results.get("code_analysis", {})
        if code_results and not code_results.get("error"):
            report.append("## Code Analysis Summary")
            report.append(f"- **Files Analyzed:** {code_results.get('rust_files_analyzed', 0)}")
            report.append(f"- **Total Issues:** {code_results.get('total_issues', 0)}")
            report.append(f"- **Compilation Status:** {code_results.get('compilation_status', 'unknown')}")
            
            issues_by_type = code_results.get("issues_by_type", {})
            if issues_by_type:
                report.append("- **Issues by Type:**")
                for issue_type, count in issues_by_type.items():
                    report.append(f"  - {issue_type}: {count}")
            report.append("")
            
        # Functional Testing Summary
        func_results = self.results.get("functional_testing", {})
        if func_results and not func_results.get("error"):
            report.append("## Functional Testing Summary")
            report.append(f"- **Total Tests:** {func_results.get('total_tests', 0)}")
            report.append(f"- **Passed Tests:** {func_results.get('passed_tests', 0)}")
            report.append(f"- **Failed Tests:** {func_results.get('failed_tests', 0)}")
            report.append("")
            
        # Integration Testing Summary
        int_results = self.results.get("integration_testing", {})
        if int_results and not int_results.get("error"):
            report.append("## Integration Testing Summary")
            report.append(f"- **Total Scripts:** {int_results.get('total_scripts', 0)}")
            report.append(f"- **Executed Scripts:** {int_results.get('executed_scripts', 0)}")
            report.append(f"- **Passed Scripts:** {int_results.get('passed_scripts', 0)}")
            report.append(f"- **Failed Scripts:** {int_results.get('failed_scripts', 0)}")
            report.append("")
            
        # Docker Analysis Summary
        docker_results = self.results.get("docker_analysis", {})
        if docker_results and not docker_results.get("error"):
            report.append("## Docker Deployment Summary")
            report.append(f"- **Docker Compose Exists:** {docker_results.get('docker_compose_exists', False)}")
            report.append(f"- **Build Successful:** {docker_results.get('docker_build_successful', False)}")
            report.append(f"- **Services Started:** {docker_results.get('services_started', False)}")
            report.append("")
            
        # Evidence Summary
        evidence_summary = evidence_collector.get_evidence_summary()
        report.append("## Evidence Summary")
        report.append(f"- **Total Evidence Collected:** {evidence_summary.get('total_evidence', 0)}")
        report.append(f"- **Evidence with Code Snippets:** {evidence_summary.get('evidence_with_code', 0)}")
        report.append(f"- **Evidence with Test Results:** {evidence_summary.get('evidence_with_tests', 0)}")
        
        evidence_by_confidence = evidence_summary.get("evidence_by_confidence", {})
        if evidence_by_confidence:
            report.append("- **Evidence by Confidence:**")
            for confidence, count in evidence_by_confidence.items():
                report.append(f"  - {confidence}: {count}")
                
        # Conclusions
        report.append("")
        report.append("## Conclusions")
        
        overall_score = assessment.get('overall_score', 0)
        if overall_score < 0.3:
            report.append("The Mothership application appears to have significant gaps between advertised functionality and actual implementation.")
        elif overall_score < 0.6:
            report.append("The Mothership application has partial implementation of advertised features with notable gaps.")
        else:
            report.append("The Mothership application appears to have reasonable implementation of advertised features.")
            
        report.append("")
        report.append("For detailed analysis results, see the individual report files:")
        report.append("- `code_analysis_report.md` - Detailed code analysis")
        report.append("- `functional_testing_report.md` - Functional test results")
        report.append("- `integration_testing_report.md` - Integration test results")
        report.append("- `comprehensive_analysis.json` - Raw analysis data")
        
        # Save report
        report_path = Path("../reports/final_analysis_report.md")
        with open(report_path, 'w') as f:
            f.write("\n".join(report))
            
        print(f"Final report saved to: {report_path}")

def main():
    """Main function to run complete analysis."""
    analyzer = MothershipAnalyzer()
    results = analyzer.run_complete_analysis()
    
    # Print summary
    assessment = results.get("overall_assessment", {})
    print(f"\nFinal Assessment:")
    print(f"Overall Score: {assessment.get('overall_score', 0):.1%}")
    print(f"Implementation Completeness: {assessment.get('implementation_completeness', 0):.1%}")
    print(f"Functionality Score: {assessment.get('functionality_score', 0):.1%}")
    print(f"Deployment Readiness: {assessment.get('deployment_readiness', 0):.1%}")

if __name__ == "__main__":
    main()