//! Захват выделенного текста из любого приложения: эмулируем Ctrl/Cmd+C, читаем буфер, возвращаем прежний.

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
const KEY_C: Key = Key::Other(8); // kVK_ANSI_C — не зависит от раскладки
#[cfg(target_os = "windows")]
const KEY_C: Key = Key::Other(0x43); // VK_C — работает и в русской раскладке
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const KEY_C: Key = Key::Unicode('c');

#[cfg(target_os = "macos")]
const COPY_MOD: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const COPY_MOD: Key = Key::Control;

/// Возвращает выделенный текст или None, если ничего не выделено / копирование не удалось.
pub fn capture_selection() -> Result<Option<String>, String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("Буфер обмена недоступен: {e}"))?;
    let previous = clipboard.get_text().ok();

    // Маркер, чтобы отличить «ничего не скопировалось» от прежнего содержимого буфера.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let marker = format!("\u{200B}summorise-{nanos}");
    let _ = clipboard.set_text(marker.clone());

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| {
        format!("Не удалось эмулировать ввод (на macOS разрешите «Универсальный доступ» для приложения): {e}")
    })?;

    // Пользователь ещё может держать модификаторы горячей клавиши — отпускаем их.
    for k in [Key::Alt, Key::Shift, Key::Control, Key::Meta] {
        let _ = enigo.key(k, Direction::Release);
    }
    sleep(Duration::from_millis(60));
    enigo
        .key(COPY_MOD, Direction::Press)
        .map_err(|e| format!("Ошибка эмуляции клавиш: {e}"))?;
    let click = enigo.key(KEY_C, Direction::Click);
    let _ = enigo.key(COPY_MOD, Direction::Release);
    click.map_err(|e| format!("Ошибка эмуляции клавиш: {e}"))?;

    // Ждём, пока приложение положит текст в буфер.
    let deadline = Instant::now() + Duration::from_millis(900);
    let mut captured: Option<String> = None;
    while Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        if let Ok(t) = clipboard.get_text() {
            if t != marker {
                captured = Some(t);
                break;
            }
        }
    }

    // Возвращаем прежний текст (картинки и файлы в буфере восстановить нельзя).
    match previous {
        Some(p) => {
            let _ = clipboard.set_text(p);
        }
        None => {
            let _ = clipboard.clear();
        }
    }

    Ok(captured.filter(|t| !t.trim().is_empty()))
}

pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_string()))
        .map_err(|e| e.to_string())
}
