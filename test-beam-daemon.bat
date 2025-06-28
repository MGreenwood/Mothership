@echo off
echo 🚀 Testing Mothership Beam with Automatic Daemon Startup
echo.

echo 📋 Step 1: Check if daemon is already running
echo Checking http://localhost:7525/health...
curl -s http://localhost:7525/health 2>nul
if %errorlevel% equ 0 (
    echo ✅ Daemon is already running
) else (
    echo ❌ Daemon is not running
)
echo.

echo 📋 Step 2: Test beam command with automatic daemon startup
echo Running: cargo run --bin mothership beam "test-project"
echo.
cargo run --bin mothership beam "test-project"
echo.

echo 📋 Step 3: Check if daemon started successfully
echo Checking http://localhost:7525/health again...
curl -s http://localhost:7525/health
if %errorlevel% equ 0 (
    echo ✅ Daemon is now running!
    echo.
    echo 📊 Getting daemon status:
    curl -s http://localhost:7525/status
    echo.
    echo.
    echo 📁 Listing tracked projects:
    curl -s http://localhost:7525/projects
) else (
    echo ❌ Daemon failed to start
)
echo.

echo 🎉 Test complete! The beam command should now automatically start the daemon.
pause 