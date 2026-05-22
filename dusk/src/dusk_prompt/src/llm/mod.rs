//! The dusk LLM session. Two public types — [`Llm`] and [`LlmReply`] —
//! plus two methods on `Llm`. The KV-cache snapshot is produced at
//! build time by `build.rs` and embedded via `include_bytes!`. No
//! runtime warm-up, no on-disk cache, no system prompt at runtime.

use std::sync::Arc;

use dusk_program::anyhow::{Context, Result, anyhow};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

mod helpers;
mod params;
mod session;

use helpers::{build_context, install_llama_log_hook, locate_model_file, parse_reply};
use session::Session;

static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));
include!(concat!(env!("OUT_DIR"), "/snapshot_meta.rs"));

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

#[derive(Clone)]
pub struct Llm {
    state: Arc<Mutex<State>>,
}

enum State {
    Unloaded,
    Loading(JoinHandle<Result<Session>>),
    Ready(Session),
    Failed(String),
}

// SAFETY: `State::Ready` and `State::Loading` hold a `Session` (which
// owns raw llama.cpp handles). See the same-shaped `unsafe impl Send`
// in `session.rs` for the reasoning.
unsafe impl Send for State {}

impl Llm {
    pub fn new() -> Self {
        install_llama_log_hook();
        Self {
            state: Arc::new(Mutex::new(State::Unloaded)),
        }
    }

    /// Run one chat turn. Lazy-loads the model on first call. Returns
    /// the parsed JSON envelope; bubbles up any error.
    pub async fn ask(
        &self,
        message: &str,
        on_token: impl FnMut(usize) + Send + 'static,
    ) -> Result<LlmReply> {
        self.ensure_loaded().await?;
        let raw = self.infer_in_chat(message.to_string(), on_token).await?;
        parse_reply(&raw)
    }

    async fn ensure_loaded(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        match &*guard {
            State::Ready(_) => return Ok(()),
            State::Failed(message) => {
                return Err(anyhow!("llm previously failed to load: {message}"));
            }
            State::Loading(_) => {}
            State::Unloaded => {
                let join: JoinHandle<Result<Session>> = tokio::spawn(async move {
                    tokio::task::spawn_blocking(Self::load_session)
                        .await
                        .context("load task panicked")?
                });
                *guard = State::Loading(join);
            }
        }
        let placeholder = State::Failed(String::from("load in progress"));
        let State::Loading(join) = std::mem::replace(&mut *guard, placeholder) else {
            unreachable!();
        };
        match join.await {
            Ok(Ok(session)) => {
                *guard = State::Ready(session);
                Ok(())
            }
            Ok(Err(error)) => {
                *guard = State::Failed(error.to_string());
                Err(error)
            }
            Err(join_error) => {
                *guard = State::Failed(join_error.to_string());
                Err(anyhow!("llm load task panicked: {join_error}"))
            }
        }
    }

    async fn infer_in_chat<F>(&self, message: String, mut on_token: F) -> Result<String>
    where
        F: FnMut(usize) + Send + 'static,
    {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = state.blocking_lock();
            let State::Ready(session) = &mut *guard else {
                return Err(anyhow!("llm not loaded"));
            };
            session.chat(message, &mut on_token)
        })
        .await
        .context("chat task panicked")?
    }

    fn load_session() -> Result<Session> {
        let model_path = locate_model_file()?;
        let backend = Box::new(LlamaBackend::init().context("initialising llama.cpp backend")?);
        let model = Box::new(
            LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())
                .with_context(|| format!("loading GGUF model from {}", model_path.display()))?,
        );
        let mut context = build_context(&model, &backend)?;

        // SAFETY: SNAPSHOT was produced by build.rs against the same
        // context parameters (CONTEXT_TOKENS, KV_CACHE_TYPE). The upstream
        // API admits "help wanted: not entirely sure what the safety
        // requirements are here" — we trust the round-trip.
        let written = unsafe { context.set_state_data(SNAPSHOT) };
        if written != SNAPSHOT.len() {
            return Err(anyhow!(
                "snapshot load consumed {written} bytes, expected {}",
                SNAPSHOT.len(),
            ));
        }

        Ok(Session::new(context, model, backend, SNAPSHOT_TOKENS))
    }
}

