//! The dusk LLM session. One public type ([`Llm`]) handles lazy
//! loading, KV-cache persistence, Gemma chat-template scaffolding,
//! streaming inference, a UI spinner, and JSON-envelope parsing. The
//! interface the shell sees is exactly two methods:
//!
//! - [`Llm::new(system_prompt)`](Llm::new) — cheap; touches no model.
//! - [`Llm::ask(&str) -> Option<LlmReply>`](Llm::ask) — orchestrates
//!   everything; returns the parsed reply or `None` with errors
//!   already logged + printed.

use std::io::{Write as _, stdout};
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use dusk_program::anyhow::{Context, Result, anyhow};
use dusk_program_sh::entry::ProgramInfo;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use nu_ansi_term::{Color, Style};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const MODEL_DIRECTORY: &str = "git/model";
const MODEL_FILE_PREFIX: &str = "gemma";
const MODEL_FILE_EXTENSION: &str = "gguf";

const CONTEXT_TOKENS: u32 = 4_096;
const MAX_RESPONSE_TOKENS: i32 = 256;
const INFERENCE_THREADS: i32 = 4;

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

#[derive(Clone)]
pub struct Llm {
    system_prompt: Arc<String>,
    state: Arc<Mutex<State>>,
}


enum State {
    Unloaded,
    Loading(JoinHandle<Result<Loaded>>),
    Ready(Loaded),
    Failed(String),
}

unsafe impl Send for Loaded {}
unsafe impl Send for State {}

struct Loaded {
    context: Option<LlamaContext<'static>>,
    sampler: LlamaSampler,
    next_position: i32,
    had_first_chat: bool,
    model: Box<LlamaModel>,
    backend: Box<LlamaBackend>,
}

// ─── public API ──────────────────────────────────────────────────────

impl Llm {
    /// Construct without touching the model. Cheap — the baseline cost
    /// of a `dusk` shell stays at ~25 MB until the user actually opens
    /// the Ask Duck dialogue.
    pub fn new(system_prompt: String) -> Self {
        Self {
            system_prompt: Arc::new(system_prompt),
            state: Arc::new(Mutex::new(State::Unloaded)),
        }
    }

    /// One-shot natural-language → [`LlmReply`] orchestration. Drives a
    /// spinner through the load phase (first time only) and the
    /// inference phase, parses the JSON envelope, and surfaces any
    /// failure as `None` with a log line + user-visible `chat failed: …`.
    pub async fn ask(&self, natural_language: &str) -> Option<LlmReply> {
        let natural_language = natural_language.trim();
        if natural_language.is_empty() {
            return None;
        }

        let token_count = Arc::new(AtomicUsize::new(0));
        let writer = token_count.clone();
        let reader = token_count.clone();
        let work = async move {
            self.ensure_loaded().await?;
            self.run_inference(natural_language.to_string(), move |n| {
                writer.store(n, Ordering::Relaxed);
            })
            .await
        };
        let reply_text = match with_spinner(work, move || {
            let count = reader.load(Ordering::Relaxed);
            format!(
                "{} {}",
                Style::new().fg(Color::Yellow).paint("Dusking…"),
                Style::new()
                    .fg(Color::DarkGray)
                    .paint(format!("· ↓ {count} tokens")),
            )
        })
        .await
        {
            Ok(text) => text,
            Err(error) => return ask_fail(error, "chat request failed"),
        };

        match parse_llm_reply(&reply_text) {
            Ok(reply) => Some(reply),
            Err(error) => {
                tracing::warn!(%error, raw = %reply_text, "llm reply was not valid JSON");
                None
            }
        }
    }

    /// Transition `Unloaded → Loading → Ready` (or `Failed`). Idempotent:
    /// once `Ready` further calls just return immediately.
    async fn ensure_loaded(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        match &*guard {
            State::Ready(_) => return Ok(()),
            State::Failed(message) => {
                return Err(anyhow!("llm previously failed to load: {message}"));
            }
            State::Loading(_) => {}
            State::Unloaded => {
                let prompt = (*self.system_prompt).clone();
                let join: JoinHandle<Result<Loaded>> = tokio::spawn(async move {
                    tokio::task::spawn_blocking(move || Loaded::load(&prompt))
                        .await
                        .context("load task panicked")?
                });
                *guard = State::Loading(join);
            }
        }
        // Take ownership of the join handle and await it.
        let placeholder = State::Failed(String::from("load in progress"));
        let State::Loading(join) = std::mem::replace(&mut *guard, placeholder) else {
            unreachable!("guarded above");
        };
        match join.await {
            Ok(Ok(loaded)) => {
                *guard = State::Ready(loaded);
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

    /// Run one chat turn on the loaded model. The caller is expected to
    /// have awaited [`ensure_loaded`] first.
    async fn run_inference<F>(&self, message: String, mut progress: F) -> Result<String>
    where
        F: FnMut(usize) + Send + 'static,
    {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = state.blocking_lock();
            let State::Ready(loaded) = &mut *guard else {
                return Err(anyhow!("llm not loaded"));
            };
            loaded.chat(message, &mut progress)
        })
        .await
        .context("chat task panicked")?
    }
}

impl Loaded {
    fn load(system_prompt: &str) -> Result<Self> {
        install_llama_log_hook();

        let model_path = locate_model_file()?;
        let backend = Box::new(LlamaBackend::init().context("initialising llama.cpp backend")?);
        let model = Box::new(
            LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())
                .with_context(|| format!("loading GGUF model from {}", model_path.display()))?,
        );

        let mut loaded = Self {
            context: None,
            sampler: LlamaSampler::chain_simple([LlamaSampler::greedy()]),
            next_position: 0,
            had_first_chat: false,
            model,
            backend,
        };
        loaded.context = Some(loaded.build_context()?);

        // Try to restore the warmed KV cache from disk. The cache file
        // is keyed on `(model_path, system_prompt)` so any change to
        // either invalidates it automatically.
        let cache_path = state_cache_path(&model_path, system_prompt);
        let cache_hit = match loaded
            .context
            .as_mut()
            .unwrap()
            .state_load_file(&cache_path, CONTEXT_TOKENS as usize)
        {
            Ok(tokens) => {
                tracing::debug!(
                    cache = %cache_path.display(),
                    n_tokens = tokens.len(),
                    "restored warmed KV cache",
                );
                loaded.next_position =
                    tokens.len().try_into().context("cached KV length overflows i32")?;
                true
            }
            Err(error) => {
                tracing::debug!(cache = %cache_path.display(), %error, "no cached KV state");
                false
            }
        };

        if !cache_hit {
            if let Err(error) = loaded.warm_up_and_save_cache(system_prompt, &cache_path) {
                tracing::warn!(%error, "warm-up failed; first chat will be slower");
            }
        }

        Ok(loaded)
    }

    /// Build a fresh `LlamaContext` against this `Loaded`'s `model` /
    /// `backend` and unsafely promote its lifetime to `'static`.
    ///
    /// SAFETY: `model` and `backend` are `Box`ed so their heap addresses
    /// are stable for the rest of `self`'s life. `context` is declared
    /// before `model` / `backend` in [`Loaded`], so Drop runs in the
    /// borrow's safe direction (context first, then model + backend).
    fn build_context(&mut self) -> Result<LlamaContext<'static>> {
        let context_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS))
            // Q8 KV cache halves the per-token footprint vs F16, with
            // negligible quality cost for short translation generations.
            .with_type_k(KvCacheType::Q8_0)
            .with_type_v(KvCacheType::Q8_0)
            .with_n_threads(INFERENCE_THREADS)
            .with_n_threads_batch(INFERENCE_THREADS);
        let context = self
            .model
            .new_context(&self.backend, context_params)
            .context("creating llama.cpp context")?;
        Ok(unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(context) })
    }

    /// Decode `<bos><start_of_turn>user\n{system_prompt}\n\n` into KV
    /// and persist the resulting state to `cache_path`.
    fn warm_up_and_save_cache(&mut self, system_prompt: &str, cache_path: &Path) -> Result<()> {
        let opening = format!("<start_of_turn>user\n{system_prompt}\n\n");
        let tokens = self
            .model
            .str_to_token(&opening, AddBos::Always)
            .context("tokenising warm-up prefix")?;
        self.decode_tokens(&tokens)?;

        if let Some(parent) = cache_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!(%error, dir = %parent.display(), "couldn't create cache dir");
                return Ok(());
            }
        }
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| anyhow!("llama context not initialised"))?;
        match context.state_save_file(cache_path, &tokens) {
            Ok(()) => tracing::info!(cache = %cache_path.display(), "wrote warmed KV cache"),
            Err(error) => tracing::warn!(%error, "failed to write warm-up cache"),
        }
        Ok(())
    }

    /// One conversation turn. Appends the new fragment to the KV cache
    /// (no re-tokenisation, no drift), samples the assistant reply,
    /// then writes the closing `<end_of_turn>\n` so the next call can
    /// open a fresh user turn from the right boundary.
    fn chat<F: FnMut(usize)>(&mut self, message: String, progress: &mut F) -> Result<String> {
        let fragment = if !self.had_first_chat {
            // Warm-up parked us inside the first user turn: finish it.
            format!("{message}<end_of_turn>\n<start_of_turn>model\n")
        } else {
            format!("<start_of_turn>user\n{message}<end_of_turn>\n<start_of_turn>model\n")
        };
        let tokens = self
            .model
            .str_to_token(&fragment, AddBos::Never)
            .context("tokenising chat fragment")?;
        if tokens.is_empty() {
            return Err(anyhow!("chat fragment tokenised to zero tokens"));
        }
        let first_sample_index: i32 = (tokens.len() - 1)
            .try_into()
            .context("sample index overflow")?;
        self.decode_tokens(&tokens)?;

        let reply = self.sample_reply(first_sample_index, progress)?;

        let closer = self
            .model
            .str_to_token("<end_of_turn>\n", AddBos::Never)
            .context("tokenising assistant closer")?;
        self.decode_tokens(&closer)?;

        self.had_first_chat = true;
        Ok(reply.trim().to_string())
    }

    /// Decode a pre-tokenised slice into the KV cache, bumping
    /// `next_position`. Logits are requested on the last token so the
    /// caller can sample immediately afterwards.
    fn decode_tokens(&mut self, tokens: &[LlamaToken]) -> Result<()> {
        if tokens.is_empty() {
            return Ok(());
        }
        let context = self
            .context
            .as_mut()
            .ok_or_else(|| anyhow!("llama context not initialised"))?;
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        let last_index = tokens.len() - 1;
        for (offset, token) in tokens.iter().enumerate() {
            let position: i32 = self
                .next_position
                .checked_add(offset as i32)
                .ok_or_else(|| anyhow!("KV position overflows i32"))?;
            batch
                .add(*token, position, &[0], offset == last_index)
                .context("adding token to batch")?;
        }
        context.decode(&mut batch).context("decoding tokens")?;
        self.next_position = self
            .next_position
            .checked_add(tokens.len() as i32)
            .ok_or_else(|| anyhow!("KV position overflows i32"))?;
        Ok(())
    }

    /// Sample the assistant reply token-by-token, decoding each into
    /// the KV cache so the next iteration's logits are available.
    fn sample_reply<F: FnMut(usize)>(
        &mut self,
        first_sample_index: i32,
        progress: &mut F,
    ) -> Result<String> {
        let context = self
            .context
            .as_mut()
            .ok_or_else(|| anyhow!("llama context not initialised"))?;
        let mut reply = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut sample_idx = first_sample_index;
        let mut count: usize = 0;
        let mut batch = LlamaBatch::new(1, 1);
        for _ in 0..MAX_RESPONSE_TOKENS {
            let token = self.sampler.sample(context, sample_idx);
            self.sampler.accept(token);
            if self.model.is_eog_token(token) {
                break;
            }
            let piece = self
                .model
                .token_to_piece(token, &mut decoder, false, None)
                .context("converting sampled token to text")?;
            reply.push_str(&piece);
            count += 1;
            progress(count);

            batch.clear();
            batch
                .add(token, self.next_position, &[0], true)
                .context("adding sampled token to batch")?;
            self.next_position = self
                .next_position
                .checked_add(1)
                .ok_or_else(|| anyhow!("KV position overflows i32"))?;
            context
                .decode(&mut batch)
                .context("decoding sampled token")?;
            sample_idx = 0;
        }
        Ok(reply)
    }
}

// ─── helpers: spinner / parsing / cache path / model locator ─────────

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_TICK: std::time::Duration = std::time::Duration::from_millis(80);

fn paint_spinner(frame: &str, label: &str) {
    let mut handle = stdout().lock();
    let _ = write!(
        handle,
        "\r{} {}\x1b[K",
        Style::new().fg(Color::Yellow).bold().paint(frame),
        Style::new().fg(Color::DarkGray).paint(label),
    );
    let _ = handle.flush();
}

fn clear_progress_indicator() {
    let mut handle = stdout().lock();
    let _ = write!(handle, "\r\x1b[K");
    let _ = handle.flush();
}

/// Drive a braille spinner alongside `future`. `label_for_frame` is
/// invoked each tick so the caller can show changing state.
async fn with_spinner<F, T, L>(future: F, mut label_for_frame: L) -> T
where
    F: std::future::Future<Output = T>,
    L: FnMut() -> String,
{
    let mut interval = tokio::time::interval(SPINNER_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut frame_index = 0usize;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            result = &mut future => {
                clear_progress_indicator();
                return result;
            }
            _ = interval.tick() => {
                paint_spinner(
                    SPINNER_FRAMES[frame_index % SPINNER_FRAMES.len()],
                    &label_for_frame(),
                );
                frame_index = frame_index.wrapping_add(1);
            }
        }
    }
}

fn ask_fail<E: std::fmt::Display>(error: E, log_message: &str) -> Option<LlmReply> {
    tracing::warn!(error = %error, "{log_message}");
    println!(
        "{} {}",
        Style::new().fg(Color::Red).paint("chat failed:"),
        error,
    );
    None
}

/// Parse the first JSON value out of `text`, ignoring trailing content
/// (whitespace, stray newlines, follow-up prose). Tolerant of duplicate
/// fields — the model occasionally repeats `explanation` or `command`;
/// we parse to a generic `serde_json::Value` (last-wins on duplicates)
/// and then project into [`LlmReply`].
fn parse_llm_reply(text: &str) -> Result<LlmReply> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    let value = match stream.next() {
        Some(Ok(value)) => value,
        Some(Err(error)) => return Err(error.into()),
        None => return Err(anyhow!("no JSON object in reply")),
    };
    serde_json::from_value::<LlmReply>(value).map_err(Into::into)
}

/// Route llama.cpp / ggml log output into tracing exactly once for the
/// life of the process.
static INSTALL_LLAMA_LOG_HOOK: Once = Once::new();
fn install_llama_log_hook() {
    INSTALL_LLAMA_LOG_HOOK.call_once(|| {
        send_logs_to_tracing(LogOptions::default());
    });
}

/// Stable cache-file location for a warmed KV state, keyed on the
/// `(model_path, system_prompt)` pair so any change to either
/// invalidates the cache automatically. Hash is FNV-1a — collision
/// risk is astronomical for our ~1 KB inputs.
fn state_cache_path(model_path: &Path, system_prompt: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let fnv = |hash: &mut u64, bytes: &[u8]| {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    fnv(&mut hash, model_path.as_os_str().as_bytes());
    fnv(&mut hash, b"\0");
    fnv(&mut hash, system_prompt.as_bytes());

    let cache_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cache")
        .join("dusk");
    cache_root.join(format!("duck-{hash:016x}.state"))
}

/// Find the GGUF model file under `~/git/model/`. Picks the first entry
/// whose filename starts with [`MODEL_FILE_PREFIX`] and ends with `.gguf`.
fn locate_model_file() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    let directory = PathBuf::from(home).join(MODEL_DIRECTORY);
    let entries = std::fs::read_dir(&directory)
        .with_context(|| format!("reading {}", directory.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", directory.display()))?;
        let path = entry.path();
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name,
            None => continue,
        };
        if name.starts_with(MODEL_FILE_PREFIX)
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                == Some(MODEL_FILE_EXTENSION)
        {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "no `{MODEL_FILE_PREFIX}*.{MODEL_FILE_EXTENSION}` file found in {}",
        directory.display()
    ))
}

// ─── system prompt builder ───────────────────────────────────────────

/// Build the system prompt fed to the LLM at session start. Compact on
/// purpose — every token here is decoded on first chat. The model gets
/// dusk's grammar, the JSON envelope rule, and a one-line summary of
/// each available program. No long-form help text, no per-example JSON
/// pairs.
pub fn build_system_prompt(available_programs_info: &[ProgramInfo]) -> String {
    let mut buffer = String::from(
        "You are Duck, the assistant for the dusk shell. dusk uses POSIX-sh syntax for \
         control flow, but its programs are NOT GNU/coreutils — only the flags listed \
         below exist. Do not invent flags.\n\n\
         Grammar (everything dusk supports, nothing else exists):\n\
         - `cmd args` — words split on whitespace; quote with `'…'` or `\"…\"`.\n\
         - `a ; b` or newline — sequence.\n\
         - `a && b`, `a || b` — short-circuit AND/OR on exit status.\n\
         - `sh -d \"cmd\"` — background/detached. There is NO `&` postfix.\n\
         - `name() { body }` — define. Body is `;`-separated statements.\n\
         - `name` — call. Functions are invoked by bare name, just like bash/sh \
         (`hi() { echo hi; }` then `hi`). NEVER write `name()` to call. Recurse by \
         name; that's how you loop, since there is no `while` or `for`.\n\
         Not in dusk: pipes `|`, redirection `> >> <`, `&` postfix, subshells `(…)`, \
         `$VAR`, `$(…)`, backticks, globs. If a request needs any of these, return an \
         empty `command` and explain.\n\n\
         Reply with ONE JSON object, nothing else: \
         {\"explanation\": \"<one short sentence>\", \"command\": \"<dusk command or empty>\"}. \
         Exactly one of each key. No markdown, no code fences, no prose outside the JSON.\n\n\
         Available programs:\n",
    );
    for program in available_programs_info {
        buffer.push_str("\n## ");
        buffer.push_str(program.name);
        buffer.push_str(" — ");
        buffer.push_str(program.short_description);
        buffer.push('\n');
        let long = program.long_description.trim();
        if !long.is_empty() {
            buffer.push_str(long);
            buffer.push('\n');
        }
    }
    buffer
}
