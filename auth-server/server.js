const express = require('express');
const axios = require('axios');
const cors = require('cors');
const path = require('path');

const app = express();
const PORT = process.env.PORT || 3000;
const MOTHERSHIP_API = process.env.MOTHERSHIP_API || 'http://localhost:7523';

// Middleware
app.use(cors());
app.use(express.json());
app.use(express.urlencoded({ extended: true }));
app.use(express.static('public'));

// Serve the auth page
app.get('/auth/authorize', (req, res) => {
    const deviceCode = req.query.device_code;
    
    if (!deviceCode) {
        return res.status(400).send('Missing device code');
    }

    res.send(`
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <title>Mothership Authentication</title>
            <style>
                body {
                    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
                    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                    margin: 0;
                    padding: 0;
                    min-height: 100vh;
                    display: flex;
                    align-items: center;
                    justify-content: center;
                }
                .auth-container {
                    background: white;
                    padding: 3rem;
                    border-radius: 1rem;
                    box-shadow: 0 20px 40px rgba(0,0,0,0.1);
                    max-width: 400px;
                    width: 100%;
                    text-align: center;
                }
                .logo {
                    font-size: 3rem;
                    margin-bottom: 1rem;
                }
                h1 {
                    color: #333;
                    margin-bottom: 0.5rem;
                }
                .subtitle {
                    color: #666;
                    margin-bottom: 2rem;
                }
                .form-group {
                    margin-bottom: 1.5rem;
                    text-align: left;
                }
                label {
                    display: block;
                    margin-bottom: 0.5rem;
                    color: #333;
                    font-weight: 500;
                }
                input[type="text"], input[type="email"] {
                    width: 100%;
                    padding: 0.75rem;
                    border: 2px solid #e1e5e9;
                    border-radius: 0.5rem;
                    font-size: 1rem;
                    transition: border-color 0.2s;
                    box-sizing: border-box;
                }
                input[type="text"]:focus, input[type="email"]:focus {
                    outline: none;
                    border-color: #667eea;
                }
                button {
                    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                    color: white;
                    border: none;
                    padding: 1rem 2rem;
                    border-radius: 0.5rem;
                    font-size: 1rem;
                    font-weight: 600;
                    cursor: pointer;
                    width: 100%;
                    transition: transform 0.2s;
                }
                button:hover {
                    transform: translateY(-2px);
                }
                button:disabled {
                    opacity: 0.6;
                    cursor: not-allowed;
                    transform: none;
                }
                .device-code {
                    background: #f8f9fa;
                    padding: 1rem;
                    border-radius: 0.5rem;
                    font-family: monospace;
                    color: #495057;
                    word-break: break-all;
                    margin-bottom: 1rem;
                }
                .success {
                    color: #28a745;
                    margin-top: 1rem;
                }
                .error {
                    color: #dc3545;
                    margin-top: 1rem;
                }
            </style>
        </head>
        <body>
            <div class="auth-container">
                <div class="logo">🚀</div>
                <h1>Mothership Authentication</h1>
                <p class="subtitle">Complete your machine authorization</p>
                
                <div class="device-code">
                    Device Code: ${deviceCode}
                </div>
                
                <form id="authForm">
                    <div class="form-group">
                        <label for="username">Username:</label>
                        <input type="text" id="username" name="username" required>
                    </div>
                    
                    <div class="form-group">
                        <label for="email">Email:</label>
                        <input type="email" id="email" name="email" required>
                    </div>
                    
                    <button type="submit" id="submitBtn">
                        Authorize Machine
                    </button>
                </form>
                
                <div id="message"></div>
            </div>

            <script>
                document.getElementById('authForm').addEventListener('submit', async (e) => {
                    e.preventDefault();
                    
                    const submitBtn = document.getElementById('submitBtn');
                    const messageDiv = document.getElementById('message');
                    const username = document.getElementById('username').value;
                    const email = document.getElementById('email').value;
                    
                    submitBtn.disabled = true;
                    submitBtn.textContent = 'Authorizing...';
                    messageDiv.innerHTML = '';
                    
                    try {
                        const response = await fetch('/auth/complete', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json',
                            },
                            body: JSON.stringify({
                                device_code: '${deviceCode}',
                                username: username,
                                email: email
                            })
                        });
                        
                        const result = await response.json();
                        
                        if (result.success) {
                            messageDiv.innerHTML = '<div class="success">✅ Authorization complete! You can close this window and return to your terminal.</div>';
                            submitBtn.textContent = 'Authorized!';
                        } else {
                            messageDiv.innerHTML = '<div class="error">❌ ' + (result.error || 'Authorization failed') + '</div>';
                            submitBtn.disabled = false;
                            submitBtn.textContent = 'Authorize Machine';
                        }
                    } catch (error) {
                        messageDiv.innerHTML = '<div class="error">❌ Network error. Please try again.</div>';
                        submitBtn.disabled = false;
                        submitBtn.textContent = 'Authorize Machine';
                    }
                });
            </script>
        </body>
        </html>
    `);
});

// Handle the authorization completion
app.post('/auth/complete', async (req, res) => {
    try {
        const { device_code, username, email } = req.body;
        
        if (!device_code || !username || !email) {
            return res.json({ success: false, error: 'Missing required fields' });
        }

        // Create or get user ID (simplified - in production you'd validate credentials)
        const userId = generateUserId(username, email);
        
        // Call Mothership server to complete authorization
        const mothershipResponse = await axios.post(`${MOTHERSHIP_API}/auth/authorize-device`, {
            device_code: device_code,
            user_id: userId,
            username: username,
            email: email
        });

        if (mothershipResponse.data.success) {
            res.json({ success: true, message: 'Authorization complete' });
        } else {
            res.json({ success: false, error: mothershipResponse.data.error || 'Authorization failed' });
        }
    } catch (error) {
        console.error('Auth completion error:', error.message);
        
        if (error.response) {
            res.json({ success: false, error: error.response.data?.error || 'Server error' });
        } else {
            res.json({ success: false, error: 'Network error connecting to Mothership server' });
        }
    }
});

// Health check
app.get('/health', (req, res) => {
    res.json({ status: 'ok', service: 'mothership-auth-server' });
});

// Helper function to generate consistent user ID
function generateUserId(username, email) {
    const crypto = require('crypto');
    return crypto.createHash('sha256').update(username + email).digest('hex').substring(0, 32);
}

app.listen(PORT, '0.0.0.0', () => {
    console.log(`🔐 Mothership Auth Server running on http://0.0.0.0:${PORT}`);
    console.log(`🔗 Mothership API: ${MOTHERSHIP_API}`);
}); 