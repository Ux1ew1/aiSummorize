use crate::selection;
use crate::settings::Settings;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        // Обход ограничения Windows на перехват фокуса фоновым процессом
        let _ = w.set_always_on_top(true);
        let _ = w.set_focus();
        let _ = w.set_always_on_top(false);
    }
}

/// Регистрирует все горячие клавиши. Возвращает список проблем (занято / неверный формат).
pub fn register_all(app: &AppHandle, s: &Settings) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut problems = vec![];
    let entries = [
        (&s.hotkey_summarize, "summarize", "Сводка"),
        (&s.hotkey_translate, "translate", "Перевод"),
        (&s.hotkey_open, "open", "Открыть окно"),
    ];
    for (spec, action, title) in entries {
        let spec = spec.trim();
        if spec.is_empty() {
            continue;
        }
        let action = action.to_string();
        let res = gs.on_shortcut(spec, move |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                trigger(app.clone(), action.clone());
            }
        });
        if let Err(e) = res {
            problems.push(format!("{title} ({spec}): {e}"));
        }
    }
    problems
}

/// Обработка нажатия: захват выделения (пока фокус в исходном приложении) → показ окна → событие в UI.
pub fn trigger(app: AppHandle, action: String) {
    std::thread::spawn(move || {
        if action == "open" {
            show_main_window(&app);
            let _ = app.emit("hotkey-action", json!({"mode":"open","text":""}));
            return;
        }
        let (text, error) = match selection::capture_selection() {
            Ok(t) => (t.unwrap_or_default(), None),
            Err(e) => (String::new(), Some(e)),
        };
        show_main_window(&app);
        let _ = app.emit(
            "hotkey-action",
            json!({"mode":action,"text":text,"error":error}),
        );
    });
}
