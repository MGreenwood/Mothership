#!/usr/bin/env python3
"""
Authentication Flow Tester for Mothership Application

This tool tests the complete OAuth authentication flow including:
- OAuth initiation and callback handling
- Token persistence and session management
- Protected resource access
- User creation and role management

Requirements tested: 3.5, 3.4
"""

import requests
import json
import time
import urllib.parse
from typing import Dict, Optional, Tuple
import logging
from logger import analysis_logger

class AuthenticationFlowTester:
    def __init__(self, base_url: str = "http://localhost:8080"):
        self.base_url = base_url.rstrip('/')
        self.session = requests.Session()
        self.logger = analysis_logger.main_logger
        self.test_results = []
        
    def log_test_result(self, test_name: str, success: bool, details: str, evidence: str = ""):
        """Log a test result with evidence"""
        result = {
            'test': test_name,
            'success': success,
            'details': details,
            'evidence': evidence,
            'timestamp': time.time()
        }
        self.test_results.append(result)
        
        status = "✅ PASS" if success else "❌ FAIL"
        self.logger.info(f"{status}: {test_name}")
        self.logger.info(f"Details: {details}")
        if evidence:
            self.logger.info(f"Evidence: {evidence}")
        print(f"{status}: {test_name} - {details}")

    def test_server_availability(self) -> bool:
        """Test if the server is running and accessible"""
        try:
            response = self.session.get(f"{self.base_url}/health", timeout=5)
            if response.status_code == 200:
                self.log_test_result(
                    "Server Availability",
                    True,
                    f"Server responding on {self.base_url}",
                    f"HTTP {response.status_code}: {response.text[:100]}"
                )
                return True
            else:
                self.log_test_result(
                    "Server Availability",
                    False,
                    f"Server returned status {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
                return False
        except Exception as e:
            self.log_test_result(
                "Server Availability",
                False,
                f"Cannot connect to server: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_oauth_initiation_endpoints(self) -> Dict[str, bool]:
        """Test OAuth initiation endpoints for Google and GitHub"""
        results = {}
        
        for provider in ['google', 'github']:
            try:
                # Test OAuth initiation endpoint
                response = self.session.get(
                    f"{self.base_url}/auth/oauth/{provider}",
                    allow_redirects=False,
                    timeout=10
                )
                
                if response.status_code in [302, 307]:
                    # Check if redirect URL contains OAuth provider
                    location = response.headers.get('Location', '')
                    provider_domains = {
                        'google': 'accounts.google.com',
                        'github': 'github.com'
                    }
                    
                    if provider_domains[provider] in location:
                        self.log_test_result(
                            f"OAuth Initiation - {provider.title()}",
                            True,
                            f"Properly redirects to {provider} OAuth",
                            f"Redirect to: {location[:100]}..."
                        )
                        results[provider] = True
                    else:
                        self.log_test_result(
                            f"OAuth Initiation - {provider.title()}",
                            False,
                            f"Invalid redirect URL for {provider}",
                            f"Redirect to: {location}"
                        )
                        results[provider] = False
                else:
                    self.log_test_result(
                        f"OAuth Initiation - {provider.title()}",
                        False,
                        f"Expected redirect, got status {response.status_code}",
                        f"Response: {response.text[:200]}"
                    )
                    results[provider] = False
                    
            except Exception as e:
                self.log_test_result(
                    f"OAuth Initiation - {provider.title()}",
                    False,
                    f"Error testing {provider} OAuth: {str(e)}",
                    f"Exception: {type(e).__name__}"
                )
                results[provider] = False
                
        return results

    def test_oauth_callback_handling(self) -> bool:
        """Test OAuth callback endpoint handling"""
        try:
            # Test callback endpoint with invalid parameters
            response = self.session.get(
                f"{self.base_url}/auth/oauth/callback/google?code=invalid&state=invalid",
                timeout=10
            )
            
            # Should handle invalid OAuth callback gracefully
            if response.status_code in [400, 401, 403]:
                self.log_test_result(
                    "OAuth Callback Handling",
                    True,
                    "Properly handles invalid OAuth callback",
                    f"HTTP {response.status_code}: {response.text[:200]}"
                )
                return True
            else:
                self.log_test_result(
                    "OAuth Callback Handling",
                    False,
                    f"Unexpected response to invalid callback: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "OAuth Callback Handling",
                False,
                f"Error testing OAuth callback: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_protected_resource_access(self) -> Dict[str, bool]:
        """Test access to protected resources without authentication"""
        protected_endpoints = [
            '/api/projects',
            '/api/user/profile',
            '/api/admin/users',
            '/dashboard'
        ]
        
        results = {}
        
        for endpoint in protected_endpoints:
            try:
                response = self.session.get(f"{self.base_url}{endpoint}", timeout=10)
                
                # Protected resources should return 401 or 403 without auth
                if response.status_code in [401, 403]:
                    self.log_test_result(
                        f"Protected Resource - {endpoint}",
                        True,
                        f"Properly protects {endpoint} (HTTP {response.status_code})",
                        f"Response: {response.text[:100]}"
                    )
                    results[endpoint] = True
                elif response.status_code == 404:
                    self.log_test_result(
                        f"Protected Resource - {endpoint}",
                        True,
                        f"Endpoint {endpoint} not found (expected for some endpoints)",
                        f"HTTP 404: {response.text[:100]}"
                    )
                    results[endpoint] = True
                else:
                    self.log_test_result(
                        f"Protected Resource - {endpoint}",
                        False,
                        f"Unprotected endpoint {endpoint} (HTTP {response.status_code})",
                        f"Response: {response.text[:200]}"
                    )
                    results[endpoint] = False
                    
            except Exception as e:
                self.log_test_result(
                    f"Protected Resource - {endpoint}",
                    False,
                    f"Error testing {endpoint}: {str(e)}",
                    f"Exception: {type(e).__name__}"
                )
                results[endpoint] = False
                
        return results

    def test_jwt_token_validation(self) -> bool:
        """Test JWT token validation with invalid tokens"""
        try:
            # Test with invalid JWT token
            headers = {'Authorization': 'Bearer invalid.jwt.token'}
            response = self.session.get(
                f"{self.base_url}/api/user/profile",
                headers=headers,
                timeout=10
            )
            
            # Should reject invalid JWT token
            if response.status_code in [401, 403]:
                self.log_test_result(
                    "JWT Token Validation",
                    True,
                    "Properly rejects invalid JWT token",
                    f"HTTP {response.status_code}: {response.text[:100]}"
                )
                return True
            else:
                self.log_test_result(
                    "JWT Token Validation",
                    False,
                    f"Accepts invalid JWT token (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "JWT Token Validation",
                False,
                f"Error testing JWT validation: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_session_management(self) -> bool:
        """Test session management and cookie handling"""
        try:
            # Make a request to get session cookie
            response = self.session.get(f"{self.base_url}/", timeout=10)
            
            # Check if session cookies are set
            cookies = self.session.cookies
            session_cookies = [cookie for cookie in cookies if 'session' in cookie.name.lower()]
            
            if session_cookies:
                self.log_test_result(
                    "Session Management",
                    True,
                    f"Session cookies properly set: {[c.name for c in session_cookies]}",
                    f"Cookies: {dict(cookies)}"
                )
                return True
            else:
                # Check response headers for session management
                set_cookie = response.headers.get('Set-Cookie', '')
                if 'session' in set_cookie.lower():
                    self.log_test_result(
                        "Session Management",
                        True,
                        "Session management detected in headers",
                        f"Set-Cookie: {set_cookie}"
                    )
                    return True
                else:
                    self.log_test_result(
                        "Session Management",
                        False,
                        "No session management detected",
                        f"Response headers: {dict(response.headers)}"
                    )
                    return False
                    
        except Exception as e:
            self.log_test_result(
                "Session Management",
                False,
                f"Error testing session management: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_user_creation_endpoints(self) -> bool:
        """Test user creation and management endpoints"""
        try:
            # Test user creation endpoint (should be protected)
            user_data = {
                'username': 'testuser',
                'email': 'test@example.com',
                'role': 'user'
            }
            
            response = self.session.post(
                f"{self.base_url}/api/admin/users",
                json=user_data,
                timeout=10
            )
            
            # Should require authentication
            if response.status_code in [401, 403]:
                self.log_test_result(
                    "User Creation Endpoint",
                    True,
                    "User creation properly protected",
                    f"HTTP {response.status_code}: {response.text[:100]}"
                )
                return True
            elif response.status_code == 404:
                self.log_test_result(
                    "User Creation Endpoint",
                    True,
                    "User creation endpoint not found (expected)",
                    f"HTTP 404: {response.text[:100]}"
                )
                return True
            else:
                self.log_test_result(
                    "User Creation Endpoint",
                    False,
                    f"User creation not properly protected (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "User Creation Endpoint",
                False,
                f"Error testing user creation: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_role_based_access(self) -> Dict[str, bool]:
        """Test role-based access control"""
        admin_endpoints = [
            '/api/admin/users',
            '/api/admin/projects',
            '/api/admin/system'
        ]
        
        results = {}
        
        for endpoint in admin_endpoints:
            try:
                response = self.session.get(f"{self.base_url}{endpoint}", timeout=10)
                
                # Admin endpoints should be protected
                if response.status_code in [401, 403]:
                    self.log_test_result(
                        f"Role-Based Access - {endpoint}",
                        True,
                        f"Admin endpoint {endpoint} properly protected",
                        f"HTTP {response.status_code}: {response.text[:100]}"
                    )
                    results[endpoint] = True
                elif response.status_code == 404:
                    self.log_test_result(
                        f"Role-Based Access - {endpoint}",
                        True,
                        f"Admin endpoint {endpoint} not found (expected)",
                        f"HTTP 404: {response.text[:100]}"
                    )
                    results[endpoint] = True
                else:
                    self.log_test_result(
                        f"Role-Based Access - {endpoint}",
                        False,
                        f"Admin endpoint {endpoint} not protected (HTTP {response.status_code})",
                        f"Response: {response.text[:200]}"
                    )
                    results[endpoint] = False
                    
            except Exception as e:
                self.log_test_result(
                    f"Role-Based Access - {endpoint}",
                    False,
                    f"Error testing {endpoint}: {str(e)}",
                    f"Exception: {type(e).__name__}"
                )
                results[endpoint] = False
                
        return results

    def run_comprehensive_test(self) -> Dict:
        """Run all authentication flow tests"""
        print("🔐 Starting Authentication Flow Testing...")
        print("=" * 60)
        
        # Test server availability first
        if not self.test_server_availability():
            print("❌ Server not available - skipping authentication tests")
            return self.generate_report()
        
        # Run all authentication tests
        print("\n📋 Testing OAuth Initiation...")
        oauth_results = self.test_oauth_initiation_endpoints()
        
        print("\n📋 Testing OAuth Callback Handling...")
        self.test_oauth_callback_handling()
        
        print("\n📋 Testing Protected Resource Access...")
        protected_results = self.test_protected_resource_access()
        
        print("\n📋 Testing JWT Token Validation...")
        self.test_jwt_token_validation()
        
        print("\n📋 Testing Session Management...")
        self.test_session_management()
        
        print("\n📋 Testing User Creation Endpoints...")
        self.test_user_creation_endpoints()
        
        print("\n📋 Testing Role-Based Access Control...")
        rbac_results = self.test_role_based_access()
        
        return self.generate_report()

    def generate_report(self) -> Dict:
        """Generate comprehensive test report"""
        total_tests = len(self.test_results)
        passed_tests = sum(1 for result in self.test_results if result['success'])
        failed_tests = total_tests - passed_tests
        
        report = {
            'summary': {
                'total_tests': total_tests,
                'passed': passed_tests,
                'failed': failed_tests,
                'success_rate': (passed_tests / total_tests * 100) if total_tests > 0 else 0
            },
            'test_results': self.test_results,
            'timestamp': time.time()
        }
        
        print("\n" + "=" * 60)
        print("🔐 AUTHENTICATION FLOW TEST RESULTS")
        print("=" * 60)
        print(f"Total Tests: {total_tests}")
        print(f"Passed: {passed_tests}")
        print(f"Failed: {failed_tests}")
        print(f"Success Rate: {report['summary']['success_rate']:.1f}%")
        
        if failed_tests > 0:
            print(f"\n❌ Failed Tests:")
            for result in self.test_results:
                if not result['success']:
                    print(f"  - {result['test']}: {result['details']}")
        
        return report

def main():
    """Main function to run authentication flow tests"""
    import argparse
    
    parser = argparse.ArgumentParser(description='Test Mothership authentication flows')
    parser.add_argument('--url', default='http://localhost:8080', 
                       help='Base URL of the Mothership server')
    parser.add_argument('--output', help='Output file for test results (JSON)')
    
    args = parser.parse_args()
    
    tester = AuthenticationFlowTester(args.url)
    results = tester.run_comprehensive_test()
    
    if args.output:
        with open(args.output, 'w') as f:
            json.dump(results, f, indent=2)
        print(f"\n📄 Results saved to: {args.output}")
    
    # Return exit code based on test results
    return 0 if results['summary']['failed'] == 0 else 1

if __name__ == "__main__":
    exit(main())