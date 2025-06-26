import * as monaco from 'monaco-editor'
import { initVimMode } from 'monaco-vim'

// Tauri detection - simplified for Tauri 2.x
let isTauri = false;

function detectTauri() {
    // Tauri 2.x detection
    const hasInvoke = typeof window.__TAURI_INTERNALS__ !== 'undefined' && 
                     typeof window.__TAURI_INTERNALS__.invoke === 'function';
    const isTauriUrl = window.location.hostname === 'localhost' && 
                      window.location.port === '1420';
    
    isTauri = hasInvoke && isTauriUrl;
    
    console.log('Tauri Detection:', {
        hasInvoke,
        isTauriUrl,
        isTauri,
        userAgent: navigator.userAgent,
        location: window.location.href,
        tauriInternals: typeof window.__TAURI_INTERNALS__,
        availableGlobals: Object.keys(window).filter(key => key.includes('TAURI'))
    });
    
    // Also check for dialog API availability
    if (isTauri) {
        console.log('Tauri Dialog API Check:', {
            hasTauriGlobal: typeof window.__TAURI__,
            hasDialog: typeof window.__TAURI__ !== 'undefined' && typeof window.__TAURI__.dialog !== 'undefined',
            hasInternals: typeof window.__TAURI_INTERNALS__,
            hasInternalsInvoke: typeof window.__TAURI_INTERNALS__ !== 'undefined' && typeof window.__TAURI_INTERNALS__.invoke === 'function'
        });
    }
    
    return isTauri;
}

// Safe invoke wrapper
async function safeInvoke(command, args = {}) {
    if (!isTauri) {
        console.warn('Not running in Tauri context, mocking:', command, args)
        // Mock responses for development
        switch (command) {
            case 'get_editor_state':
                return { current_file: null, vim_mode: true, projects: [] }
            case 'load_projects':
                return []
            case 'list_directory':
                // Mock file listing
                return [
                    { name: 'src', path: args.path + '/src', is_directory: true, size: null, modified: null },
                    { name: 'main.js', path: args.path + '/main.js', is_directory: false, size: 1024, modified: '2024-01-01 12:00:00' },
                    { name: 'package.json', path: args.path + '/package.json', is_directory: false, size: 512, modified: '2024-01-01 11:00:00' }
                ]
            case 'read_file_content':
                return '// This is a mock file content\n// The actual file content would be loaded in Tauri context\nconsole.log("Hello from mock file!");'
            case 'write_file_content':
                console.log('Mock: Would save to', args.path)
                return null
            case 'set_current_file':
                console.log('Mock: Current file set to', args.path)
                return null
            case 'toggle_vim_mode':
                return !isVimEnabled
            case 'create_checkpoint':
                console.log('Mock: Checkpoint created with message:', args.message)
                return null
            case 'authenticate_with_mothership':
                throw new Error('Authentication requires Tauri desktop app')
            case 'start_google_oauth':
                throw new Error('OAuth authentication requires Tauri desktop app')
            case 'create_gateway':
                console.log('Mock: Would create gateway:', args)
                // Simulate gateway creation
                return {
                    id: 'mock-gateway-' + Date.now(),
                    name: args.name,
                    description: args.description,
                    path: args.project_path
                }
            case 'open_directory_dialog':
                console.log('Mock: Directory picker not available in browser mode')
                return null
            case 'debug_credentials_file':
                console.log('Mock: Debug credentials not available in browser mode')
                return 'Debug not available in browser mode'
            default:
                console.warn('Unhandled mock command:', command)
                return null
        }
    }
    
         try {
         console.log(`🔧 Invoking Tauri command: ${command}`, args)
         const result = await window.__TAURI_INTERNALS__.invoke(command, args)
         console.log(`✅ Command ${command} successful:`, result)
         return result
     } catch (error) {
         console.error(`❌ Command ${command} failed:`, error)
         
         // If invoke fails with IPC errors, it means Tauri isn't ready yet or we're in browser mode
         if (error.message && (
             error.message.includes('not a function') ||
             error.message.includes('IPC')
         )) {
             console.warn('⚠️ Tauri IPC error detected, switching to browser mode')
             isTauri = false
             
             // Re-run this function in browser mode
             return await safeInvoke(command, args)
         }
         
         throw error
     }
}

// Application state
let editor = null
let vimMode = null
let currentFile = null
let isVimEnabled = true
let projects = []
let currentProject = null

// DOM elements
const authOverlay = document.getElementById('auth-overlay')
const authMessage = document.getElementById('auth-message')
const authBtn = document.getElementById('auth-btn')
const refreshBtn = document.getElementById('refresh-btn')
const checkpointBtn = document.getElementById('checkpoint-btn')
const vimToggle = document.getElementById('vim-toggle')
const projectList = document.getElementById('project-list')
const fileExplorer = document.getElementById('file-explorer')
const editorTabs = document.getElementById('editor-tabs')
const currentFileSpan = document.getElementById('current-file')
const vimModeSpan = document.getElementById('vim-mode')
const cursorPositionSpan = document.getElementById('cursor-position')

// Authentication modal elements
const googleLoginBtn = document.getElementById('google-login-btn')
const githubLoginBtn = document.getElementById('github-login-btn')
const emailLoginForm = document.getElementById('email-login-form')
const emailLoginBtn = document.getElementById('email-login-btn')
const forgotPasswordLink = document.getElementById('forgot-password')
const createAccountLink = document.getElementById('create-account')

// Gateway creation modal elements
const gatewayOverlay = document.getElementById('gateway-overlay')
const gatewayMessage = document.getElementById('gateway-message')
const newGatewayBtn = document.getElementById('new-gateway-btn')
const gatewayForm = document.getElementById('gateway-form')
const gatewayNameInput = document.getElementById('gateway-name')
const gatewayDescriptionInput = document.getElementById('gateway-description')
const gatewayPathInput = document.getElementById('gateway-path')
const browsePathBtn = document.getElementById('browse-path-btn')
const cancelGatewayBtn = document.getElementById('cancel-gateway-btn')
const createGatewayBtn = document.getElementById('create-gateway-btn')

// Authentication state
let isAuthenticated = false

// Initialize Monaco Editor
function initializeEditor() {
    // Configure Monaco Worker (fixes web worker errors in Tauri)
    self.MonacoEnvironment = {
        getWorker: function (moduleId, label) {
            // In Tauri, we need to disable workers or use a different approach
            // This prevents the worker loading errors
            return {
                postMessage: function() {},
                terminate: function() {},
                addEventListener: function() {},
                removeEventListener: function() {}
            };
        }
    };
    
    // Configure Monaco for dark theme
    monaco.editor.defineTheme('mothership-dark', {
        base: 'vs-dark',
        inherit: true,
        rules: [
            { token: '', foreground: 'ffffff' },
            { token: 'comment', foreground: '6a9955', fontStyle: 'italic' },
            { token: 'keyword', foreground: '569cd6' },
            { token: 'string', foreground: 'ce9178' },
            { token: 'number', foreground: 'b5cea8' },
        ],
        colors: {
            'editor.background': '#1e1e1e',
            'editor.foreground': '#d4d4d4',
            'editor.lineHighlightBackground': '#2d2d30',
            'editor.selectionBackground': '#264f78',
            'editorCursor.foreground': '#ffffff',
            'editorWhitespace.foreground': '#404040'
        }
    })

    monaco.editor.setTheme('mothership-dark')

    // Create editor instance
    editor = monaco.editor.create(document.getElementById('monaco-editor'), {
        value: '// Welcome to Mothership\n// Revolutionary version control with vim integration\n\n// Open a file from the sidebar to start editing',
        language: 'javascript',
        theme: 'mothership-dark',
        fontSize: 14,
        fontFamily: 'JetBrains Mono, Fira Code, Monaco, Consolas, monospace',
        ligatures: true,
        minimap: { enabled: true },
        automaticLayout: true,
        wordWrap: 'on',
        lineNumbers: 'on',
        folding: true,
        renderWhitespace: 'selection',
        scrollBeyondLastLine: false,
        smoothScrolling: true,
        cursorBlinking: 'smooth'
    })

    // Initialize vim mode
    if (isVimEnabled) {
        enableVimMode()
    }

    // Update cursor position
    editor.onDidChangeCursorPosition((e) => {
        cursorPositionSpan.textContent = `Ln ${e.position.lineNumber}, Col ${e.position.column}`
    })

    // Auto-save on content change
    let saveTimeout
    editor.onDidChangeModelContent(() => {
        if (currentFile && editor.getValue() !== '') {
            clearTimeout(saveTimeout)
            saveTimeout = setTimeout(async () => {
                try {
                    await safeInvoke('write_file_content', {
                        path: currentFile,
                        content: editor.getValue()
                    })
                    console.log('Auto-saved:', currentFile)
                } catch (error) {
                    console.error('Auto-save failed:', error)
                }
            }, 1000) // Auto-save after 1 second of inactivity
        }
    })
}

// Enable vim mode
function enableVimMode() {
    if (vimMode) {
        vimMode.dispose()
    }
    
    vimMode = initVimMode(editor, document.getElementById('vim-mode'))
    
    // Update vim mode indicator
    const statusNode = document.getElementById('vim-mode')
    const observer = new MutationObserver(() => {
        const mode = statusNode.textContent.toLowerCase()
        statusNode.className = `vim-mode-indicator ${mode}`
    })
    observer.observe(statusNode, { childList: true, subtree: true })
    
    isVimEnabled = true
    vimToggle.classList.add('active')
    vimToggle.textContent = 'Vim Mode'
}

// Disable vim mode
function disableVimMode() {
    if (vimMode) {
        vimMode.dispose()
        vimMode = null
    }
    
    isVimEnabled = false
    vimToggle.classList.remove('active')
    vimToggle.textContent = 'Normal Mode'
    vimModeSpan.textContent = 'NORMAL'
    vimModeSpan.className = 'vim-mode-indicator normal'
}

// Load and display projects
async function loadProjects() {
    try {
        projectList.innerHTML = '<div class="loading">Loading gateways...</div>'
        
        const loadedProjects = await safeInvoke('load_projects')
        projects = loadedProjects
        
        if (projects.length === 0) {
            projectList.innerHTML = '<div class="loading">No gateways found</div>'
            return
        }
        
        projectList.innerHTML = ''
        projects.forEach(gatewayProject => {
            const project = gatewayProject.project
            const projectElement = document.createElement('div')
            projectElement.className = 'project-item'
            projectElement.innerHTML = `
                <span style="color: #007acc;">📁</span>
                <div>
                    <div style="font-weight: 500;">${project.name}</div>
                    <div style="font-size: 11px; color: #888; margin-top: 2px;">
                        ${project.description || 'No description'}
                    </div>
                </div>
            `
            
            projectElement.addEventListener('click', () => {
                selectProject(gatewayProject)
            })
            
            projectList.appendChild(projectElement)
        })
        
    } catch (error) {
        console.error('Failed to load gateways:', error)
        projectList.innerHTML = `<div class="error">Failed to load gateways: ${error}</div>`
    }
}

// Select a project and load its files
async function selectProject(gatewayProject) {
    currentProject = gatewayProject
    
    // Update UI
    document.querySelectorAll('.project-item').forEach(item => {
        item.classList.remove('active')
    })
    event.currentTarget.classList.add('active')
    
    // For now, show project info since we don't have a local path
    const project = gatewayProject.project
    fileExplorer.innerHTML = `
        <div style="padding: 20px; text-align: center;">
            <h3>${project.name}</h3>
            <p style="color: #888; margin: 10px 0;">${project.description}</p>
            <p style="font-size: 12px; color: #666;">
                Project ID: ${project.id}<br/>
                Created: ${new Date(project.created_at).toLocaleDateString()}
            </p>
            <p style="font-size: 12px; color: #999; margin-top: 20px;">
                File browsing will be available in a future update.<br/>
                For now, you can create and manage gateways.
            </p>
        </div>
    `
}

// Load files from a directory
async function loadDirectoryFiles(dirPath) {
    try {
        fileExplorer.innerHTML = '<div class="loading">Loading files...</div>'
        
        const files = await safeInvoke('list_directory', { path: dirPath })
        
        if (files.length === 0) {
            fileExplorer.innerHTML = '<div class="loading">Empty directory</div>'
            return
        }
        
        fileExplorer.innerHTML = ''
        files.forEach(file => {
            const fileElement = document.createElement('div')
            fileElement.className = 'file-item'
            
            const icon = file.is_directory ? '📁' : getFileIcon(file.name)
            const size = file.size ? formatFileSize(file.size) : ''
            
            fileElement.innerHTML = `
                <span class="file-icon">${icon}</span>
                <div style="flex: 1;">
                    <div style="font-weight: ${file.is_directory ? '500' : '400'};">${file.name}</div>
                    ${!file.is_directory && size ? `<div style="font-size: 11px; color: #888;">${size}</div>` : ''}
                </div>
            `
            
            fileElement.addEventListener('click', () => {
                if (file.is_directory) {
                    loadDirectoryFiles(file.path)
                } else {
                    openFile(file.path)
                }
            })
            
            fileExplorer.appendChild(fileElement)
        })
        
    } catch (error) {
        console.error('Failed to load directory:', error)
        fileExplorer.innerHTML = `<div class="error">Failed to load directory: ${error}</div>`
    }
}

// Open a file in the editor
async function openFile(filePath) {
    try {
        const content = await safeInvoke('read_file_content', { path: filePath })
        
        // Set file content in editor
        editor.setValue(content)
        
        // Detect language from file extension
        const language = detectLanguage(filePath)
        monaco.editor.setModelLanguage(editor.getModel(), language)
        
        // Update current file
        currentFile = filePath
        await safeInvoke('set_current_file', { path: filePath })
        
        // Update UI
        const fileName = filePath.split(/[/\\]/).pop()
        currentFileSpan.textContent = fileName
        
        editorTabs.innerHTML = `
            <div class="editor-tab active">
                <span>${fileName}</span>
            </div>
        `
        
        // Update file explorer selection
        document.querySelectorAll('.file-item').forEach(item => {
            item.classList.remove('active')
        })
        
        // Find and highlight the current file
        const fileItems = document.querySelectorAll('.file-item')
        fileItems.forEach(item => {
            if (item.textContent.includes(fileName)) {
                item.classList.add('active')
            }
        })
        
        // Focus editor
        editor.focus()
        
    } catch (error) {
        console.error('Failed to open file:', error)
        alert(`Failed to open file: ${error}`)
    }
}

// Detect programming language from file extension
function detectLanguage(filePath) {
    const ext = filePath.split('.').pop().toLowerCase()
    const languageMap = {
        'js': 'javascript',
        'ts': 'typescript',
        'py': 'python',
        'rs': 'rust',
        'go': 'go',
        'java': 'java',
        'cpp': 'cpp',
        'c': 'c',
        'cs': 'csharp',
        'php': 'php',
        'rb': 'ruby',
        'swift': 'swift',
        'kt': 'kotlin',
        'scala': 'scala',
        'html': 'html',
        'css': 'css',
        'scss': 'scss',
        'sass': 'sass',
        'less': 'less',
        'json': 'json',
        'xml': 'xml',
        'yaml': 'yaml',
        'yml': 'yaml',
        'toml': 'toml',
        'md': 'markdown',
        'txt': 'plaintext',
        'sh': 'shell',
        'bash': 'shell',
        'zsh': 'shell',
        'ps1': 'powershell',
        'sql': 'sql',
        'dockerfile': 'dockerfile'
    }
    
    return languageMap[ext] || 'plaintext'
}

// Get file icon based on extension
function getFileIcon(fileName) {
    const ext = fileName.split('.').pop().toLowerCase()
    const iconMap = {
        'js': '📄',
        'ts': '📘',
        'py': '🐍',
        'rs': '🦀',
        'go': '🐹',
        'java': '☕',
        'cpp': '⚡',
        'c': '⚡',
        'html': '🌐',
        'css': '🎨',
        'json': '📋',
        'md': '📝',
        'txt': '📄',
        'pdf': '📕',
        'img': '🖼️',
        'png': '🖼️',
        'jpg': '🖼️',
        'jpeg': '🖼️',
        'gif': '🖼️',
        'svg': '🖼️'
    }
    
    if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'ico'].includes(ext)) {
        return '🖼️'
    }
    
    return iconMap[ext] || '📄'
}

// OAuth completion handler
async function waitForOAuthCompletion() {
    return new Promise((resolve, reject) => {
        let pollInterval
        let messageListener
        let timeoutId
        
        console.log('🔍 Waiting for OAuth completion...')
        
        // Method 1: Listen for postMessage from OAuth window
        messageListener = async (event) => {
            if (event.data && event.data.type === 'OAUTH_SUCCESS') {
                console.log('✅ Received OAuth token via postMessage')
                cleanup()
                await handleOAuthSuccess(event.data.token, event.data.user, event.data.email)
                resolve(event.data)
            }
        }
        window.addEventListener('message', messageListener)
        
        // Method 2: Poll localStorage for token
        pollInterval = setInterval(async () => {
            const token = localStorage.getItem('mothership_oauth_token')
            const timestamp = localStorage.getItem('mothership_oauth_timestamp')
            
            console.log('🔍 Polling for OAuth token...', { 
                hasToken: !!token, 
                hasTimestamp: !!timestamp,
                tokenLength: token ? token.length : 0
            })
            
            if (token && timestamp) {
                const age = Date.now() - parseInt(timestamp)
                console.log('⏰ Token age:', age, 'ms')
                
                if (age < 60000) { // Token is less than 60 seconds old
                    console.log('✅ Found valid OAuth token in localStorage')
                    const user = localStorage.getItem('mothership_oauth_user') || ''
                    const email = localStorage.getItem('mothership_oauth_email') || ''
                    
                    console.log('👤 User info:', { user, email })
                    
                    // Clean up localStorage
                    localStorage.removeItem('mothership_oauth_token')
                    localStorage.removeItem('mothership_oauth_user')
                    localStorage.removeItem('mothership_oauth_email')
                    localStorage.removeItem('mothership_oauth_timestamp')
                    
                    cleanup()
                    await handleOAuthSuccess(token, user, email)
                    resolve({ token, user, email })
                } else {
                    console.log('⚠️ Token too old, ignoring')
                }
            }
        }, 2000)
        
        // Cleanup function
        function cleanup() {
            if (pollInterval) clearInterval(pollInterval)
            if (messageListener) window.removeEventListener('message', messageListener)
            if (timeoutId) clearTimeout(timeoutId)
        }
        
        // Timeout after 5 minutes
        timeoutId = setTimeout(() => {
            cleanup()
            showAuthMessage('OAuth timeout - please try again', 'error')
            reject(new Error('OAuth timeout'))
        }, 300000)
    })
}

// Handle successful OAuth
async function handleOAuthSuccess(token, user, email) {
    try {
        console.log('🎉 Processing OAuth success for user:', user)
        
        // Save the token and credentials persistently
        await safeInvoke('save_auth_token', { token })
        
        console.log('✅ Token and credentials saved successfully')
        
        // Update authentication state
        isAuthenticated = true
        updateAuthUI()
        
    } catch (error) {
        console.error('Failed to save OAuth token:', error)
        throw error
    }
}

// Authentication Functions
async function tryAutoLogin() {
    console.log('🔐 Attempting auto-login...')
    
    if (!isTauri) {
        console.log('❌ Auto-login not available in browser mode')
        return false
    }
    
    try {
        // Show loading message during auto-login
        showAuthMessage('Checking stored credentials...', 'success')
        
        console.log('🔍 Invoking auto_login...')
        const loginSuccess = await safeInvoke('auto_login')
        
        if (loginSuccess) {
            console.log('🎉 Auto-login successful!')
            isAuthenticated = true
            updateAuthUI()
            
            // Show success message briefly
            showAuthMessage('Welcome back! Automatically signed in.', 'success')
            
            // Load projects after auto-login
            setTimeout(async () => {
                try {
                    await loadProjects()
                    console.log('✅ Projects loaded after auto-login')
                } catch (error) {
                    console.error('❌ Failed to load projects after auto-login:', error)
                }
            }, 1000)
            
            return true
        } else {
            console.log('❌ Auto-login failed or no stored credentials')
            // Clear the loading message if auto-login failed
            authMessage.innerHTML = ''
            return false
        }
    } catch (error) {
        console.error('❌ Auto-login error:', error)
        // Clear the loading message on error
        authMessage.innerHTML = ''
        return false
    }
}

async function checkAuthStatus() {
    console.log('🔐 Checking authentication status...')
    console.log('   - isTauri:', isTauri)
    
    if (!isTauri) {
        console.log('❌ Not in Tauri context - skipping auth check')
        isAuthenticated = false
        updateAuthUI()
        return false
    }
    
    try {
        console.log('🔍 Invoking check_auth_status...')
        isAuthenticated = await safeInvoke('check_auth_status')
        console.log('✅ Auth status result:', isAuthenticated)
        updateAuthUI()
        return isAuthenticated
    } catch (error) {
        console.error('❌ Failed to check auth status:', error)
        isAuthenticated = false
        updateAuthUI()
        return false
    }
}

function updateAuthUI() {
    console.log('🎨 Updating auth UI - isAuthenticated:', isAuthenticated)
    
    if (!authOverlay) {
        console.error('❌ Auth overlay element not found!')
        return
    }
    
    console.log('🎨 Auth overlay element:', authOverlay)
    console.log('🎨 Auth overlay classes before:', authOverlay.className)
    
    if (isAuthenticated) {
        console.log('✅ User authenticated - hiding auth overlay')
        authOverlay.classList.add('hidden')
        
        if (authBtn) {
            authBtn.textContent = 'Logout'
            authBtn.style.background = '#28a745'
        }
        
        console.log('🎨 Auth overlay classes after hiding:', authOverlay.className)
        console.log('🎨 Auth overlay should now be hidden')
    } else {
        console.log('❌ User not authenticated - showing auth overlay')
        authOverlay.classList.remove('hidden')
        
        if (authBtn) {
            authBtn.textContent = 'Authenticate'
            authBtn.style.background = '#404040'
        }
        
        console.log('🎨 Auth overlay classes after showing:', authOverlay.className)
    }
}

function showAuthMessage(message, type = 'error') {
    authMessage.innerHTML = `<div class="auth-${type}">${message}</div>`
    setTimeout(() => {
        authMessage.innerHTML = ''
    }, 5000)
}

async function handleGoogleLogin() {
    console.log('🚀 Starting Google OAuth login...')
    
    try {
        googleLoginBtn.disabled = true
        googleLoginBtn.innerHTML = `
            <svg width="18" height="18" viewBox="0 0 24 24">
                <circle cx="12" cy="12" r="10" stroke="white" stroke-width="2" fill="none" opacity="0.5"/>
            </svg>
            Signing in...
        `
        
        showAuthMessage('Opening Google login in your browser...', 'success')
        
        // Start OAuth flow
        console.log('🔍 Invoking start_google_oauth command...')
        const oauthData = await safeInvoke('start_google_oauth')
        console.log('✅ OAuth data received:', oauthData)
        
        showAuthMessage('Complete the login in your browser. The authentication will complete automatically...', 'success')
        
        // Wait for OAuth completion
        showAuthMessage('Complete the login in your browser. The authentication will complete automatically...', 'success')
        
        // Poll to check if authentication completed
        const pollForAuth = setInterval(async () => {
            try {
                const isAuth = await safeInvoke('check_auth_status')
                if (isAuth) {
                    console.log('🎉 OAuth authentication completed!')
                    clearInterval(pollForAuth)
                    
                    // Authentication was successful
                    isAuthenticated = true
                    console.log('🔄 Updating UI after authentication...')
                    updateAuthUI()
                    showAuthMessage('Successfully authenticated! Loading gateways...', 'success')
                    
                    // Load gateways after successful authentication
                    console.log('📁 Loading gateways...')
                    setTimeout(async () => {
                        try {
                            await loadProjects()
                            console.log('✅ Gateways loaded successfully')
                        } catch (error) {
                            console.error('❌ Failed to load gateways:', error)
                            showAuthMessage(`Failed to load gateways: ${error}`, 'error')
                        }
                    }, 1000)
                }
            } catch (error) {
                console.error('Error checking auth status:', error)
            }
        }, 2000)
        
        // Timeout after 5 minutes
        setTimeout(() => {
            clearInterval(pollForAuth)
            showAuthMessage('OAuth timeout - please try again', 'error')
        }, 300000)
        
    } catch (error) {
        console.error('Google login failed:', error)
        showAuthMessage(`Login failed: ${error}`, 'error')
    } finally {
        googleLoginBtn.disabled = false
        googleLoginBtn.innerHTML = `
            <svg width="18" height="18" viewBox="0 0 24 24">
                <path fill="white" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
                <path fill="white" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/>
                <path fill="white" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/>
                <path fill="white" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/>
            </svg>
            Continue with Google
        `
    }
}

async function handleEmailLogin(event) {
    event.preventDefault()
    
    const email = document.getElementById('email').value
    const password = document.getElementById('password').value
    
    try {
        emailLoginBtn.disabled = true
        emailLoginBtn.textContent = 'Signing in...'
        
        const result = await safeInvoke('authenticate_with_username_password', { email, password })
        
        isAuthenticated = true
        updateAuthUI()
        showAuthMessage('Successfully authenticated!', 'success')
        
        // Load gateways after successful authentication
        setTimeout(() => {
            loadProjects()
        }, 1000)
        
    } catch (error) {
        showAuthMessage(`Login failed: ${error}`, 'error')
    } finally {
        emailLoginBtn.disabled = false
        emailLoginBtn.textContent = 'Sign In'
    }
}

async function handleLogout() {
    try {
        await safeInvoke('logout')
        isAuthenticated = false
        updateAuthUI()
        
        // Clear gateways and files
        projects = []
        currentProject = null
        currentFile = null
        projectList.innerHTML = '<div class="loading">No gateways loaded</div>'
        fileExplorer.innerHTML = '<div class="loading">Select a gateway</div>'
        editor.setValue('// Welcome to Mothership\n// Sign in to access your gateways')
        currentFileSpan.textContent = 'No file selected'
        
    } catch (error) {
        console.error('Logout failed:', error)
    }
}

// Format file size
function formatFileSize(bytes) {
    if (bytes === 0) return '0 B'
    const k = 1024
    const sizes = ['B', 'KB', 'MB', 'GB']
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

// Event listeners
authBtn.addEventListener('click', async () => {
    if (isAuthenticated) {
        await handleLogout()
    } else {
        // Show auth modal
        authOverlay.classList.remove('hidden')
    }
})

// Authentication modal event listeners
document.querySelectorAll('.auth-tab').forEach(tab => {
    tab.addEventListener('click', () => {
        // Update active tab
        document.querySelectorAll('.auth-tab').forEach(t => t.classList.remove('active'))
        tab.classList.add('active')
        
        // Show corresponding section
        document.querySelectorAll('.auth-section').forEach(section => section.classList.remove('active'))
        const targetSection = document.getElementById(`${tab.dataset.tab}-section`)
        if (targetSection) {
            targetSection.classList.add('active')
        }
    })
})

googleLoginBtn.addEventListener('click', handleGoogleLogin)

githubLoginBtn.addEventListener('click', () => {
    showAuthMessage('GitHub login is coming soon! Use Google login for now.', 'error')
})

emailLoginForm.addEventListener('submit', handleEmailLogin)

forgotPasswordLink.addEventListener('click', (e) => {
    e.preventDefault()
    showAuthMessage('Password recovery is not yet implemented. Please use Google login.', 'error')
})

createAccountLink.addEventListener('click', (e) => {
    e.preventDefault()
    showAuthMessage('Account creation is not yet implemented. Please use Google login.', 'error')
})

refreshBtn.addEventListener('click', () => {
    loadProjects()
    // Note: File browsing not yet implemented for projects
})

checkpointBtn.addEventListener('click', async () => {
    if (!currentFile) {
        alert('No file is currently open')
        return
    }
    
    const message = prompt('Checkpoint message (optional):') || 'Auto checkpoint'
    
    try {
        checkpointBtn.disabled = true
        checkpointBtn.textContent = 'Creating...'
        
        await safeInvoke('create_checkpoint', { message })
        
        checkpointBtn.textContent = 'Checkpoint ✓'
        checkpointBtn.style.background = '#28a745'
        
        setTimeout(() => {
            checkpointBtn.textContent = 'Checkpoint'
            checkpointBtn.style.background = '#404040'
        }, 2000)
        
    } catch (error) {
        console.error('Checkpoint failed:', error)
        alert(`Checkpoint failed: ${error}`)
    } finally {
        checkpointBtn.disabled = false
    }
})

vimToggle.addEventListener('click', async () => {
    try {
        const newVimMode = await safeInvoke('toggle_vim_mode')
        
        if (newVimMode) {
            enableVimMode()
        } else {
            disableVimMode()
        }
        
    } catch (error) {
        console.error('Failed to toggle vim mode:', error)
    }
})

// Gateway creation functions
function showGatewayModal() {
    gatewayOverlay.classList.remove('hidden')
    gatewayNameInput.focus()
}

function hideGatewayModal() {
    gatewayOverlay.classList.add('hidden')
    clearGatewayForm()
}

function clearGatewayForm() {
    gatewayForm.reset()
    gatewayMessage.innerHTML = ''
}

function showGatewayMessage(message, type = 'error') {
    gatewayMessage.innerHTML = `<div class="gateway-${type}">${message}</div>`
    setTimeout(() => {
        gatewayMessage.innerHTML = ''
    }, 5000)
}

async function browseForDirectory() {
    try {
        if (!isTauri) {
            // Fallback for browser mode
            const suggestedPaths = [
                '~/Projects',
                '~/Code',
                '~/Development',
                '~/Desktop',
                './current-directory'
            ]
            
            const selectedPath = prompt(
                'Enter project directory path:\n\nSuggested locations:\n' + 
                suggestedPaths.join('\n'),
                '~/Projects/my-project'
            )
            
            if (selectedPath) {
                gatewayPathInput.value = selectedPath
            }
            return
        }
        
        console.log('🔍 Opening native directory picker...');
        
        // Use our custom Tauri command which is more reliable
        const selected = await safeInvoke('open_directory_dialog');
        
        if (selected) {
            console.log('✅ Directory selected:', selected);
            gatewayPathInput.value = selected;
        } else {
            console.log('❌ No directory selected (user cancelled)');
        }
        
    } catch (error) {
        console.error('Failed to open directory picker:', error);
        
        // Provide detailed error information
        console.log('Error details:', {
            message: error.message,
            isTauri,
            hasInvoke: typeof window.__TAURI_INTERNALS__ !== 'undefined',
            availableCommands: 'open_directory_dialog should be available'
        });
        
        // Fallback to prompt if native dialog fails
        showGatewayMessage('Native directory picker failed, using fallback input...', 'error');
        
        const suggestedPaths = [
            '~/Projects',
            '~/Code',
            '~/Development',
            '~/Desktop',
            './current-directory'
        ]
        
        const selectedPath = prompt(
            'Enter project directory path:\n\nSuggested locations:\n' + 
            suggestedPaths.join('\n'),
            '~/Projects/my-project'
        )
        
        if (selectedPath) {
            gatewayPathInput.value = selectedPath
        }
    }
}

async function handleCreateGateway(event) {
    event.preventDefault()
    
    const name = gatewayNameInput.value.trim()
    const description = gatewayDescriptionInput.value.trim()
    const projectPath = gatewayPathInput.value.trim()
    
    if (!name || !projectPath) {
        showGatewayMessage('Please fill in all required fields', 'error')
        return
    }
    
    try {
        createGatewayBtn.disabled = true
        createGatewayBtn.textContent = 'Creating...'
        
        showGatewayMessage('Creating gateway...', 'success')
        
        // Create the gateway via Tauri command
        const newProject = await safeInvoke('create_gateway', {
            request: {
                name,
                description: description || `Gateway for ${name}`,
                project_path: projectPath
            }
        })
        
        console.log('✅ Gateway created:', newProject)
        showGatewayMessage('Gateway created successfully! Opening...', 'success')
        
        // Refresh the project list and auto-select the new gateway
        setTimeout(async () => {
            hideGatewayModal()
            try {
                await loadProjects()
                
                // Find and select the newly created gateway
                const gatewayProject = projects.find(gp => gp.project.id === newProject.id)
                if (gatewayProject) {
                    console.log('🎯 Auto-selecting newly created gateway:', gatewayProject.project.name)
                    
                    // Find the corresponding project element and trigger click
                    const projectElements = document.querySelectorAll('.project-item')
                    projectElements.forEach((element, index) => {
                        if (projects[index] && projects[index].project.id === newProject.id) {
                            element.click()
                            element.scrollIntoView({ behavior: 'smooth', block: 'center' })
                            
                            // Add a brief highlight effect
                            element.style.border = '2px solid #007acc'
                            element.style.background = 'rgba(0, 122, 204, 0.1)'
                            setTimeout(() => {
                                element.style.border = ''
                                element.style.background = ''
                            }, 3000)
                        }
                    })
                } else {
                    console.warn('⚠️ Could not find newly created gateway in projects list')
                }
            } catch (error) {
                console.error('Failed to refresh projects:', error)
            }
        }, 1000)
        
    } catch (error) {
        console.error('Gateway creation failed:', error)
        showGatewayMessage(`Failed to create gateway: ${error}`, 'error')
    } finally {
        createGatewayBtn.disabled = false
        createGatewayBtn.textContent = 'Create Gateway'
    }
}

// Gateway creation event listeners
newGatewayBtn.addEventListener('click', () => {
    if (!isAuthenticated) {
        showAuthMessage('Please authenticate first to create gateways', 'error')
        authOverlay.classList.remove('hidden')
        return
    }
    showGatewayModal()
})

gatewayForm.addEventListener('submit', handleCreateGateway)

cancelGatewayBtn.addEventListener('click', hideGatewayModal)

browsePathBtn.addEventListener('click', browseForDirectory)

// Close gateway modal when clicking outside
gatewayOverlay.addEventListener('click', (e) => {
    if (e.target === gatewayOverlay) {
        hideGatewayModal()
    }
})

// Close gateway modal with Escape key
document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !gatewayOverlay.classList.contains('hidden')) {
        hideGatewayModal()
    }
})

// Initialize application
document.addEventListener('DOMContentLoaded', async () => {
    initializeEditor()
    
    // Initialize Tauri detection first
    console.log('🔍 Initializing Tauri detection...')
    detectTauri()
    
    // Debug Tauri context
    console.log('🔍 Tauri context detected:', isTauri)
    
    // Try auto-login first
    const autoLoginSuccess = await tryAutoLogin()
    
    if (!autoLoginSuccess) {
        // Auto-login failed, check auth status normally
        console.log('🔐 Auto-login failed, checking auth status...')
        await checkAuthStatus()
        
        // Load initial state if authenticated
        if (isAuthenticated) {
            try {
                const state = await safeInvoke('get_editor_state')
                if (state.projects.length > 0) {
                    projects = state.projects
                    loadProjects()
                }
            } catch (error) {
                console.error('Failed to load initial state:', error)
            }
        }
    }
    
    // Only show warning if actually not in Tauri
    if (!isTauri) {
        console.warn('⚠️ Running in browser mode - OAuth authentication requires the Tauri desktop app')
        const contextWarning = document.createElement('div')
        contextWarning.style.cssText = `
            position: fixed; top: 0; left: 0; right: 0; 
            background: #ff9800; color: white; padding: 8px; 
            text-align: center; z-index: 1000; font-size: 14px;
        `
        contextWarning.textContent = '⚠️ Running in browser mode - some features require the Tauri desktop app'
        document.body.appendChild(contextWarning)
    }
})

// Handle window resize
window.addEventListener('resize', () => {
    if (editor) {
        editor.layout()
    }
})

// Debug helper function (can be called from browser console)
window.debugCredentials = async function() {
    try {
        console.log('🔍 === DEBUGGING CREDENTIALS ===')
        const result = await safeInvoke('debug_credentials_file')
        console.log(result)
        return result
    } catch (error) {
        console.error('Debug failed:', error)
        return error.toString()
    }
} 