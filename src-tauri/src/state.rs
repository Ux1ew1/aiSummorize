use crate::llm::Engine;
use crate::settings::Settings;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub engine: Arc<Engine>,
    pub job_cancel: Arc<AtomicBool>,
    pub job_running: Arc<AtomicBool>,
    pub job_seq: AtomicU64,
    pub download_cancel: Arc<AtomicBool>,
    pub downloading: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            engine: Arc::new(Engine::new()),
            job_cancel: Arc::new(AtomicBool::new(false)),
            job_running: Arc::new(AtomicBool::new(false)),
            job_seq: AtomicU64::new(1),
            download_cancel: Arc::new(AtomicBool::new(false)),
            downloading: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
