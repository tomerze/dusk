use std::path::PathBuf;
use std::sync::Once;

use dusk_program::anyhow::{Context, Error, Result, anyhow};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};

use super::LlmReply;
use super::params::{CONTEXT_TOKENS, KV_CACHE_TYPE};

const MODEL_DIRECTORY: &str = "git/model";
const MODEL_FILE_PREFIX: &str = "gemma";
const MODEL_FILE_EXTENSION: &str = "gguf";

pub(super) fn parse_reply(text: &str) -> Result<LlmReply> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    let value = match stream.next() {
        Some(Ok(value)) => value,
        Some(Err(error)) => {
            return Err(Error::new(error)
                .context(format!("model did not return JSON; raw reply: {text:?}")));
        }
        None => return Err(anyhow!("no JSON object in reply; raw reply: {text:?}")),
    };
    serde_json::from_value::<LlmReply>(value)
        .with_context(|| format!("reply JSON did not match LlmReply shape; raw reply: {text:?}"))
}

/// Resolve the number of threads llama.cpp should use for decoding.
/// Honours the `DUSK_LLM_THREADS_COUNT` env var when set to a positive
/// integer; otherwise falls back to `std::thread::available_parallelism()`,
/// and finally 1 if even that fails.
pub(super) fn inference_threads() -> i32 {
    if let Ok(value) = std::env::var("DUSK_LLM_THREADS_COUNT") {
        if let Ok(parsed) = value.parse::<i32>() {
            if parsed > 0 {
                return parsed;
            }
        }
    }
    std::thread::available_parallelism()
        .ok()
        .and_then(|count| i32::try_from(count.get()).ok())
        .unwrap_or(1)
}

pub(super) fn locate_model_file() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    let directory = PathBuf::from(home).join(MODEL_DIRECTORY);
    for entry in std::fs::read_dir(&directory)
        .with_context(|| format!("reading {}", directory.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", directory.display()))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if name.starts_with(MODEL_FILE_PREFIX)
            && path.extension().and_then(|e| e.to_str()) == Some(MODEL_FILE_EXTENSION)
        {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "no `{MODEL_FILE_PREFIX}*.{MODEL_FILE_EXTENSION}` file found in {}",
        directory.display()
    ))
}

static INSTALL_LLAMA_LOG_HOOK: Once = Once::new();
pub(super) fn install_llama_log_hook() {
    INSTALL_LLAMA_LOG_HOOK.call_once(|| {
        send_logs_to_tracing(LogOptions::default());
    });
}

pub(super) fn build_context(
    model: &LlamaModel,
    backend: &LlamaBackend,
) -> Result<LlamaContext<'static>> {
    let threads = inference_threads();
    let context_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(CONTEXT_TOKENS))
        .with_type_k(KV_CACHE_TYPE)
        .with_type_v(KV_CACHE_TYPE)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let context = model
        .new_context(backend, context_params)
        .context("creating llama.cpp context")?;
    // SAFETY: `Session` boxes `model` and `backend` and stores them
    // *after* the context in field declaration order, so Drop dismisses
    // the context first. The `'static` lifetime is a lie that the
    // surrounding `Session` invariants make sound.
    Ok(unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(context) })
}
