use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Идентификатор пресета модели (см. models::PRESETS)
    pub model_preset: String,
    /// "q4km" | "q8"
    pub quant: String,
    /// Путь к собственному GGUF-файлу с диска (пусто — не задан)
    pub custom_model_path: String,
    /// Использовать собственный файл вместо пресета
    pub use_custom_model: bool,
    /// Окно контекста, токены (8192–16384)
    pub n_ctx: u32,
    /// Максимум токенов в итоговом ответе
    pub max_output_tokens: u32,
    /// "auto" (GPU если доступен) | "cpu"
    pub gpu_mode: String,
    /// 0 = автоматически
    pub threads: u32,
    /// Режим быстрого повторного запуска: держать модель в памяти после генерации
    pub fast_restart: bool,
    /// Сколько минут держать модель загруженной в режиме быстрого запуска
    pub keep_alive_minutes: u32,
    /// "short" | "medium" | "detailed"
    pub summary_length: String,
    /// "auto" (язык оригинала) или английское название языка
    pub summary_language: String,
    /// Основной язык перевода
    pub translate_target: String,
    /// Запасной язык: если текст уже на основном — переводим на него
    pub translate_secondary: String,
    pub hotkey_summarize: String,
    pub hotkey_translate: String,
    pub hotkey_open: String,
    pub autostart: bool,
    /// Скопировать результат в буфер обмена автоматически после горячей клавиши
    pub auto_copy_result: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model_preset: "qwen38".into(),
            quant: "q4km".into(),
            custom_model_path: String::new(),
            use_custom_model: false,
            n_ctx: 8192,
            max_output_tokens: 1024,
            gpu_mode: "auto".into(),
            threads: 0,
            fast_restart: false,
            keep_alive_minutes: 5,
            summary_length: "medium".into(),
            summary_language: "auto".into(),
            translate_target: "Russian".into(),
            translate_secondary: "English".into(),
            hotkey_summarize: "CommandOrControl+Alt+S".into(),
            hotkey_translate: "CommandOrControl+Alt+T".into(),
            hotkey_open: "CommandOrControl+Alt+Space".into(),
            autostart: false,
            auto_copy_result: false,
        }
    }
}

impl Settings {
    /// Приводит значения к допустимым диапазонам.
    pub fn sanitize(&mut self) {
        self.n_ctx = self.n_ctx.clamp(8192, 16384);
        self.max_output_tokens = self.max_output_tokens.clamp(128, 4096);
        self.keep_alive_minutes = self.keep_alive_minutes.clamp(1, 120);
        if self.quant != "q4km" && self.quant != "q8" {
            self.quant = "q4km".into();
        }
        if !matches!(self.gpu_mode.as_str(), "auto" | "cpu") {
            self.gpu_mode = "auto".into();
        }
        if !matches!(self.summary_length.as_str(), "short" | "medium" | "detailed") {
            self.summary_length = "medium".into();
        }
        if self.summary_language.trim().is_empty() {
            self.summary_language = "auto".into();
        }
        if self.translate_target.trim().is_empty() {
            self.translate_target = "Russian".into();
        }
        if self.translate_secondary.trim().is_empty() {
            self.translate_secondary = "English".into();
        }
    }
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    let mut s = settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}

pub fn save(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let path = settings_path(app).ok_or("не удалось определить папку настроек")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}
