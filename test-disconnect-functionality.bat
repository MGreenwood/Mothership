@echo off
echo 🎯 Testing Mothership Disconnect and Daemon Management
echo =====================================================
echo.

echo 📋 Step 1: Check initial daemon status
echo Running: cargo run --bin mothership daemon status
cargo run --bin mothership daemon status
echo.

echo 📋 Step 2: Start a daemon by beaming into poop project
echo Running: cargo run --bin mothership beam "poop" --local-dir .
cargo run --bin mothership beam "poop" --local-dir .
echo.

echo 📋 Step 3: Check daemon status after beam
echo Running: cargo run --bin mothership daemon status
cargo run --bin mothership daemon status
echo.

echo 📋 Step 4: Test disconnect from poop project
echo Running: cargo run --bin mothership disconnect "poop"
cargo run --bin mothership disconnect "poop"
echo.

echo 📋 Step 5: Check daemon status after disconnect
echo Running: cargo run --bin mothership daemon status
cargo run --bin mothership daemon status
echo.

echo 📋 Step 6: Test daemon stop
echo Running: cargo run --bin mothership daemon stop
cargo run --bin mothership daemon stop
echo.

echo 📋 Step 7: Check daemon status after stop
echo Running: cargo run --bin mothership daemon status
cargo run --bin mothership daemon status
echo.

echo 📋 Step 8: Test daemon restart
echo Running: cargo run --bin mothership daemon restart
cargo run --bin mothership daemon restart
echo.

echo 📋 Step 9: Final daemon status check
echo Running: cargo run --bin mothership daemon status
cargo run --bin mothership daemon status
echo.

echo 🎉 Disconnect and daemon management test complete!
echo.
echo 📊 Summary of new commands:
echo   - mothership disconnect [project]     # Disconnect from project tracking
echo   - mothership daemon status           # Show daemon status and projects
echo   - mothership daemon stop             # Stop the background daemon
echo   - mothership daemon restart          # Restart the daemon
echo.
pause 