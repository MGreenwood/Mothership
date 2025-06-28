# 🧪 Testing CLI Distribution System

This guide shows how to set up and test the self-hosted CLI distribution system for beta testers.

## 🏗️ **Setup for Distribution**

### **1. Build CLI Binaries**

First, build the CLI for all supported platforms:

```bash
# Make the build script executable and run it
chmod +x scripts/build-for-distribution.sh
./scripts/build-for-distribution.sh
```

This creates a `cli-binaries/` directory structure:
```
cli-binaries/
└── 0.1.0/
    ├── x86_64-unknown-linux-gnu/
    │   ├── mothership
    │   └── mothership-daemon
    ├── x86_64-apple-darwin/
    │   ├── mothership
    │   └── mothership-daemon
    ├── x86_64-pc-windows-msvc/
    │   ├── mothership.exe
    │   └── mothership-daemon.exe
    └── ...
```

### **2. Start Mothership Server**

Make sure your Mothership server is running with the new CLI distribution endpoints:

```bash
# Start the server stack
./start-docker.sh

# Or run the server directly
cargo run --bin mothership-server
```

The server now serves CLI distribution endpoints at:
- `/cli/install` - Installation script
- `/cli/latest` - Latest version info
- `/cli/versions` - All available versions
- `/cli/download/{version}/{platform}/{binary}` - Binary downloads

## 🧪 **Testing the Distribution System**

### **Test 1: Check Server Endpoints**

```bash
# Check if CLI endpoints are working
curl http://localhost:7523/cli/latest

# Should return something like:
# {"version":"0.1.0","platforms":["x86_64-unknown-linux-gnu",...],"release_date":"...","changes":["..."]}
```

### **Test 2: Test Installation Script**

**Unix (macOS/Linux):**
```bash
# Test the installation script
curl -sSL http://localhost:7523/cli/install | bash
```

**Windows (PowerShell):**
```powershell
# Test the Windows installation script
irm http://localhost:7523/cli/install/windows | iex
```

### **Test 3: Test Update Command**

After installation, test the update functionality:

```bash
# Check for updates
mothership update --check-only

# List available versions
mothership update --list-versions

# Force update (for testing)
mothership update --force
```

## 🎯 **Beta Tester Experience**

Here's what your beta testers will experience:

### **Initial Installation**

**For Unix users:**
```bash
curl -sSL https://your-server.com/cli/install | bash
```

**For Windows users:**
```powershell
irm https://your-server.com/cli/install/windows | iex
```

### **The installation script will:**
1. ✅ Detect their platform automatically
2. ✅ Download latest CLI and daemon
3. ✅ Install to appropriate location
4. ✅ Configure server URL in `~/.config/mothership/config.toml`
5. ✅ Add binaries to PATH
6. ✅ Verify installation works

### **Staying Updated**

```bash
# Check for updates (shows changelog)
mothership update --check-only

# Update to latest version
mothership update

# Update to specific version
mothership update --version 0.2.0
```

## 🔧 **Configuration for Beta Testing**

### **Server Configuration**

Set the server URL for your beta environment:

```bash
# In your .env file or environment
export MOTHERSHIP_SERVER_URL="https://beta.yourcompany.com"
```

### **Authentication Required**

Beta testers need to authenticate before downloading:

```bash
# After installation, testers must authenticate
mothership auth

# Then they can use update commands
mothership update
```

## 🚀 **Deployment Scenarios**

### **Scenario 1: Company Internal**

```bash
# Company sets up internal server
export MOTHERSHIP_SERVER_URL="https://mothership.company.com"

# Employees install CLI from company server
curl -sSL https://mothership.company.com/cli/install | bash

# All updates come from company server
# No external dependencies
```

### **Scenario 2: Beta Testing Group**

```bash
# Set up staging server
export MOTHERSHIP_SERVER_URL="https://beta-mothership.yourcompany.com"

# Invite beta testers with one-liner
# Send them: curl -sSL https://beta-mothership.yourcompany.com/cli/install | bash

# Push updates by building new binaries and restarting server
# Testers get notified: "Update available: 0.1.0 → 0.2.0"
```

### **Scenario 3: Development Team**

```bash
# Local development server
export MOTHERSHIP_SERVER_URL="http://localhost:7523"

# Team members install from local builds
curl -sSL http://localhost:7523/cli/install | bash

# Rapid iteration - rebuild and test immediately
./scripts/build-for-distribution.sh
mothership update --force
```

## 📝 **Update Loop Workflow**

### **For Developers:**

1. **Make changes to CLI**
2. **Build new binaries:**
   ```bash
   ./scripts/build-for-distribution.sh
   ```
3. **Update version in Cargo.toml** (if needed)
4. **Restart server** (picks up new binaries)
5. **Notify testers:** "New version available!"

### **For Beta Testers:**

1. **Receive notification** (via Slack/email/Discord)
2. **Check what's new:**
   ```bash
   mothership update --check-only
   ```
3. **Update when ready:**
   ```bash
   mothership update
   ```
4. **Continue testing with latest features**

## 🔍 **Troubleshooting**

### **Common Issues:**

**"Binary not found" error:**
- Ensure `cli-binaries/` directory is in server root
- Check server logs for file permissions
- Verify platform detection is correct

**"Authentication required" error:**
- Testers need to run `mothership auth` first
- Check if JWT tokens are valid
- Verify server authentication endpoints

**"Update failed" error:**
- Check file permissions for binary installation
- On Unix: may need `sudo` for `/usr/local/bin` writes
- On Windows: may need admin rights for PATH modification

### **Debug Commands:**

```bash
# Check server endpoints
curl -v http://localhost:7523/cli/latest

# Check current CLI config
cat ~/.config/mothership/config.toml

# Check daemon status
mothership daemon status

# Force reinstall
curl -sSL http://localhost:7523/cli/install | bash
```

## 🎉 **Success Metrics**

You'll know the system is working when:

- ✅ **One-liner install works** on all platforms
- ✅ **Update notifications show** when new versions available  
- ✅ **Seamless updates** without breaking existing workflows
- ✅ **Beta testers stay current** with latest features
- ✅ **No external dependencies** - everything from your server

## 🚀 **Next Steps**

This system gives you:

1. **Complete control** over CLI distribution
2. **Authentication-gated access** for security
3. **Platform-specific binaries** served automatically  
4. **Seamless update experience** for testers
5. **No external services** required

Perfect for private beta testing with a controlled group of users! 🎯 