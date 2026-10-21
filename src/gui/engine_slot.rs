//! The running engine, shared between the window and the tray icon.
//!
//! While the window is hidden egui doesn't run frames, so the tray's menu
//! (lock, quit) can't go through the UI. Both hold an `EngineSlot` instead.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::engine::Engine;

#[derive(Clone, Default)]
pub struct EngineSlot(Arc<Mutex<Option<Engine>>>);

impl EngineSlot {
    fn lock(&self) -> MutexGuard<'_, Option<Engine>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` on the engine if one is running.
    pub fn with<R>(&self, f: impl FnOnce(&Engine) -> R) -> Option<R> {
        self.lock().as_ref().map(f)
    }

    pub fn set(&self, engine: Engine) {
        *self.lock() = Some(engine);
    }

    pub fn is_running(&self) -> bool {
        self.lock().is_some()
    }

    /// Stop the engine (restoring normal input) if one is running.
    pub fn stop(&self) {
        let engine = self.lock().take();
        if let Some(e) = engine {
            e.stop();
        }
    }
}
