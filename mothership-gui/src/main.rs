// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod explorer;
mod hub;
mod nvim;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(nvim::NvimSessions::default())
        .invoke_handler(tauri::generate_handler![
            explorer::explorer_list_dir,
            explorer::explorer_is_beamed,
            nvim::nvim_open,
            nvim::nvim_write,
            nvim::nvim_resize,
            nvim::nvim_close,
            nvim::nvim_check_file,
            nvim::nvim_goto,
            nvim::nvim_reload_files,
            nvim::terminal_open,
            nvim::terminal_close,
            hub::hub_auth_status,
            hub::hub_auth_start,
            hub::hub_auth_logout,
            hub::hub_health,
            hub::hub_ensure_daemon,
            hub::hub_list_projects,
            hub::hub_beam,
            hub::hub_active_rift,
            hub::hub_create_project,
            hub::hub_delete_project,
            hub::hub_list_rifts,
            hub::hub_create_rift,
            hub::hub_switch_rift,
            hub::hub_create_checkpoint,
            hub::hub_checkpoint_history,
            hub::hub_rift_events,
        hub::hub_diff,
        hub::hub_file_diff,
        hub::hub_secret_holds,
        hub::hub_allow_secret,
            hub::hub_restore_checkpoint,
            hub::hub_server_info,
            hub::hub_daemon_status,
            hub::hub_daemon_start,
            hub::hub_daemon_stop,
            hub::hub_daemon_restart,
            hub::hub_list_wormholes,
            hub::hub_create_wormhole,
            hub::hub_show_wormhole,
            hub::hub_resolve_conflict,
            hub::hub_converge_wormhole,
            hub::hub_dissolve_wormhole,
            hub::hub_delete_wormhole,
            hub::hub_toggle_wormhole_rift,
            hub::hub_beam_wormhole,
            hub::hub_wormhole_rift_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
