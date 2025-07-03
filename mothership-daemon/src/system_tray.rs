use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use std::collections::HashMap;
use uuid::Uuid;
use std::ffi::CString;
use std::ptr;

use crate::daemon::{DaemonStatus, TrackedProject};

#[cfg(windows)]
use {
    tray_icon::{
        menu::{Menu, MenuEvent, MenuItemBuilder, Submenu},
        TrayIconBuilder, TrayIconEvent, Icon,
    },
    winit::{
        event_loop::{ControlFlow, EventLoopBuilder},
        platform::windows::EventLoopBuilderExtWindows,
    },
    image,
    std::process::Command,
};

/// System tray integration for Windows
pub struct SystemTray {
    status: Arc<RwLock<DaemonStatus>>,
    tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
}

#[cfg(windows)]
fn load_tray_icon() -> Option<Icon> {
    // Embed the icon directly into the binary at compile time
    // This ensures the icon is always available regardless of where the binary is located
    const ICON_DATA: &[u8] = include_bytes!("../../mothership-gui/icons/icon.png");
    
    match image::load_from_memory(ICON_DATA) {
        Ok(img) => {
            // Convert to RGBA8 format expected by tray-icon
            let rgba_img = img.to_rgba8();
            let (width, height) = rgba_img.dimensions();
            let rgba_data = rgba_img.into_raw();
            
            match Icon::from_rgba(rgba_data, width, height) {
                Ok(icon) => {
                    info!("✅ Loaded embedded tray icon ({}x{} pixels)", width, height);
                    return Some(icon);
                }
                Err(e) => {
                    error!("⚠️ Failed to create icon from embedded data: {}", e);
                }
            }
        }
        Err(e) => {
            error!("⚠️ Failed to load embedded icon data: {}", e);
        }
    }
    
    info!("📋 Using default system tray icon (embedded icon failed to load)");
    None
}

#[cfg(windows)]
fn open_folder(path: &std::path::Path) -> Result<()> {
    Command::new("explorer")
        .arg(path)
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to open folder: {}", e))?;
    Ok(())
}

#[cfg(windows)]
fn get_dynamic_tooltip(status: &DaemonStatus) -> String {
    let connection_status = if status.server_connected { "🟢" } else { "🔴" };
    let sync_status = if status.files_syncing > 0 { 
        format!("⏳ {} files syncing", status.files_syncing) 
    } else { 
        "✅ All synced".to_string() 
    };
    
    format!(
        "Mothership Daemon\n{} Server: {}\n📁 Projects: {}\n{}",
        connection_status,
        if status.server_connected { "Connected" } else { "Disconnected" },
        status.projects_tracked,
        sync_status
    )
}

impl SystemTray {
    /// Create a new system tray instance
    pub fn new(
        status: Arc<RwLock<DaemonStatus>>, 
        tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>
    ) -> Result<Self> {
        info!("🖥️ Initializing enhanced system tray...");
        Ok(Self { status, tracked_projects })
    }

    /// Run the system tray (Windows only)
    pub async fn run(self) -> Result<()> {
        #[cfg(windows)]
        {
            info!("🖥️ Starting enhanced Mothership system tray icon...");
            
            // Clone for thread-safe access
            let status = self.status.clone();
            let tracked_projects = self.tracked_projects.clone();
            
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

                // Create initial tray icon with dynamic tooltip
                let initial_tooltip = {
                    let initial_status = status.blocking_read();
                    get_dynamic_tooltip(&initial_status)
                };
                
                let mut tray_builder = TrayIconBuilder::new()
                    .with_tooltip(&initial_tooltip);
                
                // Try to use custom icon
                if let Some(custom_icon) = load_tray_icon() {
                    tray_builder = tray_builder.with_icon(custom_icon);
                }
                
                let tray_icon = match tray_builder.build() {
                    Ok(icon) => icon,
                    Err(e) => {
                        error!("Failed to create tray icon: {}", e);
                        return;
                    }
                };

                info!("✅ Enhanced system tray icon created successfully");

                // Track last menu update time to avoid rebuilding too frequently
                let mut last_menu_update = std::time::Instant::now();
                let mut last_tooltip_update = std::time::Instant::now();
                
                // Run the event loop
                let result = event_loop.run(move |_event, elwt| {
                    elwt.set_control_flow(ControlFlow::Wait);

                    // Update tooltip every 5 seconds
                    if last_tooltip_update.elapsed() > std::time::Duration::from_secs(5) {
                        let current_status = status.blocking_read();
                        let new_tooltip = get_dynamic_tooltip(&current_status);
                        let _ = tray_icon.set_tooltip(Some(&new_tooltip));
                        last_tooltip_update = std::time::Instant::now();
                    }

                    // Handle menu events
                    while let Ok(event) = MenuEvent::receiver().try_recv() {
                        match event.id.as_ref() {
                            "status" => {
                                info!("📊 Status menu item clicked");
                                let status = status.blocking_read();
                                let _projects = tracked_projects.blocking_read();
                                
                                let status_msg = format!(
                                    "Mothership Daemon Status\n\n\
                                    Running: {}\n\
                                    Server Connected: {}\n\
                                    Projects Tracked: {}\n\
                                    Files Syncing: {}\n\
                                    Last Sync: {}",
                                    if status.is_running { "✅ Yes" } else { "❌ No" },
                                    if status.server_connected { "🟢 Connected" } else { "🔴 Disconnected" },
                                    status.projects_tracked,
                                    status.files_syncing,
                                    status.last_sync
                                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                        .unwrap_or_else(|| "Never".to_string())
                                );
                                
                                // Show status in a message box
                                let title = CString::new("Mothership Status").unwrap();
                                let message = CString::new(status_msg).unwrap();
                                
                                unsafe {
                                    winapi::um::winuser::MessageBoxA(
                                        ptr::null_mut(),
                                        message.as_ptr(),
                                        title.as_ptr(),
                                        winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONINFORMATION,
                                    );
                                }
                            }
                            "projects" => {
                                info!("📁 Projects menu item clicked");
                                let projects = tracked_projects.blocking_read();
                                
                                if projects.is_empty() {
                                    let title = CString::new("Tracked Projects").unwrap();
                                    let message = CString::new("No projects are currently being tracked.\n\nUse 'mothership beam <project>' to start tracking a project.").unwrap();
                                    
                                    unsafe {
                                        winapi::um::winuser::MessageBoxA(
                                            ptr::null_mut(),
                                            message.as_ptr(),
                                            title.as_ptr(),
                                            winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONINFORMATION,
                                        );
                                    }
                                } else {
                                    let projects_list = projects.values()
                                        .map(|p| format!("📁 {} ({})", p.project_name, p.project_path.display()))
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    
                                    let message = format!("Tracked Projects ({}):\n\n{}", projects.len(), projects_list);
                                    let title = CString::new("Tracked Projects").unwrap();
                                    let msg_cstring = CString::new(message).unwrap();
                                    
                                    unsafe {
                                        winapi::um::winuser::MessageBoxA(
                                            ptr::null_mut(),
                                            msg_cstring.as_ptr(),
                                            title.as_ptr(),
                                            winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONINFORMATION,
                                        );
                                    }
                                }
                            }
                            id if id.starts_with("open_project_") => {
                                let project_id_str = &id["open_project_".len()..];
                                if let Ok(project_id) = uuid::Uuid::parse_str(project_id_str) {
                                    let projects = tracked_projects.blocking_read();
                                    if let Some(project) = projects.get(&project_id) {
                                        info!("📂 Opening project folder: {}", project.project_path.display());
                                        if let Err(e) = open_folder(&project.project_path) {
                                            error!("Failed to open project folder: {}", e);
                                        }
                                    }
                                }
                            }
                            "open_logs" => {
                                info!("📜 Opening logs folder");
                                // Try to open the logs directory
                                let log_paths = vec![
                                    std::env::temp_dir().join("mothership"),
                                    std::env::current_dir().unwrap_or_default().join("logs"),
                                    std::path::PathBuf::from("C:\\ProgramData\\Mothership\\logs"),
                                ];
                                
                                let mut opened = false;
                                for log_path in log_paths {
                                    if log_path.exists() {
                                        if let Err(e) = open_folder(&log_path) {
                                            warn!("Failed to open log folder {}: {}", log_path.display(), e);
                                        } else {
                                            opened = true;
                                            break;
                                        }
                                    }
                                }
                                
                                if !opened {
                                    let title = CString::new("Logs").unwrap();
                                    let message = CString::new("Could not locate log files. Check the console output for daemon logs.").unwrap();
                                    
                                    unsafe {
                                        winapi::um::winuser::MessageBoxA(
                                            ptr::null_mut(),
                                            message.as_ptr(),
                                            title.as_ptr(),
                                            winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONWARNING,
                                        );
                                    }
                                }
                            }
                            "force_sync" => {
                                info!("🔄 Force sync requested from system tray");
                                // TODO: Implement force sync functionality
                                let title = CString::new("Force Sync").unwrap();
                                let message = CString::new("Force sync feature coming soon!\n\nFor now, file changes are automatically detected and synced.").unwrap();
                                
                                unsafe {
                                    winapi::um::winuser::MessageBoxA(
                                        ptr::null_mut(),
                                        message.as_ptr(),
                                        title.as_ptr(),
                                        winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONINFORMATION,
                                    );
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

                    // Handle tray icon events (clicks)
                    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                        match event {
                            TrayIconEvent::Click { .. } => {
                                info!("🖱️ Tray icon clicked - rebuilding context menu");
                                
                                // Rebuild menu with current project list
                                if last_menu_update.elapsed() > std::time::Duration::from_millis(500) {
                                    let tray_menu = Self::build_context_menu(&status, &tracked_projects);
                                    let _ = tray_icon.set_menu(Some(Box::new(tray_menu)));
                                    last_menu_update = std::time::Instant::now();
                                }
                            }
                            // Note: DoubleClick variant doesn't exist in this version of tray-icon
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
            info!("💡 Enhanced system tray not supported on this platform");
            
            // Keep the task alive on non-Windows platforms
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            }
        }
    }
    
    #[cfg(windows)]
    fn build_context_menu(
        _status: &Arc<RwLock<DaemonStatus>>, 
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>
    ) -> Menu {
        let tray_menu = Menu::new();
        
        // Status section
        let status_item = MenuItemBuilder::new()
            .text("📊 Show Status")
            .id("status".into())
            .build();
        
        let projects_item = MenuItemBuilder::new()
            .text("📁 Show Projects")
            .id("projects".into())
            .build();
        
        let _ = tray_menu.append(&status_item);
        let _ = tray_menu.append(&projects_item);
        
        // Projects submenu (if any projects exist)
        let projects = tracked_projects.blocking_read();
        if !projects.is_empty() {
            let projects_submenu = Submenu::new("📂 Open Project Folders", true);
            
            for project in projects.values() {
                let project_item = MenuItemBuilder::new()
                    .text(&format!("📁 {}", project.project_name))
                    .id(format!("open_project_{}", project.project_id).into())
                    .build();
                let _ = projects_submenu.append(&project_item);
            }
            
            let _ = tray_menu.append(&projects_submenu);
        }
        
        // Separator
        let _ = tray_menu.append(&tray_icon::menu::MenuItemBuilder::new().text("─────────────").enabled(false).build());
        
        // Actions section
        let force_sync_item = MenuItemBuilder::new()
            .text("🔄 Force Sync All")
            .id("force_sync".into())
            .build();
        
        let logs_item = MenuItemBuilder::new()
            .text("📜 Open Logs")
            .id("open_logs".into())
            .build();
        
        let _ = tray_menu.append(&force_sync_item);
        let _ = tray_menu.append(&logs_item);
        
        // Separator
        let _ = tray_menu.append(&tray_icon::menu::MenuItemBuilder::new().text("─────────────").enabled(false).build());
        
        // Control section
        let stop_item = MenuItemBuilder::new()
            .text("⏹️ Stop Daemon")
            .id("stop".into())
            .build();
        
        let restart_item = MenuItemBuilder::new()
            .text("🔄 Restart Daemon")
            .id("restart".into())
            .build();
        
        let exit_item = MenuItemBuilder::new()
            .text("❌ Exit")
            .id("exit".into())
            .build();
        
        let _ = tray_menu.append(&stop_item);
        let _ = tray_menu.append(&restart_item);
        let _ = tray_menu.append(&exit_item);
        
        tray_menu
    }
} 