use anyhow::Result;
use std::env;
use tracing::info;

mod daemon;
mod file_watcher;
mod ipc_server;
mod project_scanner;
mod system_tray;
mod windows_service;

use daemon::MothershipDaemon;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "mothership_daemon=info,tower_http=debug".to_string()),
        )
        .init();

    info!("🚀 Mothership Daemon starting...");

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    
    match args.get(1).map(|s| s.as_str()) {
        Some("install") => {
            // Install as Windows service
            #[cfg(windows)]
            {
                info!("Installing Mothership Daemon as Windows service...");
                windows_service::install_service()?;
                println!("✅ Mothership Daemon service installed successfully!");
                println!("💡 Use 'sc start MothershipDaemon' to start the service");
            }
            #[cfg(not(windows))]
            {
                error!("Service installation only supported on Windows");
            }
        }
        Some("uninstall") => {
            // Uninstall Windows service
            #[cfg(windows)]
            {
                info!("Uninstalling Mothership Daemon Windows service...");
                windows_service::uninstall_service()?;
                println!("✅ Mothership Daemon service uninstalled successfully!");
            }
            #[cfg(not(windows))]
            {
                error!("Service uninstallation only supported on Windows");
            }
        }
        Some("service") => {
            // Run as Windows service (called by Service Control Manager)
            #[cfg(windows)]
            {
                info!("Starting as Windows service...");
                windows_service::run_service().await?;
            }
            #[cfg(not(windows))]
            {
                error!("Service mode only supported on Windows");
            }
        }
        Some("--help") | Some("-h") => {
            print_help();
        }
        _ => {
            // Run as standalone application (for development/testing)
            info!("Running as standalone application...");
            
            // Create and start the daemon
            let daemon = MothershipDaemon::new().await?;
            
            info!("✅ Mothership Daemon started successfully!");
            info!("🔍 Scanning for Mothership projects...");
            info!("🎯 System tray icon should appear in notification area");
            
            // Run the daemon
            daemon.run().await?;
        }
    }

    Ok(())
}

fn print_help() {
    println!("Mothership Daemon - Background file synchronization service");
    println!();
    println!("USAGE:");
    println!("    mothership-daemon [SUBCOMMAND]");
    println!();
    println!("SUBCOMMANDS:");
    println!("    install      Install as Windows service (requires admin privileges)");
    println!("    uninstall    Uninstall Windows service (requires admin privileges)");
    println!("    service      Run as Windows service (internal use by Service Control Manager)");
    println!("    --help, -h   Show this help message");
    println!();
    println!("EXAMPLES:");
    println!("    # Run as standalone application (for testing)");
    println!("    mothership-daemon");
    println!();
    println!("    # Install as Windows service");
    println!("    mothership-daemon install");
    println!();
    println!("    # Start the service");
    println!("    sc start MothershipDaemon");
    println!();
    println!("NOTES:");
    println!("    • The daemon automatically discovers Mothership projects in common directories");
    println!("    • A system tray icon provides status and controls");
    println!("    • File changes are synchronized in real-time with the Mothership server");
    println!("    • The daemon listens on http://localhost:7525 for CLI communication");
} 