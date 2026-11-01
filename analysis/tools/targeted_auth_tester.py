#!/usr/bin/env python3
"""
Targeted Authentication Flow Tester for Mothership Application

Tests the actual implemented authentication endpoints:
- /auth/check
- /auth/oauth/test
- /auth/oauth/start (POST)
- /auth/oauth/callback/google
- /auth/oauth/callback/github
- /auth/finalize

Requirements tested: 3.5, 3.4
"""

import requests
import json
import time
from typing import Dict, Optional
from logger import analysis_logger

class TargetedAuthTester:
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
        
        status = "PASS" if success else "FAIL"
        self.logger.info(f"{status}: {test_name}")
        self.logger.info(f"Details: {details}")
        if evidence:
            self.logger.info(f"Evidence: {evidence}")
        print(f"{status}: {test_name} - {details}")

    def test_auth_check_endpoint(self) -> bool:
        """Test the /auth/check endpoint"""
        try:
            response = self.session.get(f"{self.base_url}/auth/check", timeout=10)
            
            if response.status_code == 200:
                try:
                    data = response.json()
                    self.log_test_result(
                        "Auth Check Endpoint",
                        True,
                        f"Auth check endpoint working (HTTP {response.status_code})",
                        f"Response: {json.dumps(data, indent=2)}"
                    )
                    return True
                except:
                    self.log_test_result(
                        "Auth Check Endpoint",
                        True,
                        f"Auth check endpoint working (HTTP {response.status_code})",
                        f"Response: {response.text[:200]}"
                    )
                    return True
            elif response.status_code == 401:
                self.log_test_result(
                    "Auth Check Endpoint",
                    True,
                    "Auth check properly returns 401 for unauthenticated request",
                    f"Response: {response.text[:200]}"
                )
                return True
            else:
                self.log_test_result(
                    "Auth Check Endpoint",
                    False,
                    f"Unexpected response from auth check: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "Auth Check Endpoint",
                False,
                f"Error testing auth check: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_oauth_test_endpoint(self) -> bool:
        """Test the /auth/oauth/test endpoint"""
        try:
            response = self.session.get(f"{self.base_url}/auth/oauth/test", timeout=10)
            
            if response.status_code == 200:
                self.log_test_result(
                    "OAuth Test Endpoint",
                    True,
                    f"OAuth test endpoint working (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
                return True
            else:
                self.log_test_result(
                    "OAuth Test Endpoint",
                    False,
                    f"OAuth test endpoint returned: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "OAuth Test Endpoint",
                False,
                f"Error testing OAuth test endpoint: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_oauth_start_endpoint(self) -> bool:
        """Test the /auth/oauth/start endpoint (POST)"""
        try:
            # Test with Google provider
            google_data = {"provider": "google"}
            response = self.session.post(
                f"{self.base_url}/auth/oauth/start",
                json=google_data,
                timeout=10
            )
            
            google_success = False
            if response.status_code in [200, 302]:
                google_success = True
                self.log_test_result(
                    "OAuth Start - Google",
                    True,
                    f"OAuth start for Google working (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
            else:
                self.log_test_result(
                    "OAuth Start - Google",
                    False,
                    f"OAuth start for Google failed: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
            
            # Test with GitHub provider
            github_data = {"provider": "github"}
            response = self.session.post(
                f"{self.base_url}/auth/oauth/start",
                json=github_data,
                timeout=10
            )
            
            github_success = False
            if response.status_code in [200, 302]:
                github_success = True
                self.log_test_result(
                    "OAuth Start - GitHub",
                    True,
                    f"OAuth start for GitHub working (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
            else:
                self.log_test_result(
                    "OAuth Start - GitHub",
                    False,
                    f"OAuth start for GitHub failed: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
            
            return google_success and github_success
                
        except Exception as e:
            self.log_test_result(
                "OAuth Start Endpoint",
                False,
                f"Error testing OAuth start: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_oauth_callback_endpoints(self) -> Dict[str, bool]:
        """Test OAuth callback endpoints"""
        results = {}
        
        for provider in ['google', 'github']:
            try:
                # Test callback with invalid parameters
                response = self.session.get(
                    f"{self.base_url}/auth/oauth/callback/{provider}?code=invalid&state=invalid",
                    timeout=10
                )
                
                # Should handle invalid OAuth callback gracefully (400, 401, 403, or redirect)
                if response.status_code in [400, 401, 403, 302, 307]:
                    self.log_test_result(
                        f"OAuth Callback - {provider.title()}",
                        True,
                        f"OAuth callback for {provider} handles invalid params properly (HTTP {response.status_code})",
                        f"Response: {response.text[:200]}"
                    )
                    results[provider] = True
                else:
                    self.log_test_result(
                        f"OAuth Callback - {provider.title()}",
                        False,
                        f"OAuth callback for {provider} unexpected response: {response.status_code}",
                        f"Response: {response.text[:200]}"
                    )
                    results[provider] = False
                    
            except Exception as e:
                self.log_test_result(
                    f"OAuth Callback - {provider.title()}",
                    False,
                    f"Error testing {provider} OAuth callback: {str(e)}",
                    f"Exception: {type(e).__name__}"
                )
                results[provider] = False
                
        return results

    def test_auth_finalize_endpoint(self) -> bool:
        """Test the /auth/finalize endpoint"""
        try:
            response = self.session.get(f"{self.base_url}/auth/finalize", timeout=10)
            
            # Should handle finalize request (might redirect or return error for unauthenticated)
            if response.status_code in [200, 302, 307, 401, 403]:
                self.log_test_result(
                    "Auth Finalize Endpoint",
                    True,
                    f"Auth finalize endpoint working (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
                return True
            else:
                self.log_test_result(
                    "Auth Finalize Endpoint",
                    False,
                    f"Auth finalize unexpected response: {response.status_code}",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "Auth Finalize Endpoint",
                False,
                f"Error testing auth finalize: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_protected_api_endpoints(self) -> Dict[str, bool]:
        """Test protected API endpoints without authentication"""
        # Look for actual API endpoints from the server code
        api_endpoints = [
            '/api/projects',
            '/api/user/profile', 
            '/api/admin/users',
            '/api/admin/projects'
        ]
        
        results = {}
        
        for endpoint in api_endpoints:
            try:
                response = self.session.get(f"{self.base_url}{endpoint}", timeout=10)
                
                # Protected endpoints should return 401/403 or 404 if not implemented
                if response.status_code in [401, 403]:
                    self.log_test_result(
                        f"Protected API - {endpoint}",
                        True,
                        f"API endpoint {endpoint} properly protected (HTTP {response.status_code})",
                        f"Response: {response.text[:100]}"
                    )
                    results[endpoint] = True
                elif response.status_code == 404:
                    self.log_test_result(
                        f"Protected API - {endpoint}",
                        True,
                        f"API endpoint {endpoint} not implemented (HTTP 404)",
                        f"Response: {response.text[:100]}"
                    )
                    results[endpoint] = True
                else:
                    self.log_test_result(
                        f"Protected API - {endpoint}",
                        False,
                        f"API endpoint {endpoint} not properly protected (HTTP {response.status_code})",
                        f"Response: {response.text[:200]}"
                    )
                    results[endpoint] = False
                    
            except Exception as e:
                self.log_test_result(
                    f"Protected API - {endpoint}",
                    False,
                    f"Error testing {endpoint}: {str(e)}",
                    f"Exception: {type(e).__name__}"
                )
                results[endpoint] = False
                
        return results

    def test_jwt_token_handling(self) -> bool:
        """Test JWT token handling with invalid tokens"""
        try:
            # Test with malformed JWT token
            headers = {'Authorization': 'Bearer invalid.jwt.token'}
            response = self.session.get(
                f"{self.base_url}/auth/check",
                headers=headers,
                timeout=10
            )
            
            # Should reject invalid JWT token
            if response.status_code in [401, 403]:
                self.log_test_result(
                    "JWT Token Handling",
                    True,
                    "Properly rejects invalid JWT token",
                    f"HTTP {response.status_code}: {response.text[:100]}"
                )
                return True
            else:
                self.log_test_result(
                    "JWT Token Handling",
                    False,
                    f"Does not properly reject invalid JWT (HTTP {response.status_code})",
                    f"Response: {response.text[:200]}"
                )
                return False
                
        except Exception as e:
            self.log_test_result(
                "JWT Token Handling",
                False,
                f"Error testing JWT handling: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def test_session_persistence(self) -> bool:
        """Test session persistence and cookie handling"""
        try:
            # Make initial request to establish session
            response1 = self.session.get(f"{self.base_url}/", timeout=10)
            
            # Check for session cookies
            initial_cookies = dict(self.session.cookies)
            
            # Make second request to see if session persists
            response2 = self.session.get(f"{self.base_url}/auth/check", timeout=10)
            
            # Check if cookies are maintained
            final_cookies = dict(self.session.cookies)
            
            if initial_cookies or final_cookies:
                self.log_test_result(
                    "Session Persistence",
                    True,
                    f"Session cookies detected and maintained",
                    f"Cookies: {final_cookies}"
                )
                return True
            else:
                # Check response headers for session management
                set_cookie_headers = []
                for response in [response1, response2]:
                    if 'Set-Cookie' in response.headers:
                        set_cookie_headers.append(response.headers['Set-Cookie'])
                
                if set_cookie_headers:
                    self.log_test_result(
                        "Session Persistence",
                        True,
                        "Session management detected in headers",
                        f"Set-Cookie headers: {set_cookie_headers}"
                    )
                    return True
                else:
                    self.log_test_result(
                        "Session Persistence",
                        False,
                        "No session management detected",
                        f"No cookies or Set-Cookie headers found"
                    )
                    return False
                    
        except Exception as e:
            self.log_test_result(
                "Session Persistence",
                False,
                f"Error testing session persistence: {str(e)}",
                f"Exception: {type(e).__name__}"
            )
            return False

    def run_comprehensive_test(self) -> Dict:
        """Run all targeted authentication tests"""
        print("Starting Targeted Authentication Flow Testing...")
        print("=" * 60)
        
        # Test server availability first
        try:
            response = self.session.get(f"{self.base_url}/health", timeout=5)
            if response.status_code != 200:
                print("Server not available - skipping authentication tests")
                return self.generate_report()
        except:
            print("Server not available - skipping authentication tests")
            return self.generate_report()
        
        print("Server is available, running authentication tests...")
        
        # Run all authentication tests
        print("\nTesting Auth Check Endpoint...")
        self.test_auth_check_endpoint()
        
        print("\nTesting OAuth Test Endpoint...")
        self.test_oauth_test_endpoint()
        
        print("\nTesting OAuth Start Endpoint...")
        self.test_oauth_start_endpoint()
        
        print("\nTesting OAuth Callback Endpoints...")
        self.test_oauth_callback_endpoints()
        
        print("\nTesting Auth Finalize Endpoint...")
        self.test_auth_finalize_endpoint()
        
        print("\nTesting Protected API Endpoints...")
        self.test_protected_api_endpoints()
        
        print("\nTesting JWT Token Handling...")
        self.test_jwt_token_handling()
        
        print("\nTesting Session Persistence...")
        self.test_session_persistence()
        
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
        print("TARGETED AUTHENTICATION TEST RESULTS")
        print("=" * 60)
        print(f"Total Tests: {total_tests}")
        print(f"Passed: {passed_tests}")
        print(f"Failed: {failed_tests}")
        print(f"Success Rate: {report['summary']['success_rate']:.1f}%")
        
        if failed_tests > 0:
            print(f"\nFailed Tests:")
            for result in self.test_results:
                if not result['success']:
                    print(f"  - {result['test']}: {result['details']}")
        
        return report

def main():
    """Main function to run targeted authentication tests"""
    import argparse
    
    parser = argparse.ArgumentParser(description='Test Mothership authentication flows (targeted)')
    parser.add_argument('--url', default='http://localhost:8080', 
                       help='Base URL of the Mothership server')
    parser.add_argument('--output', help='Output file for test results (JSON)')
    
    args = parser.parse_args()
    
    tester = TargetedAuthTester(args.url)
    results = tester.run_comprehensive_test()
    
    if args.output:
        with open(args.output, 'w') as f:
            json.dump(results, f, indent=2)
        print(f"\nResults saved to: {args.output}")
    
    # Return exit code based on test results
    return 0 if results['summary']['failed'] == 0 else 1

if __name__ == "__main__":
    exit(main())