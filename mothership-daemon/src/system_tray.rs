use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::daemon::DaemonStatus;

#[cfg(windows)]
use {
    tray_icon::{
        menu::{Menu, MenuEvent, MenuItemBuilder},
        TrayIconBuilder, TrayIconEvent, Icon,
    },
    winit::{
        event_loop::{ControlFlow, EventLoopBuilder},
        platform::windows::EventLoopBuilderExtWindows,
    },
    image,
};

/// System tray integration for Windows
pub struct SystemTray {
    status: Arc<RwLock<DaemonStatus>>,
}

#[cfg(windows)]
fn load_tray_icon() -> Option<Icon> {
    // Try to load the custom icon from mothership-gui/icons/icon.png
    let icon_paths = [
        "../mothership-gui/icons/icon.png",
        "mothership-gui/icons/icon.png", 
        "./mothership-gui/icons/icon.png",
        "../../mothership-gui/icons/icon.png",
    ];
    
    for icon_path in &icon_paths {
        if let Ok(img) = image::open(icon_path) {
            // Convert to RGBA8 format expected by tray-icon
            let rgba_img = img.to_rgba8();
            let (width, height) = rgba_img.dimensions();
            let rgba_data = rgba_img.into_raw();
            
            match Icon::from_rgba(rgba_data, width, height) {
                Ok(icon) => {
                    info!("✅ Loaded custom tray icon from: {}", icon_path);
                    return Some(icon);
                }
                Err(e) => {
                    info!("⚠️ Failed to create icon from {}: {}", icon_path, e);
                }
            }
        }
    }
    
    info!("📋 Using default system tray icon (custom icon not found)");
    None
}

impl SystemTray {
    /// Create a new system tray instance
    pub fn new(status: Arc<RwLock<DaemonStatus>>) -> Result<Self> {
        info!("🖥️ Initializing system tray...");
        Ok(Self { status })
    }

    /// Run the system tray (Windows only)
    pub async fn run(self) -> Result<()> {
        #[cfg(windows)]
        {
            info!("🖥️ Starting Mothership system tray icon...");
            
            // Clone status for thread-safe access
            let status = self.status.clone();
            
            // Spawn the system tray in a dedicated std::thread
            let tray_handle = std::thread::spawn(move || {
                // Create event loop for tray (using any_thread for Windows compatibility)
                let event_loop = match EventLoopBuilder::new().with_any_thread(true).build() {
                    Ok(loop_) => loop_,
                    Err(e) => {
                        error!("Failed to create event loop: {}", e);
                        return;
                    }
                };

                // Create tray menu
                let tray_menu = Menu::new();
                
                // Status item
                let status_item = MenuItemBuilder::new()
                    .text("📊 Show Status")
                    .id("status".into())
                    .build();
                
                // Control items
                let stop_item = MenuItemBuilder::new()
                    .text("⏹️ Stop Daemon")
                    .id("stop".into())
                    .build();
                
                let restart_item = MenuItemBuilder::new()
                    .text("🔄 Restart Daemon")
                    .id("restart".into())
                    .build();
                
                // Exit item
                let exit_item = MenuItemBuilder::new()
                    .text("❌ Exit")
                    .id("exit".into())
                    .build();
                
                // Add items to menu
                let _ = tray_menu.append(&status_item);
                let _ = tray_menu.append(&stop_item);
                let _ = tray_menu.append(&restart_item);
                let _ = tray_menu.append(&exit_item);

                // Create tray icon with custom icon if available
                let mut tray_builder = TrayIconBuilder::new()
                    .with_menu(Box::new(tray_menu))
                    .with_tooltip("Mothership");
                
                // Try to use custom icon
                if let Some(custom_icon) = load_tray_icon() {
                    tray_builder = tray_builder.with_icon(custom_icon);
                }
                
                let _tray_icon = match tray_builder.build() {
                    Ok(icon) => icon,
                    Err(e) => {
                        error!("Failed to create tray icon: {}", e);
                        return;
                    }
                };

                info!("✅ System tray icon created successfully");

                // Run the event loop
                let result = event_loop.run(move |_event, elwt| {
                    elwt.set_control_flow(ControlFlow::Wait);

                    // Handle menu events
                    while let Ok(event) = MenuEvent::receiver().try_recv() {
                        match event.id.as_ref() {
                            "status" => {
                                info!("📊 Status menu item clicked");
                                // Use blocking read for status since we're in std::thread
                                match status.blocking_read() {
                                    status => {
                                        info!("📊 Daemon Status: {} projects tracked, running: {}", 
                                            status.projects_tracked, status.is_running);
                                        info!("📊 Files syncing: {}, Server connected: {}", 
                                            status.files_syncing, status.server_connected);
                                    }
                                }
                            }
                            "stop" => {
                                info!("⏹️ Stop daemon requested from system tray");
                                // Graceful exit through event loop
                                elwt.exit();
                            }
                            "restart" => {
                                info!("🔄 Restart daemon requested from system tray");
                                // For restart, we'll exit and let the system handle restart
                                elwt.exit();
                            }
                            "exit" => {
                                info!("❌ Exit daemon requested from system tray");
                                elwt.exit();
                            }
                            _ => {}
                        }
                    }

                    // Handle tray icon events
                    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                        match event {
                            TrayIconEvent::Click { .. } => {
                                info!("🖱️ Tray icon clicked - showing status");
                                match status.blocking_read() {
                                    status => {
                                        info!("📊 Status: {} projects tracked, daemon running: {}", 
                                            status.projects_tracked, status.is_running);
                                        if status.projects_tracked > 0 {
                                            info!("📁 Active file watching for {} projects", status.projects_tracked);
                                        } else {
                                            info!("📁 No projects currently tracked");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                });
                
                match result {
                    Ok(_) => info!("📱 System tray event loop exited normally"),
                    Err(e) => error!("📱 System tray event loop error: {}", e),
                }
            });

            // Wait for the tray thread to complete
            tokio::task::spawn_blocking(move || {
                if let Err(e) = tray_handle.join() {
                    error!("System tray thread panic: {:?}", e);
                }
            }).await?;
            
            Ok(())
        }
        
        #[cfg(not(windows))]
        {
            info!("💡 System tray not supported on this platform");
            
            // Keep the task alive on non-Windows platforms
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            }
        }
    }
} 