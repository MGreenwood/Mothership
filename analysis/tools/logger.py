#!/usr/bin/env python3
"""
Logging framework for Mothership analysis.
Provides structured logging with evidence collection capabilities.
"""

import logging
import json
import os
from datetime import datetime
from typing import Dict, Any, Optional
from enum import Enum

class LogLevel(Enum):
    DEBUG = "DEBUG"
    INFO = "INFO"
    WARNING = "WARNING"
    ERROR = "ERROR"
    CRITICAL = "CRITICAL"

class EvidenceType(Enum):
    CODE_ANALYSIS = "code_analysis"
    FUNCTIONAL_TEST = "functional_test"
    INTEGRATION_TEST = "integration_test"
    DOCUMENTATION_REVIEW = "documentation_review"

class AnalysisLogger:
    """Structured logger for analysis activities with evidence collection."""
    
    def __init__(self, log_dir: str = "../logs"):
        self.log_dir = log_dir
        self.ensure_log_directory()
        self.setup_loggers()
        
    def ensure_log_directory(self):
        """Ensure log directory exists."""
        os.makedirs(self.log_dir, exist_ok=True)
        
    def setup_loggers(self):
        """Set up structured loggers for different analysis activities."""
        # Main analysis logger
        self.main_logger = self._create_logger("analysis", "analysis.log")
        
        # Specialized loggers
        self.code_logger = self._create_logger("code_analysis", "code_analysis.log")
        self.test_logger = self._create_logger("functional_tests", "functional_tests.log")
        self.integration_logger = self._create_logger("integration_tests", "integration_tests.log")
        self.error_logger = self._create_logger("errors", "error.log")
        
    def _create_logger(self, name: str, filename: str) -> logging.Logger:
        """Create a structured logger with file and console handlers."""
        logger = logging.getLogger(name)
        logger.setLevel(logging.DEBUG)
        
        # File handler
        file_handler = logging.FileHandler(os.path.join(self.log_dir, filename))
        file_handler.setLevel(logging.DEBUG)
        
        # Console handler
        console_handler = logging.StreamHandler()
        console_handler.setLevel(logging.INFO)
        
        # Formatter
        formatter = logging.Formatter(
            '%(asctime)s - %(name)s - %(levelname)s - %(message)s'
        )
        file_handler.setFormatter(formatter)
        console_handler.setFormatter(formatter)
        
        logger.addHandler(file_handler)
        logger.addHandler(console_handler)
        
        return logger
        
    def log_analysis_start(self, analysis_type: str, target: str):
        """Log the start of an analysis activity."""
        self.main_logger.info(f"Starting {analysis_type} analysis of {target}")
        
    def log_analysis_complete(self, analysis_type: str, target: str, results: Dict[str, Any]):
        """Log the completion of an analysis activity."""
        self.main_logger.info(f"Completed {analysis_type} analysis of {target}")
        self.main_logger.debug(f"Results: {json.dumps(results, indent=2)}")
        
    def log_evidence(self, evidence_type: EvidenceType, description: str, 
                    source_file: Optional[str] = None, line_number: Optional[int] = None,
                    code_snippet: Optional[str] = None, confidence: str = "medium"):
        """Log evidence with structured metadata."""
        evidence = {
            "timestamp": datetime.now().isoformat(),
            "evidence_type": evidence_type.value,
            "description": description,
            "source_file": source_file,
            "line_number": line_number,
            "code_snippet": code_snippet,
            "confidence": confidence
        }
        
        self.main_logger.info(f"Evidence collected: {description}")
        self.main_logger.debug(f"Evidence details: {json.dumps(evidence, indent=2)}")
        
        # Save evidence to structured file
        self._save_evidence(evidence)
        
    def _save_evidence(self, evidence: Dict[str, Any]):
        """Save evidence to structured evidence file."""
        evidence_file = os.path.join(self.log_dir, "../evidence/collected_evidence.jsonl")
        os.makedirs(os.path.dirname(evidence_file), exist_ok=True)
        
        with open(evidence_file, "a") as f:
            f.write(json.dumps(evidence) + "\n")
            
    def log_test_result(self, test_name: str, passed: bool, details: Dict[str, Any]):
        """Log test execution results."""
        status = "PASSED" if passed else "FAILED"
        self.test_logger.info(f"Test {test_name}: {status}")
        self.test_logger.debug(f"Test details: {json.dumps(details, indent=2)}")
        
    def log_error(self, error_type: str, message: str, exception: Optional[Exception] = None):
        """Log errors with full context."""
        self.error_logger.error(f"{error_type}: {message}")
        if exception:
            self.error_logger.exception(f"Exception details: {str(exception)}")
            
    def log_code_finding(self, file_path: str, line_number: int, finding_type: str, 
                        description: str, code_snippet: str):
        """Log code analysis findings."""
        self.code_logger.info(f"Found {finding_type} in {file_path}:{line_number}")
        self.code_logger.debug(f"Description: {description}")
        self.code_logger.debug(f"Code snippet: {code_snippet}")
        
        # Also log as evidence
        self.log_evidence(
            EvidenceType.CODE_ANALYSIS,
            f"{finding_type}: {description}",
            file_path,
            line_number,
            code_snippet,
            "high"
        )

# Global logger instance
analysis_logger = AnalysisLogger()