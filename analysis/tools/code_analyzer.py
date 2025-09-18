#!/usr/bin/env python3
"""
Static code analyzer for Mothership application.
Analyzes Rust source code to identify implementation gaps and issues.
"""

import os
import re
import json
import glob
from pathlib import Path
from typing import List, Dict, Any, Optional
from dataclasses import dataclass

from logger import analysis_logger
from evidence_collector import evidence_collector, EvidenceType, ConfidenceLevel

@dataclass
class CodeIssue:
    """Represents a code issue or finding."""
    file_path: str
    line_number: int
    issue_type: str
    description: str
    code_snippet: str
    severity: str

class CodeAnalyzer:
    """Static code analyzer for Rust projects."""
    
    def __init__(self, target_dir: str = "../../"):
        self.target_dir = Path(target_dir).resolve()
        self.issues: List[CodeIssue] = []
        
        # Patterns to detect various issues
        self.todo_patterns = [
            r'//\s*TODO',
            r'//\s*FIXME',
            r'//\s*HACK',
            r'//\s*XXX'
        ]
        
        self.unimplemented_patterns = [
            r'unimplemented!\(',
            r'todo!\(',
            r'panic!\("not implemented"',
            r'panic!\("TODO"'
        ]
        
        self.placeholder_patterns = [
            r'fn\s+\w+.*\{\s*//.*placeholder',
            r'fn\s+\w+.*\{\s*$',  # Empty function bodies
            r'struct\s+\w+\s*;\s*$',  # Empty structs
        ]
        
    def analyze_project(self) -> Dict[str, Any]:
        """Analyze the entire Rust project."""
        analysis_logger.log_analysis_start("code_analysis", str(self.target_dir))
        
        results = {
            "project_path": str(self.target_dir),
            "rust_files_analyzed": 0,
            "total_issues": 0,
            "issues_by_type": {},
            "issues_by_severity": {},
            "compilation_status": "unknown"
        }
        
        # Find all Rust source files
        rust_files = self._find_rust_files()
        results["rust_files_analyzed"] = len(rust_files)
        
        analysis_logger.main_logger.info(f"Found {len(rust_files)} Rust files to analyze")
        
        # Analyze each file
        for rust_file in rust_files:
            self._analyze_file(rust_file)
            
        # Test compilation
        compilation_result = self._test_compilation()
        results["compilation_status"] = compilation_result
        
        # Summarize results
        results["total_issues"] = len(self.issues)
        results["issues_by_type"] = self._count_issues_by_type()
        results["issues_by_severity"] = self._count_issues_by_severity()
        
        # Collect evidence for each issue
        self._collect_evidence_for_issues()
        
        analysis_logger.log_analysis_complete("code_analysis", str(self.target_dir), results)
        return results
        
    def _find_rust_files(self) -> List[Path]:
        """Find all Rust source files in the project."""
        rust_files = []
        
        # Search patterns
        patterns = ["**/*.rs", "**/Cargo.toml"]
        exclude_patterns = ["**/target/**", "**/node_modules/**"]
        
        for pattern in patterns:
            for file_path in self.target_dir.glob(pattern):
                # Check if file should be excluded
                should_exclude = False
                for exclude_pattern in exclude_patterns:
                    if file_path.match(exclude_pattern):
                        should_exclude = True
                        break
                        
                if not should_exclude and file_path.is_file():
                    rust_files.append(file_path)
                    
        return rust_files
        
    def _analyze_file(self, file_path: Path):
        """Analyze a single Rust file."""
        try:
            with open(file_path, 'r', encoding='utf-8') as f:
                content = f.read()
                lines = content.split('\n')
                
            analysis_logger.code_logger.debug(f"Analyzing file: {file_path}")
            
            # Check for various issue types
            self._check_todo_comments(file_path, lines)
            self._check_unimplemented_functions(file_path, lines)
            self._check_placeholder_implementations(file_path, lines)
            self._check_error_handling(file_path, lines)
            self._check_websocket_implementation(file_path, lines)
            self._check_database_implementation(file_path, lines)
            self._check_authentication_implementation(file_path, lines)
            
        except Exception as e:
            analysis_logger.log_error("file_analysis", f"Error analyzing {file_path}: {str(e)}", e)
            
    def _check_todo_comments(self, file_path: Path, lines: List[str]):
        """Check for TODO comments and similar markers."""
        for line_num, line in enumerate(lines, 1):
            for pattern in self.todo_patterns:
                if re.search(pattern, line, re.IGNORECASE):
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=line_num,
                        issue_type="todo_comment",
                        description=f"TODO comment found: {line.strip()}",
                        code_snippet=line.strip(),
                        severity="medium"
                    )
                    self.issues.append(issue)
                    
    def _check_unimplemented_functions(self, file_path: Path, lines: List[str]):
        """Check for unimplemented functions."""
        for line_num, line in enumerate(lines, 1):
            for pattern in self.unimplemented_patterns:
                if re.search(pattern, line):
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=line_num,
                        issue_type="unimplemented",
                        description=f"Unimplemented function found: {line.strip()}",
                        code_snippet=line.strip(),
                        severity="high"
                    )
                    self.issues.append(issue)
                    
    def _check_placeholder_implementations(self, file_path: Path, lines: List[str]):
        """Check for placeholder implementations."""
        for line_num, line in enumerate(lines, 1):
            # Check for empty function bodies
            if re.search(r'fn\s+\w+.*\{\s*$', line):
                # Check if next line is just closing brace
                if line_num < len(lines) and lines[line_num].strip() == '}':
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=line_num,
                        issue_type="empty_function",
                        description=f"Empty function implementation: {line.strip()}",
                        code_snippet=f"{line.strip()}\\n{lines[line_num].strip()}",
                        severity="high"
                    )
                    self.issues.append(issue)
                    
    def _check_error_handling(self, file_path: Path, lines: List[str]):
        """Check for proper error handling."""
        unwrap_count = 0
        expect_count = 0
        
        for line_num, line in enumerate(lines, 1):
            if '.unwrap()' in line:
                unwrap_count += 1
                if unwrap_count > 5:  # Threshold for concern
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=line_num,
                        issue_type="poor_error_handling",
                        description=f"Excessive use of unwrap() - potential panic risk",
                        code_snippet=line.strip(),
                        severity="medium"
                    )
                    self.issues.append(issue)
                    
    def _check_websocket_implementation(self, file_path: Path, lines: List[str]):
        """Check WebSocket implementation for real-time collaboration."""
        if 'websocket' in str(file_path).lower() or 'sync' in str(file_path).lower():
            websocket_features = {
                'bidirectional_communication': False,
                'message_broadcasting': False,
                'conflict_resolution': False,
                'connection_management': False
            }
            
            content = '\n'.join(lines)
            
            # Check for WebSocket features
            if re.search(r'WebSocket|ws::|tokio_tungstenite', content, re.IGNORECASE):
                websocket_features['connection_management'] = True
                
            if re.search(r'broadcast|send_to_all|message.*all', content, re.IGNORECASE):
                websocket_features['message_broadcasting'] = True
                
            if re.search(r'conflict|merge|resolve', content, re.IGNORECASE):
                websocket_features['conflict_resolution'] = True
                
            # Report missing features
            for feature, implemented in websocket_features.items():
                if not implemented:
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=1,
                        issue_type="missing_websocket_feature",
                        description=f"Missing WebSocket feature: {feature}",
                        code_snippet="",
                        severity="high"
                    )
                    self.issues.append(issue)
                    
    def _check_database_implementation(self, file_path: Path, lines: List[str]):
        """Check database implementation."""
        if any(db_term in str(file_path).lower() for db_term in ['db', 'database', 'sql', 'postgres']):
            content = '\n'.join(lines)
            
            db_features = {
                'connection_pooling': bool(re.search(r'Pool|connection.*pool', content, re.IGNORECASE)),
                'transaction_handling': bool(re.search(r'transaction|begin|commit|rollback', content, re.IGNORECASE)),
                'error_handling': bool(re.search(r'Result|Error|try|catch', content, re.IGNORECASE)),
                'migration_support': bool(re.search(r'migration|migrate|schema', content, re.IGNORECASE))
            }
            
            for feature, implemented in db_features.items():
                if not implemented:
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=1,
                        issue_type="missing_database_feature",
                        description=f"Missing database feature: {feature}",
                        code_snippet="",
                        severity="medium"
                    )
                    self.issues.append(issue)
                    
    def _check_authentication_implementation(self, file_path: Path, lines: List[str]):
        """Check authentication implementation."""
        if any(auth_term in str(file_path).lower() for auth_term in ['auth', 'oauth', 'login', 'user']):
            content = '\n'.join(lines)
            
            auth_features = {
                'oauth_flow': bool(re.search(r'oauth|OAuth', content)),
                'jwt_handling': bool(re.search(r'jwt|JWT|token', content, re.IGNORECASE)),
                'session_management': bool(re.search(r'session|Session', content)),
                'user_persistence': bool(re.search(r'User|user.*save|store.*user', content))
            }
            
            for feature, implemented in auth_features.items():
                if not implemented:
                    issue = CodeIssue(
                        file_path=str(file_path.relative_to(self.target_dir)),
                        line_number=1,
                        issue_type="missing_auth_feature",
                        description=f"Missing authentication feature: {feature}",
                        code_snippet="",
                        severity="medium"
                    )
                    self.issues.append(issue)
                    
    def _test_compilation(self) -> str:
        """Test if the Rust project compiles."""
        try:
            import subprocess
            
            # Try to compile the main project
            result = subprocess.run(
                ["cargo", "check", "--all"],
                cwd=self.target_dir,
                capture_output=True,
                text=True,
                timeout=60
            )
            
            if result.returncode == 0:
                return "success"
            else:
                analysis_logger.log_error("compilation", f"Compilation failed: {result.stderr}")
                return "failed"
                
        except subprocess.TimeoutExpired:
            return "timeout"
        except FileNotFoundError:
            return "cargo_not_found"
        except Exception as e:
            analysis_logger.log_error("compilation", f"Compilation test error: {str(e)}", e)
            return "error"
            
    def _count_issues_by_type(self) -> Dict[str, int]:
        """Count issues by type."""
        counts = {}
        for issue in self.issues:
            counts[issue.issue_type] = counts.get(issue.issue_type, 0) + 1
        return counts
        
    def _count_issues_by_severity(self) -> Dict[str, int]:
        """Count issues by severity."""
        counts = {}
        for issue in self.issues:
            counts[issue.severity] = counts.get(issue.severity, 0) + 1
        return counts
        
    def _collect_evidence_for_issues(self):
        """Collect evidence for all identified issues."""
        for issue in self.issues:
            confidence = ConfidenceLevel.HIGH if issue.severity == "high" else ConfidenceLevel.MEDIUM
            
            evidence_collector.collect_evidence(
                evidence_type=EvidenceType.CODE_ANALYSIS,
                description=issue.description,
                source_file=issue.file_path,
                line_number=issue.line_number,
                code_snippet=issue.code_snippet,
                confidence_level=confidence,
                metadata={
                    "issue_type": issue.issue_type,
                    "severity": issue.severity
                }
            )
            
    def generate_report(self) -> str:
        """Generate a summary report of the analysis."""
        report = []
        report.append("# Code Analysis Report")
        report.append(f"**Project:** {self.target_dir}")
        report.append(f"**Total Issues:** {len(self.issues)}")
        report.append("")
        
        # Issues by type
        report.append("## Issues by Type")
        for issue_type, count in self._count_issues_by_type().items():
            report.append(f"- {issue_type}: {count}")
        report.append("")
        
        # Issues by severity
        report.append("## Issues by Severity")
        for severity, count in self._count_issues_by_severity().items():
            report.append(f"- {severity}: {count}")
        report.append("")
        
        # Top issues
        report.append("## Critical Issues")
        high_severity_issues = [issue for issue in self.issues if issue.severity == "high"]
        for issue in high_severity_issues[:10]:  # Top 10
            report.append(f"- **{issue.file_path}:{issue.line_number}** - {issue.description}")
            
        return "\n".join(report)

def main():
    """Main function to run code analysis."""
    print("Starting Mothership code analysis...")
    
    analyzer = CodeAnalyzer()
    results = analyzer.analyze_project()
    
    print(f"\nAnalysis completed!")
    print(f"Files analyzed: {results['rust_files_analyzed']}")
    print(f"Total issues found: {results['total_issues']}")
    print(f"Compilation status: {results['compilation_status']}")
    
    # Generate and save report
    report = analyzer.generate_report()
    report_path = Path("../reports/code_analysis_report.md")
    report_path.parent.mkdir(exist_ok=True)
    
    with open(report_path, 'w') as f:
        f.write(report)
        
    print(f"Report saved to: {report_path}")
    
    # Save detailed results
    results_path = Path("../evidence/code_analysis/analysis_results.json")
    results_path.parent.mkdir(exist_ok=True)
    
    with open(results_path, 'w') as f:
        json.dump(results, f, indent=2)
        
    print(f"Detailed results saved to: {results_path}")

if __name__ == "__main__":
    main()