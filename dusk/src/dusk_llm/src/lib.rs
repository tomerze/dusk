//! Ask Dusk's local LLM.
//!
//! The chat is primed at build time: `build.rs` runs `dusk_warmup` against
//! the vendored llama.cpp, dumps the resulting KV cache into `OUT_DIR/model.state`,
//! and we ship that file inside the binary via `include_bytes!`. The GGUF
//! itself rides along in a NON-ALLOC ELF section (see `load.rs`) and is read
//! at runtime through a glibc cookie stream.
//!
//! Lifecycle:
//!   `Llm::new()`        — zero-cost; just registers the llama log hook.
//!   `Llm::ask(message)` — on first call, loads the model and applies the
//!                         snapshot on a blocking task; thereafter just
//!                         decodes one chat turn.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::{Arc, Once};

use dusk_program::anyhow::{Context, Result};
use tokio::sync::Mutex;

mod chat;
mod ffi;
mod load;

pub use chat::LlmReply;

use chat::chat;
use ffi::{ggml_log_level, llama_log_set};
use load::{LoadedLlm, load};

#[derive(Clone, Default)]
pub struct Llm {
    loaded: Arc<Mutex<Option<LoadedLlm>>>,
}

impl Llm {
    pub fn new() -> Self {
        install_llama_log_hook();
        Self::default()
    }

    /// Run one chat turn. Lazy-loads the model on first call. `on_token`
    /// fires once per generated token with the running count.
    pub async fn ask(
        &self,
        message: &str,
        on_token: impl FnMut(usize) + Send + 'static,
    ) -> Result<LlmReply> {
        let message = message.to_string();
        let loaded = self.loaded.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = loaded.blocking_lock();
            if guard.is_none() {
                *guard = Some(load()?);
            }
            chat(guard.as_mut().expect("just loaded"), &message, on_token)
        })
        .await
        .context("llm task panicked")?
    }
}

static INSTALL_LLAMA_LOG_HOOK: Once = Once::new();
fn install_llama_log_hook() {
    INSTALL_LLAMA_LOG_HOOK.call_once(|| {
        // SAFETY: trampoline is `extern "C"`, `'static`; llama.cpp stores
        // the pointer indefinitely.
        unsafe { llama_log_set(Some(llama_log_trampoline), ptr::null_mut()) };
    });
}

unsafe extern "C" fn llama_log_trampoline(
    level: ggml_log_level,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let message = String::from_utf8_lossy(bytes);
    let trimmed = message.trim_end_matches('\n');
    if trimmed.is_empty() {
        return;
    }
    match level {
        ggml_log_level::GGML_LOG_LEVEL_ERROR => tracing::error!(target: "llama_cpp", "{trimmed}"),
        ggml_log_level::GGML_LOG_LEVEL_WARN => tracing::warn!(target: "llama_cpp", "{trimmed}"),
        ggml_log_level::GGML_LOG_LEVEL_INFO => tracing::info!(target: "llama_cpp", "{trimmed}"),
        ggml_log_level::GGML_LOG_LEVEL_DEBUG | ggml_log_level::GGML_LOG_LEVEL_CONT => {
            tracing::debug!(target: "llama_cpp", "{trimmed}")
        }
        ggml_log_level::GGML_LOG_LEVEL_NONE => {}
    }
}
