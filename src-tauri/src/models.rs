//! Каталог моделей и загрузка GGUF с HuggingFace.

use crate::settings::Settings;
use futures_util::StreamExt;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

pub struct QuantInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub size: u64,
}

pub struct Preset {
    pub id: &'static str,
    pub title: &'static str,
    pub note: &'static str,
    pub repo: &'static str,
    pub quants: [QuantInfo; 2],
}

pub const PRESETS: [Preset; 2] = [
    Preset {
        id: "qwen38",
        title: "Qwen3.8 4B (community distill, empero-ai)",
        note: "Неофициальная дистилляция на архитектуре Qwen3.5-4B. Apache-2.0.",
        repo: "empero-ai/Qwen3.8-4B-Distill-GGUF",
        quants: [
            QuantInfo {
                id: "q4km",
                label: "Q4_K_M",
                file: "Qwen3.8-4B-Q4_K_M.gguf",
                size: 2_783_446_304,
            },
            QuantInfo {
                id: "q8",
                label: "Q8_0",
                file: "Qwen3.8-4B-Q8_0.gguf",
                size: 4_610_579_744,
            },
        ],
    },
    Preset {
        id: "qwen35",
        title: "Qwen3.5 4B (unsloth)",
        note: "Официальные веса Qwen3.5-4B, кванты от Unsloth. Apache-2.0.",
        repo: "unsloth/Qwen3.5-4B-GGUF",
        quants: [
            QuantInfo {
                id: "q4km",
                label: "Q4_K_M",
                file: "Qwen3.5-4B-Q4_K_M.gguf",
                size: 0,
            },
            QuantInfo {
                id: "q8",
                label: "Q8_0",
                file: "Qwen3.5-4B-Q8_0.gguf",
                size: 0,
            },
        ],
    },
];

pub fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

pub fn quant<'a>(p: &'a Preset, id: &str) -> &'a QuantInfo {
    p.quants.iter().find(|q| q.id == id).unwrap_or(&p.quants[0])
}

pub fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn model_path(app: &AppHandle, preset_id: &str, quant_id: &str) -> Result<PathBuf, String> {
    let p = preset(preset_id);
    Ok(models_dir(app)?.join(quant(p, quant_id).file))
}

/// Файл считается установленным, если он есть и (при известном размере) совпадает по размеру.
pub fn is_installed(app: &AppHandle, preset_id: &str, quant_id: &str) -> bool {
    let p = preset(preset_id);
    let q = quant(p, quant_id);
    match model_path(app, preset_id, quant_id).and_then(|path| std::fs::metadata(path).map_err(|e| e.to_string())) {
        Ok(m) => m.is_file() && m.len() > 100_000_000 && (q.size == 0 || m.len() == q.size),
        Err(_) => false,
    }
}

/// Проверяет, что файл существует и является GGUF (по сигнатуре в начале файла).
pub fn validate_custom_model(path: &str) -> Result<(), String> {
    use std::io::Read;
    let p = std::path::Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| format!("Файл недоступен: {e}"))?;
    if !meta.is_file() {
        return Err("Это не файл".into());
    }
    if meta.len() < 10_000_000 {
        return Err("Файл слишком маленький для модели".into());
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| format!("Не удалось прочитать файл: {e}"))?;
    if &magic != b"GGUF" {
        return Err("Файл не в формате GGUF".into());
    }
    Ok(())
}

pub fn custom_model_ready(s: &Settings) -> bool {
    s.use_custom_model && !s.custom_model_path.is_empty() && validate_custom_model(&s.custom_model_path).is_ok()
}

/// Модель, которая будет использована для задач: свой файл или скачанный пресет.
pub fn model_ready(app: &AppHandle, s: &Settings) -> bool {
    if s.use_custom_model {
        custom_model_ready(s)
    } else {
        is_installed(app, &s.model_preset, &s.quant)
    }
}

pub fn active_model_path(app: &AppHandle, s: &Settings) -> Option<PathBuf> {
    if s.use_custom_model {
        return custom_model_ready(s).then(|| PathBuf::from(&s.custom_model_path));
    }
    if is_installed(app, &s.model_preset, &s.quant) {
        model_path(app, &s.model_preset, &s.quant).ok()
    } else {
        None
    }
}

#[derive(Serialize, Clone)]
struct DownloadEvent {
    kind: &'static str, // progress | done | error | cancelled
    preset: String,
    quant: String,
    downloaded: u64,
    total: u64,
    speed_bps: f64,
    message: String,
}

fn emit(app: &AppHandle, ev: DownloadEvent) {
    let _ = app.emit("download-event", ev);
}

/// Скачивает файл модели с докачкой. Блокирует до завершения (вызывать из async-задачи).
pub async fn download(
    app: AppHandle,
    preset_id: String,
    quant_id: String,
    cancel: std::sync::Arc<AtomicBool>,
) -> Result<(), String> {
    let p = preset(&preset_id);
    let q = quant(p, &quant_id);
    let final_path = model_path(&app, &preset_id, &quant_id)?;
    let part_path = final_path.with_extension("gguf.part");
    let url = format!("https://huggingface.co/{}/resolve/main/{}", p.repo, q.file);

    let mk = |kind, downloaded, total, speed_bps, message: String| DownloadEvent {
        kind,
        preset: preset_id.clone(),
        quant: quant_id.clone(),
        downloaded,
        total,
        speed_bps,
        message,
    };

    let client = reqwest::Client::builder()
        .user_agent("ai-summorise/0.1")
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let mut existing = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(&url);
    if existing > 0 {
        req = req.header("Range", format!("bytes={existing}-"));
    }
    let resp = req.send().await.map_err(|e| format!("Ошибка сети: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 416 {
        // Файл уже докачан целиком
        existing = 0;
        let _ = existing;
    } else if !status.is_success() {
        return Err(format!("HuggingFace вернул {status} для {url}"));
    }

    let resumed = status.as_u16() == 206;
    if !resumed {
        existing = 0;
    }
    let content_len = resp.content_length().unwrap_or(0);
    let total = if resumed { existing + content_len } else { content_len };
    let total = if total == 0 { q.size } else { total };

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part_path)
        .await
        .map_err(|e| format!("Не удалось открыть файл: {e}"))?;

    let mut downloaded = existing;
    let mut stream = resp.bytes_stream();
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let session_start_bytes = downloaded;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            let _ = file.flush().await;
            emit(&app, mk("cancelled", downloaded, total, 0.0, "Загрузка остановлена (можно продолжить)".into()));
            return Ok(());
        }
        let chunk = chunk.map_err(|e| format!("Обрыв соединения: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| format!("Ошибка записи: {e}"))?;
        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(250) {
            last_emit = Instant::now();
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let speed = (downloaded - session_start_bytes) as f64 / secs;
            emit(&app, mk("progress", downloaded, total, speed, String::new()));
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    if total > 0 && downloaded != total {
        return Err(format!("Файл скачан не полностью: {downloaded} из {total} байт. Повторите загрузку."));
    }
    if final_path.exists() {
        let _ = tokio::fs::remove_file(&final_path).await;
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| format!("Не удалось сохранить модель: {e}"))?;
    emit(&app, mk("done", downloaded, total, 0.0, String::new()));
    Ok(())
}

pub fn report_error(app: &AppHandle, preset_id: &str, quant_id: &str, message: String) {
    emit(
        app,
        DownloadEvent {
            kind: "error",
            preset: preset_id.into(),
            quant: quant_id.into(),
            downloaded: 0,
            total: 0,
            speed_bps: 0.0,
            message,
        },
    );
}
