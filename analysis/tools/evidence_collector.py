#!/usr/bin/env python3
"""
Evidence collection framework for Mothership analysis.
Provides structured evidence collection and validation.
"""

import json
import os
from datetime import datetime
from typing import Dict, Any, List, Optional
from dataclasses import dataclass, asdict
from enum import Enum

class EvidenceType(Enum):
    CODE_ANALYSIS = "code_analysis"
    FUNCTIONAL_TEST = "functional_test"
    INTEGRATION_TEST = "integration_test"
    DOCUMENTATION_REVIEW = "documentation_review"
    PERFORMANCE_TEST = "performance_test"

class ConfidenceLevel(Enum):
    HIGH = "high"      # Direct code evidence, reproducible tests
    MEDIUM = "medium"  # Indirect evidence, partial tests
    LOW = "low"        # Assumptions, incomplete evidence

class ImplementationStatus(Enum):
    FULLY_IMPLEMENTED = "fully_implemented"
    PARTIALLY_IMPLEMENTED = "partially_implemented"
    NOT_IMPLEMENTED = "not_implemented"
    PLACEHOLDER_ONLY = "placeholder_only"
    BROKEN = "broken"

@dataclass
class Evidence:
    """Structured evidence record."""
    evidence_id: str
    timestamp: str
    evidence_type: EvidenceType
    description: str
    source_file: Optional[str] = None
    line_number: Optional[int] = None
    code_snippet: Optional[str] = None
    test_result: Optional[Dict[str, Any]] = None
    confidence_level: ConfidenceLevel = ConfidenceLevel.MEDIUM
    related_claim: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None

@dataclass
class FeatureAnalysis:
    """Analysis result for a specific feature."""
    feature_name: str
    advertised_claim: str
    implementation_status: ImplementationStatus
    evidence_list: List[Evidence]
    confidence_level: ConfidenceLevel
    completion_percentage: float
    summary: str

class EvidenceCollector:
    """Centralized evidence collection and management."""
    
    def __init__(self, evidence_dir: str = "../evidence"):
        self.evidence_dir = evidence_dir
        self.evidence_file = os.path.join(evidence_dir, "collected_evidence.jsonl")
        self.analysis_file = os.path.join(evidence_dir, "feature_analysis.json")
        self.ensure_evidence_directory()
        self.evidence_counter = 0
        
    def ensure_evidence_directory(self):
        """Ensure evidence directory structure exists."""
        subdirs = ["code_analysis", "functional_tests", "integration_tests", 
                  "screenshots", "logs", "artifacts"]
        
        for subdir in subdirs:
            os.makedirs(os.path.join(self.evidence_dir, subdir), exist_ok=True)
            
    def generate_evidence_id(self) -> str:
        """Generate unique evidence ID."""
        self.evidence_counter += 1
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        return f"EVD_{timestamp}_{self.evidence_counter:04d}"
        
    def collect_evidence(self, evidence_type: EvidenceType, description: str,
                        source_file: Optional[str] = None, line_number: Optional[int] = None,
                        code_snippet: Optional[str] = None, test_result: Optional[Dict[str, Any]] = None,
                        confidence_level: ConfidenceLevel = ConfidenceLevel.MEDIUM,
                        related_claim: Optional[str] = None,
                        metadata: Optional[Dict[str, Any]] = None) -> Evidence:
        """Collect and store evidence."""
        
        evidence = Evidence(
            evidence_id=self.generate_evidence_id(),
            timestamp=datetime.now().isoformat(),
            evidence_type=evidence_type,
            description=description,
            source_file=source_file,
            line_number=line_number,
            code_snippet=code_snippet,
            test_result=test_result,
            confidence_level=confidence_level,
            related_claim=related_claim,
            metadata=metadata
        )
        
        self._save_evidence(evidence)
        return evidence
        
    def _save_evidence(self, evidence: Evidence):
        """Save evidence to persistent storage."""
        with open(self.evidence_file, "a", encoding="utf-8") as f:
            f.write(json.dumps(asdict(evidence), default=str, ensure_ascii=False) + "\n")
            
    def load_evidence(self) -> List[Evidence]:
        """Load all collected evidence."""
        evidence_list = []
        
        if os.path.exists(self.evidence_file):
            with open(self.evidence_file, "r", encoding="utf-8") as f:
                for line in f:
                    if line.strip():
                        data = json.loads(line)
                        # Convert string enums back to enum objects
                        data['evidence_type'] = EvidenceType(data['evidence_type'])
                        data['confidence_level'] = ConfidenceLevel(data['confidence_level'])
                        evidence_list.append(Evidence(**data))
                        
        return evidence_list
        
    def analyze_feature(self, feature_name: str, advertised_claim: str,
                       evidence_list: List[Evidence]) -> FeatureAnalysis:
        """Analyze a feature based on collected evidence."""
        
        # Determine implementation status based on evidence
        implementation_status = self._determine_implementation_status(evidence_list)
        
        # Calculate confidence level
        confidence_level = self._calculate_confidence_level(evidence_list)
        
        # Calculate completion percentage
        completion_percentage = self._calculate_completion_percentage(evidence_list, implementation_status)
        
        # Generate summary
        summary = self._generate_feature_summary(feature_name, implementation_status, evidence_list)
        
        analysis = FeatureAnalysis(
            feature_name=feature_name,
            advertised_claim=advertised_claim,
            implementation_status=implementation_status,
            evidence_list=evidence_list,
            confidence_level=confidence_level,
            completion_percentage=completion_percentage,
            summary=summary
        )
        
        self._save_feature_analysis(analysis)
        return analysis
        
    def _determine_implementation_status(self, evidence_list: List[Evidence]) -> ImplementationStatus:
        """Determine implementation status based on evidence."""
        if not evidence_list:
            return ImplementationStatus.NOT_IMPLEMENTED
            
        # Analyze evidence types and results
        code_evidence = [e for e in evidence_list if e.evidence_type == EvidenceType.CODE_ANALYSIS]
        test_evidence = [e for e in evidence_list if e.evidence_type in [EvidenceType.FUNCTIONAL_TEST, EvidenceType.INTEGRATION_TEST]]
        
        # Check for broken implementations
        broken_indicators = ["error", "failed", "exception", "todo", "unimplemented", "placeholder"]
        for evidence in evidence_list:
            if any(indicator in evidence.description.lower() for indicator in broken_indicators):
                return ImplementationStatus.BROKEN
                
        # Check for placeholder implementations
        placeholder_indicators = ["todo", "unimplemented", "placeholder", "stub"]
        for evidence in code_evidence:
            if any(indicator in evidence.description.lower() for indicator in placeholder_indicators):
                return ImplementationStatus.PLACEHOLDER_ONLY
                
        # Determine based on test results
        if test_evidence:
            passed_tests = sum(1 for e in test_evidence if e.test_result and e.test_result.get("passed", False))
            total_tests = len(test_evidence)
            
            if passed_tests == 0:
                return ImplementationStatus.NOT_IMPLEMENTED
            elif passed_tests == total_tests:
                return ImplementationStatus.FULLY_IMPLEMENTED
            else:
                return ImplementationStatus.PARTIALLY_IMPLEMENTED
                
        # Default based on code evidence
        if code_evidence:
            return ImplementationStatus.PARTIALLY_IMPLEMENTED
        else:
            return ImplementationStatus.NOT_IMPLEMENTED
            
    def _calculate_confidence_level(self, evidence_list: List[Evidence]) -> ConfidenceLevel:
        """Calculate overall confidence level based on evidence quality."""
        if not evidence_list:
            return ConfidenceLevel.LOW
            
        high_confidence_count = sum(1 for e in evidence_list if e.confidence_level == ConfidenceLevel.HIGH)
        total_evidence = len(evidence_list)
        
        high_confidence_ratio = high_confidence_count / total_evidence
        
        if high_confidence_ratio >= 0.7:
            return ConfidenceLevel.HIGH
        elif high_confidence_ratio >= 0.4:
            return ConfidenceLevel.MEDIUM
        else:
            return ConfidenceLevel.LOW
            
    def _calculate_completion_percentage(self, evidence_list: List[Evidence], 
                                       implementation_status: ImplementationStatus) -> float:
        """Calculate implementation completion percentage."""
        status_percentages = {
            ImplementationStatus.FULLY_IMPLEMENTED: 100.0,
            ImplementationStatus.PARTIALLY_IMPLEMENTED: 50.0,
            ImplementationStatus.PLACEHOLDER_ONLY: 10.0,
            ImplementationStatus.NOT_IMPLEMENTED: 0.0,
            ImplementationStatus.BROKEN: 0.0
        }
        
        base_percentage = status_percentages.get(implementation_status, 0.0)
        
        # Adjust based on evidence quality and quantity
        evidence_factor = min(len(evidence_list) / 5.0, 1.0)  # Max factor of 1.0 for 5+ pieces of evidence
        
        return base_percentage * evidence_factor
        
    def _generate_feature_summary(self, feature_name: str, implementation_status: ImplementationStatus,
                                evidence_list: List[Evidence]) -> str:
        """Generate a summary of the feature analysis."""
        status_descriptions = {
            ImplementationStatus.FULLY_IMPLEMENTED: "appears to be fully implemented and functional",
            ImplementationStatus.PARTIALLY_IMPLEMENTED: "has partial implementation with some functionality",
            ImplementationStatus.PLACEHOLDER_ONLY: "contains only placeholder or stub implementations",
            ImplementationStatus.NOT_IMPLEMENTED: "is not implemented",
            ImplementationStatus.BROKEN: "has broken or non-functional implementation"
        }
        
        status_desc = status_descriptions.get(implementation_status, "has unknown implementation status")
        evidence_count = len(evidence_list)
        
        return f"{feature_name} {status_desc} based on {evidence_count} pieces of evidence."
        
    def _save_feature_analysis(self, analysis: FeatureAnalysis):
        """Save feature analysis to persistent storage."""
        analyses = []
        
        # Load existing analyses
        if os.path.exists(self.analysis_file):
            with open(self.analysis_file, "r", encoding="utf-8") as f:
                data = json.load(f)
                analyses = data.get("feature_analyses", [])
                
        # Update or add analysis
        existing_index = None
        for i, existing in enumerate(analyses):
            if existing.get("feature_name") == analysis.feature_name:
                existing_index = i
                break
                
        analysis_dict = asdict(analysis)
        # Convert enums to strings for JSON serialization
        analysis_dict["implementation_status"] = analysis.implementation_status.value
        analysis_dict["confidence_level"] = analysis.confidence_level.value
        
        for evidence in analysis_dict["evidence_list"]:
            evidence["evidence_type"] = evidence["evidence_type"].value
            evidence["confidence_level"] = evidence["confidence_level"].value
            
        if existing_index is not None:
            analyses[existing_index] = analysis_dict
        else:
            analyses.append(analysis_dict)
            
        # Save updated analyses
        with open(self.analysis_file, "w", encoding="utf-8") as f:
            json.dump({"feature_analyses": analyses}, f, indent=2, ensure_ascii=False)
            
    def get_evidence_summary(self) -> Dict[str, Any]:
        """Get summary of all collected evidence."""
        evidence_list = self.load_evidence()
        
        summary = {
            "total_evidence": len(evidence_list),
            "evidence_by_type": {},
            "evidence_by_confidence": {},
            "evidence_with_code": sum(1 for e in evidence_list if e.code_snippet),
            "evidence_with_tests": sum(1 for e in evidence_list if e.test_result)
        }
        
        # Count by type
        for evidence_type in EvidenceType:
            count = sum(1 for e in evidence_list if e.evidence_type == evidence_type)
            summary["evidence_by_type"][evidence_type.value] = count
            
        # Count by confidence
        for confidence_level in ConfidenceLevel:
            count = sum(1 for e in evidence_list if e.confidence_level == confidence_level)
            summary["evidence_by_confidence"][confidence_level.value] = count
            
        return summary

# Global evidence collector instance
evidence_collector = EvidenceCollector()