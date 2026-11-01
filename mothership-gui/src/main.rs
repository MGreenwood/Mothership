// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hub;
mod nvim;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(nvim::NvimSessions::default())
        .invoke_handler(tauri::generate_handler![
            nvim::nvim_open,
            nvim::nvim_write,
            nvim::nvim_resize,
            nvim::nvim_unsaved_buffers,
            nvim::nvim_close,
            nvim::nvim_check_file,
            hub::hub_auth_status,
            hub::hub_auth_start,
            hub::hub_auth_complete,
            hub::hub_auth_logout,
            hub::hub_health,
            hub::hub_list_projects,
            hub::hub_create_project,
            hub::hub_delete_project,
            hub::hub_list_rifts,
            hub::hub_create_rift,
            hub::hub_switch_rift,
            hub::hub_create_checkpoint,
            hub::hub_checkpoint_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
