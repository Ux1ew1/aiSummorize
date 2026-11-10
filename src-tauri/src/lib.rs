mod chunking;
mod hotkeys;
mod job;
mod llm;
mod models;
mod selection;
mod settings;
mod state;

use serde::Serialize;
use settings::Settings;
use state::AppState;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_dialog::DialogExt;

#[derive(Serialize)]
struct QuantDto {
    id: &'static str,
    label: &'static str,
    file: &'static str,
    size: u64,
    installed: bool,
}

#[derive(Serialize)]
struct PresetDto {
    id: &'static str,
    title: &'static str,
    note: &'static str,
    repo: &'static str,
    quants: Vec<QuantDto>,
}

#[derive(Serialize)]
struct StateDto {
    settings: Settings,
    presets: Vec<PresetDto>,
    model_ready: bool,
    /// Имя файла выбранной с диска модели (если задана)
    custom_model_name: String,
    custom_model_size: u64,
    backend: &'static str,
    models_dir: String,
    model_loaded: bool,
    job_running: bool,
    downloading: bool,
    version: &'static str,
}

#[tauri::command]
fn get_state(app: AppHandle, state: State<'_, AppState>) -> Result<StateDto, String> {
    let settings = state.settings();
    let presets = models::PRESETS
        .iter()
        .map(|p| PresetDto {
            id: p.id,
            title: p.title,
            note: p.note,
            repo: p.repo,
            quants: p
                .quants
                .iter()
                .map(|q| QuantDto {
                    id: q.id,
                    label: q.label,
                    file: q.file,
                    size: q.size,
                    installed: models::is_installed(&app, p.id, q.id),
                })
                .collect(),
        })
        .collect();
    let model_loaded = match state.engine.loaded.try_lock() {
        Ok(g) => g.is_some(),
        Err(_) => true,
    };
    let custom = std::path::Path::new(&settings.custom_model_path);
    let custom_model_name = if settings.custom_model_path.is_empty() || !custom.is_file() {
        String::new()
    } else {
        custom.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    };
    let custom_model_size = std::fs::metadata(custom).map(|m| m.len()).unwrap_or(0);
    Ok(StateDto {
        model_ready: models::model_ready(&app, &settings),
        custom_model_name,
        custom_model_size,
        settings,
        presets,
        backend: llm::compiled_backend(),
        models_dir: models::models_dir(&app)?.display().to_string(),
        model_loaded,
        job_running: state.job_running.load(Ordering::SeqCst),
        downloading: state.downloading.load(Ordering::SeqCst),
        version: env!("CARGO_PKG_VERSION"),
    })
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let al = app.autolaunch();
    let current = al.is_enabled().unwrap_or(false);
    if enabled && !current {
        let _ = al.enable();
    } else if !enabled && current {
        let _ = al.disable();
    }
}

/// Сохраняет настройки, перерегистрирует хоткеи. Возвращает список предупреждений.
#[tauri::command]
fn save_settings(app: AppHandle, state: State<'_, AppState>, new_settings: Settings) -> Result<Vec<String>, String> {
    let mut s = new_settings;
    s.sanitize();
    settings::save(&app, &s)?;
    {
        *state.settings.lock().unwrap_or_else(|e| e.into_inner()) = s.clone();
    }
    apply_autostart(&app, s.autostart);
    let problems = hotkeys::register_all(&app, &s);
    if !s.fast_restart {
        state.engine.try_unload(&app);
    }
    Ok(problems)
}

#[tauri::command]
fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    preset: String,
    quant: String,
) -> Result<(), String> {
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err("Загрузка уже идёт".into());
    }
    state.download_cancel.store(false, Ordering::SeqCst);

    // Выбранная модель становится активной
    {
        let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        s.model_preset = preset.clone();
        s.quant = quant.clone();
        s.use_custom_model = false;
        let _ = settings::save(&app, &s);
    }

    let cancel = state.download_cancel.clone();
    let flag = state.downloading.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = models::download(app2.clone(), preset.clone(), quant.clone(), cancel).await {
            models::report_error(&app2, &preset, &quant, e);
        }
        flag.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// Открывает системный диалог выбора .gguf, проверяет файл и делает его активной моделью.
/// Возвращает путь или None, если пользователь отменил выбор.
#[tauri::command]
async fn pick_custom_model(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Выберите файл модели (GGUF)")
        .add_filter("GGUF", &["gguf"])
        .blocking_pick_file();
    let Some(file) = picked else { return Ok(None) };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().into_owned();
    models::validate_custom_model(&path_str)?;

    state.engine.try_unload(&app); // прежняя модель больше не нужна
    let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    s.custom_model_path = path_str.clone();
    s.use_custom_model = true;
    settings::save(&app, &s)?;
    Ok(Some(path_str))
}

#[tauri::command]
fn cancel_download(state: State<'_, AppState>) {
    state.download_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn delete_model(app: AppHandle, state: State<'_, AppState>, preset: String, quant: String) -> Result<(), String> {
    state.engine.try_unload(&app);
    let path = models::model_path(&app, &preset, &quant)?;
    for p in [path.clone(), path.with_extension("gguf.part")] {
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| format!("Не удалось удалить {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn run_job(app: AppHandle, req: job::JobRequest) -> Result<u64, String> {
    job::spawn_job(&app, req)
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>) {
    state.job_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn unload_model(app: AppHandle, state: State<'_, AppState>) -> bool {
    state.engine.try_unload(&app)
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    selection::copy_to_clipboard(&text)
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
fn open_models_dir(app: AppHandle) -> Result<(), String> {
    let dir = models::models_dir(&app)?;
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let unload = MenuItem::with_id(app, "unload", "Выгрузить модель из памяти", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &unload, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("AI Summorise")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => hotkeys::show_main_window(app),
            "unload" => {
                let st = app.state::<AppState>();
                st.engine.try_unload(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                hotkeys::show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Фоновый таймер: выгружает простаивающую модель.
fn spawn_idle_watcher(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        let st = app.state::<AppState>();
        let s = st.settings();
        let idle = st.engine.idle_for();
        let limit = if s.fast_restart {
            Duration::from_secs(s.keep_alive_minutes as u64 * 60)
        } else {
            Duration::from_secs(3)
        };
        if idle >= limit {
            st.engine.try_unload(&app);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            hotkeys::show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_settings,
            start_download,
            pick_custom_model,
            cancel_download,
            delete_model,
            run_job,
            cancel_job,
            unload_model,
            copy_text,
            hide_window,
            open_models_dir,
        ])
        .on_window_event(|window, event| {
            // Закрытие окна = свернуть в трей, приложение продолжает работать в фоне
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let s = settings::load(&handle);
            app.manage(AppState::new(s.clone()));

            build_tray(&handle)?;
            let problems = hotkeys::register_all(&handle, &s);
            for p in problems {
                log::warn!("hotkey: {p}");
            }
            spawn_idle_watcher(handle.clone());

            // При автозапуске (--minimized) остаёмся в трее, если модель уже скачана
            let minimized = std::env::args().any(|a| a == "--minimized");
            let ready = models::model_ready(&handle, &s);
            if !(minimized && ready) {
                hotkeys::show_main_window(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
