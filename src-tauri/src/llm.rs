//! Обёртка над llama.cpp: загрузка модели по запросу, генерация, выгрузка.

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{list_llama_ggml_backend_devices, LlamaBackendDeviceType};
use serde::Serialize;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Загруженная модель. Порядок полей важен: модель должна освободиться раньше backend.
pub struct Loaded {
    pub model: LlamaModel,
    pub backend: LlamaBackend,
    pub path: PathBuf,
    pub gpu_mode: String,
    pub gpu_layers: u32,
    pub device: String,
    pub load_ms: u64,
}

pub struct Engine {
    pub loaded: Mutex<Option<Loaded>>,
    pub last_used: Mutex<Instant>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            loaded: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Option<Loaded>> {
        self.loaded.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    pub fn idle_for(&self) -> Duration {
        self.last_used.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }

    /// Выгружает модель, если она сейчас не используется. true — модель выгружена или её не было.
    pub fn try_unload(&self, app: &AppHandle) -> bool {
        match self.loaded.try_lock() {
            Ok(mut g) => {
                if g.take().is_some() {
                    emit_model_state(app, None);
                }
                true
            }
            Err(_) => false,
        }
    }
}

#[derive(Serialize, Clone)]
struct ModelState {
    loaded: bool,
    device: String,
    gpu_layers: u32,
    load_ms: u64,
}

pub fn emit_model_state(app: &AppHandle, l: Option<&Loaded>) {
    let st = match l {
        Some(l) => ModelState {
            loaded: true,
            device: l.device.clone(),
            gpu_layers: l.gpu_layers,
            load_ms: l.load_ms,
        },
        None => ModelState {
            loaded: false,
            device: String::new(),
            gpu_layers: 0,
            load_ms: 0,
        },
    };
    let _ = app.emit("model-state", st);
}

/// Название вычислительного бэкенда, с которым собрано приложение.
pub fn compiled_backend() -> &'static str {
    if cfg!(feature = "cuda") {
        "CUDA"
    } else if cfg!(target_os = "macos") {
        "Metal"
    } else {
        "CPU"
    }
}

fn gpu_device_name() -> Option<String> {
    list_llama_ggml_backend_devices()
        .into_iter()
        .find(|d| matches!(d.device_type, LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu))
        .map(|d| format!("{} · {}", d.description, d.backend))
}

fn try_load(path: &Path, gpu_layers: u32) -> Result<(LlamaModel, LlamaBackend), String> {
    let mut backend = LlamaBackend::init().map_err(|e| format!("Не удалось инициализировать llama.cpp: {e}"))?;
    backend.void_logs();
    let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    let model = LlamaModel::load_from_file(&backend, path, &params)
        .map_err(|e| format!("Не удалось загрузить модель: {e}"))?;
    Ok((model, backend))
}

/// Загружает модель в память; при ошибке на GPU (например, нехватка VRAM) пробует CPU.
pub fn load_model(path: &Path, gpu_mode: &str) -> Result<Loaded, String> {
    let started = Instant::now();
    let want_gpu = gpu_mode != "cpu" && compiled_backend() != "CPU";
    let mut gpu_layers: u32 = if want_gpu { 999 } else { 0 };

    let (model, backend) = match try_load(path, gpu_layers) {
        Ok(v) => v,
        Err(e) if gpu_layers > 0 => {
            log::warn!("GPU load failed ({e}), falling back to CPU");
            gpu_layers = 0;
            try_load(path, 0)?
        }
        Err(e) => return Err(e),
    };

    let device = if gpu_layers > 0 {
        gpu_device_name().unwrap_or_else(|| compiled_backend().to_string())
    } else {
        "CPU".to_string()
    };

    Ok(Loaded {
        model,
        backend,
        path: path.to_path_buf(),
        gpu_mode: gpu_mode.to_string(),
        gpu_layers,
        device,
        load_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Default, Clone, Serialize, Debug)]
pub struct Stats {
    pub prompt_tokens: u64,
    pub prompt_ms: f64,
    pub gen_tokens: u64,
    pub gen_ms: f64,
}

impl Stats {
    pub fn prompt_tps(&self) -> f64 {
        if self.prompt_ms > 0.0 {
            self.prompt_tokens as f64 / (self.prompt_ms / 1000.0)
        } else {
            0.0
        }
    }
    pub fn gen_tps(&self) -> f64 {
        if self.gen_ms > 0.0 {
            self.gen_tokens as f64 / (self.gen_ms / 1000.0)
        } else {
            0.0
        }
    }
}

pub enum GenError {
    Cancelled,
    Failed(String),
}

pub struct GenParams<'a> {
    pub n_ctx: u32,
    pub threads: i32,
    pub max_new: u32,
    pub temp: f32,
    pub repeat_penalty: f32,
    pub cancel: &'a AtomicBool,
}

/// Формирует ChatML-промпт Qwen с отключённым «размышлением» (пустой блок think).
pub fn build_prompt(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

/// Убирает блоки <think>…</think>. Пока ответ стримится, не отдаёт возможное начало тега.
pub fn visible_text(raw: &str, finished: bool) -> String {
    let mut raw = raw;
    if let Some(j) = raw.find("</think>") {
        if raw.find("<think>").map_or(true, |i| i > j) {
            raw = &raw[j + "</think>".len()..];
        }
    }
    let mut out = String::new();
    let mut rest = raw;
    loop {
        match rest.find("<think>") {
            None => {
                out.push_str(rest);
                break;
            }
            Some(i) => {
                out.push_str(&rest[..i]);
                let after = &rest[i + "<think>".len()..];
                match after.find("</think>") {
                    Some(j) => rest = &after[j + "</think>".len()..],
                    None => break, // блок ещё не закрыт — скрываем
                }
            }
        }
    }
    if !finished {
        for k in (1..="<think>".len() - 1).rev() {
            if out.ends_with(&"<think>"[..k]) {
                out.truncate(out.len() - k);
                break;
            }
        }
    }
    out.trim_start().to_string()
}

impl Loaded {
    pub fn tokenize(&self, text: &str, parse_special: bool) -> Vec<LlamaToken> {
        self.model.vocab().tokenize(text.as_bytes(), false, parse_special)
    }

    pub fn count_tokens(&self, text: &str) -> usize {
        self.tokenize(text, false).len()
    }

    pub fn n_ctx_train(&self) -> u32 {
        self.model.n_ctx_train()
    }

    /// Генерирует ответ. `on_text` получает новые видимые фрагменты, `on_tick` — текущую статистику.
    pub fn generate(
        &self,
        system: &str,
        user: &str,
        p: &GenParams,
        stats: &mut Stats,
        on_text: &mut dyn FnMut(&str),
        on_tick: &mut dyn FnMut(&Stats),
    ) -> Result<String, GenError> {
        let fail = |m: String| GenError::Failed(m);
        let prompt = build_prompt(system, user);
        let tokens = self.tokenize(&prompt, true);
        let n_prompt = tokens.len() as u32;
        let n_ctx = p.n_ctx.min(self.n_ctx_train().max(512));
        if n_prompt + 8 >= n_ctx {
            return Err(fail(format!(
                "Запрос ({n_prompt} токенов) не помещается в окно контекста {n_ctx}"
            )));
        }
        let max_new = p.max_new.min(n_ctx - n_prompt - 1);

        let n_batch = n_ctx.min(2048);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_batch)
            .with_n_ubatch(n_batch.min(512))
            .with_n_threads(p.threads)
            .with_n_threads_batch(p.threads);
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| fail(format!("Не удалось создать контекст (возможно, не хватает памяти): {e}")))?;

        let vocab = self.model.vocab();
        let mut batch = LlamaBatch::new(n_batch as usize, 1);

        // --- обработка промпта ---
        let t_prompt = Instant::now();
        let mut pos: i32 = 0;
        for chunk in tokens.chunks(n_batch as usize) {
            if p.cancel.load(Ordering::Relaxed) {
                return Err(GenError::Cancelled);
            }
            batch.clear();
            for (i, t) in chunk.iter().enumerate() {
                let last = pos as usize + i == tokens.len() - 1;
                batch
                    .add(*t, pos + i as i32, &[0], last)
                    .map_err(|e| fail(format!("batch: {e}")))?;
            }
            ctx.decode(&mut batch)
                .map_err(|e| fail(format!("Ошибка обработки промпта: {e}")))?;
            pos += chunk.len() as i32;
        }
        stats.prompt_tokens += n_prompt as u64;
        stats.prompt_ms += t_prompt.elapsed().as_secs_f64() * 1000.0;
        on_tick(stats);

        // --- генерация ---
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(1234);
        let mut samplers = vec![];
        if p.repeat_penalty > 1.0 {
            samplers.push(LlamaSampler::penalties(self.model.n_vocab(), 256, p.repeat_penalty, 0.0, 0.0));
        }
        if p.temp <= 0.01 {
            samplers.push(LlamaSampler::greedy());
        } else {
            samplers.push(LlamaSampler::top_k(20));
            samplers.push(LlamaSampler::top_p(0.9, 1));
            samplers.push(LlamaSampler::temp(p.temp));
            samplers.push(LlamaSampler::dist(seed));
        }
        let mut sampler = LlamaSampler::chain_simple(samplers);

        let mut raw = String::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut emitted = 0usize;
        let mut n_gen = 0u32;
        let t_gen = Instant::now();
        let base_ms = stats.gen_ms;
        let base_tokens = stats.gen_tokens;

        while n_gen < max_new {
            if p.cancel.load(Ordering::Relaxed) {
                stats.gen_ms = base_ms + t_gen.elapsed().as_secs_f64() * 1000.0;
                return Err(GenError::Cancelled);
            }
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                break;
            }
            n_gen += 1;

            pending.extend_from_slice(&vocab.token_to_piece(token, false, None));
            match std::str::from_utf8(&pending) {
                Ok(s) => {
                    raw.push_str(s);
                    pending.clear();
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        raw.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or(""));
                        pending.drain(..valid);
                    }
                    if pending.len() > 8 {
                        pending.clear(); // мусорные байты — пропускаем
                    }
                }
            }
            let vis = visible_text(&raw, false);
            if let Some(delta) = vis.get(emitted..) {
                if !delta.is_empty() {
                    on_text(delta);
                    emitted = vis.len();
                }
            }

            stats.gen_tokens = base_tokens + n_gen as u64;
            stats.gen_ms = base_ms + t_gen.elapsed().as_secs_f64() * 1000.0;
            on_tick(stats);

            batch.clear();
            batch
                .add(token, pos, &[0], true)
                .map_err(|e| fail(format!("batch: {e}")))?;
            pos += 1;
            ctx.decode(&mut batch)
                .map_err(|e| fail(format!("Ошибка генерации: {e}")))?;
        }

        stats.gen_ms = base_ms + t_gen.elapsed().as_secs_f64() * 1000.0;
        let final_text = visible_text(&raw, true);
        if let Some(delta) = final_text.get(emitted..) {
            if !delta.is_empty() {
                on_text(delta);
            }
        }
        on_tick(stats);
        Ok(final_text.trim_end().to_string())
    }
}

pub fn default_threads(setting: u32) -> i32 {
    if setting > 0 {
        return setting as i32;
    }
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // Физических ядер обычно вдвое меньше логических; для декодирования больше потоков не помогает.
    ((n / 2).max(2)).min(12) as i32
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_blocks() {
        assert_eq!(visible_text("<think>reasoning</think>\n\nAnswer", true), "Answer");
        assert_eq!(visible_text("Answer", true), "Answer");
        assert_eq!(visible_text("reasoning</think>Answer", true), "Answer");
    }

    #[test]
    fn hides_unclosed_and_partial_tags_while_streaming() {
        assert_eq!(visible_text("Hello <thi", false), "Hello ");
        assert_eq!(visible_text("Hello <think>still thinking", false), "Hello ");
        assert_eq!(visible_text("Hello <thi", true), "Hello <thi");
    }

    #[test]
    fn prompt_is_chatml_with_empty_think() {
        let p = build_prompt("sys", "usr");
        assert!(p.starts_with("<|im_start|>system\nsys<|im_end|>"));
        assert!(p.ends_with("<think>\n\n</think>\n\n"));
    }
}
