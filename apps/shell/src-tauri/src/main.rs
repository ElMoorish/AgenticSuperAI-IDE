#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use state::{locate_agent_exe, run_gpu_loop, run_pump, run_sys_loop, AppState};
use std::sync::Arc;
use tauri::{Manager, RunEvent};

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let exe = std::env::current_exe()
                .ok()
                .and_then(|e| locate_agent_exe(&e, std::env::var("ASA_AGENT_EXE").ok().as_deref()));
            if exe.is_none() {
                eprintln!("agent-core executable not found; the agent is unavailable");
            }
            let state = Arc::new(AppState::new(dir, exe));
            let log = state.logger();
            log.write("INFO", &format!("startup v{}", env!("CARGO_PKG_VERSION")));
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                log.write("PANIC", &info.to_string());
                prev(info);
            }));
            let rx = state.take_event_rx().expect("event receiver taken once");
            tauri::async_runtime::spawn(run_pump(rx, state.sink_slot()));
            tauri::async_runtime::spawn(run_gpu_loop(state.clone()));
            tauri::async_runtime::spawn(run_sys_loop(state.clone()));
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pick_folder,
            commands::open_folder,
            commands::list_dir,
            commands::search_files,
            commands::read_file,
            commands::save_file,
            commands::subscribe_events,
            commands::agent_prompt,
            commands::agent_cancel,
            commands::resolve_approval,
            commands::confirm_edit,
            commands::probe_endpoint,
            commands::detect_servers,
            commands::find_local_models,
            commands::find_engines,
            commands::find_airllm,
            commands::model_info,
            commands::launch_model,
            commands::stop_model,
            commands::model_server_status,
            commands::get_settings,
            commands::set_settings,
            commands::start_user_terminal,
            commands::terminal_write,
            commands::terminal_resize,
            commands::close_terminal,
            commands::copy_diagnostics,
            commands::recovery_save,
            commands::recovery_list,
            commands::recovery_clear,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<Arc<AppState>>() {
                tauri::async_runtime::block_on(state.shutdown());
            }
        }
    });
}
