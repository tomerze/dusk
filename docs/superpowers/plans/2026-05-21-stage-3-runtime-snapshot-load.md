# Stage 3 — Runtime loads the embedded snapshot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace today's lazy-load + runtime-warmup runtime with one that `include_bytes!`'s the `model.state` snapshot produced by Stage 2 and feeds it to llama.cpp via `set_state_data` at session-construction time. As part of the same change, restructure `dusk_prompt` so `llm.rs` is pure backend (no stdout, no spinner, no `println!`), `reedline.rs` moves under a `ui/` module that also owns the spinner, and `Llm::ask` returns `Result<LlmReply>` rather than `Option<LlmReply>`.

**Prerequisite:** Stage 2 is complete and merged. `dusk_prompt/build.rs` produces `OUT_DIR/model.state` and `OUT_DIR/snapshot_meta.rs` on every build.

**Architecture:**
- `src/llm.rs` → `src/llm/mod.rs` (directory module). `mod params;` brings in the constants Stage 2 already extracted. `static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));` and `include!(concat!(env!("OUT_DIR"), "/snapshot_meta.rs"));` connect the runtime to the build-time artifact.
- `Llm::new()` takes no arguments. No `system_prompt` field, no FNV hash, no `~/.cache/dusk/`, no `LoadStage` enum, no `warm_up_and_save_cache`, no `build_system_prompt`. The state machine collapses to `Unloaded → Loading → Ready → Failed`.
- `Session::load` (renamed from today's `Loaded::load`): init backend, load model, build context, `unsafe { ctx.set_state_data(SNAPSHOT) }`, set `next_position = SNAPSHOT_TOKENS`, set `had_first_chat = false`. Done.
- `Llm::ask` returns `Result<LlmReply, anyhow::Error>`. No spinner inside, no `println!` for errors. The shell-side `chat_submit` in `prompt.rs` wraps the call with `with_spinner` (from the new `ui::spinner` module) and prints errors itself.
- `src/reedline.rs` → `src/ui/mod.rs`. `src/ui/spinner.rs` is new and holds `with_spinner`, `paint_spinner`, `clear_progress_indicator`.

**Tech Stack:** Rust 2024, `llama-cpp-2` (runtime, already present), `tokio`, `serde_json`, `crossterm`, `nu-ansi-term`.

---

## File Structure

**Created:**
- `dusk/src/dusk_prompt/src/llm/mod.rs` (replaces `src/llm.rs`)
- `dusk/src/dusk_prompt/src/ui/mod.rs` (replaces `src/reedline.rs`)
- `dusk/src/dusk_prompt/src/ui/spinner.rs`

**Modified:**
- `dusk/src/dusk_prompt/src/lib.rs` — replace `mod llm; pub mod prompt; mod reedline;` etc. with the new module declarations.
- `dusk/src/dusk_prompt/src/prompt.rs` — use new import paths; `chat_submit` wraps `Llm::ask` in `with_spinner`, handles `Result`.

**Deleted (by being moved or by becoming dead code):**
- `dusk/src/dusk_prompt/src/llm.rs`
- `dusk/src/dusk_prompt/src/reedline.rs`

**Reused from earlier stages:**
- `dusk/src/dusk_prompt/src/llm/params.rs` (created in Stage 2 Task 1).
- `OUT_DIR/model.state` and `OUT_DIR/snapshot_meta.rs` (written by Stage 2 build.rs).

---

## Task 1: Restructure `reedline.rs` into `ui/mod.rs`

**Files:**
- Move: `dusk/src/dusk_prompt/src/reedline.rs` → `dusk/src/dusk_prompt/src/ui/mod.rs`
- Modify: `dusk/src/dusk_prompt/src/lib.rs`
- Modify: `dusk/src/dusk_prompt/src/prompt.rs`

- [ ] **Step 1: Move the file**

```bash
mkdir -p dusk/src/dusk_prompt/src/ui
git mv dusk/src/dusk_prompt/src/reedline.rs dusk/src/dusk_prompt/src/ui/mod.rs
```

- [ ] **Step 2: Update the module declaration in `lib.rs`**

In `dusk/src/dusk_prompt/src/lib.rs`, find:

```rust
mod reedline;
```

Replace with:

```rust
pub mod ui;
```

(`pub` so the prompt module can refer to it via `crate::ui::…`.)

- [ ] **Step 3: Update all `crate::reedline::…` imports in `prompt.rs`**

In `dusk/src/dusk_prompt/src/prompt.rs`, find the import block:

```rust
use crate::reedline::{
    CommandPrompt, PromptModeFlag, TOGGLE_CHAT_HOST_COMMAND, get_line_editor,
    render_keepalive_suffix,
};
```

Replace with:

```rust
use crate::ui::{
    CommandPrompt, PromptModeFlag, TOGGLE_CHAT_HOST_COMMAND, get_line_editor,
    render_keepalive_suffix,
};
```

- [ ] **Step 4: Verify the crate still builds**

```bash
cargo check -p dusk_prompt
```

Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add dusk/src/dusk_prompt/src/ui/mod.rs dusk/src/dusk_prompt/src/lib.rs dusk/src/dusk_prompt/src/prompt.rs
git commit -m "dusk_prompt: rename reedline module to ui"
```

---

## Task 2: Extract the spinner into `ui/spinner.rs`

**Files:**
- Create: `dusk/src/dusk_prompt/src/ui/spinner.rs`
- Modify: `dusk/src/dusk_prompt/src/ui/mod.rs`
- Modify: `dusk/src/dusk_prompt/src/llm.rs` (temporarily — the file is replaced in Task 4, but we need to remove spinner code from it first to avoid duplicate symbols).

- [ ] **Step 1: Create `dusk/src/dusk_prompt/src/ui/spinner.rs`**

Path: `dusk/src/dusk_prompt/src/ui/spinner.rs`

```rust
use std::io::{Write as _, stdout};

use nu_ansi_term::{Color, Style};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

fn paint(frame: &str, label: &str) {
    let mut handle = stdout().lock();
    let _ = write!(
        handle,
        "\r{} {}\x1b[K",
        Style::new().fg(Color::Yellow).bold().paint(frame),
        Style::new().fg(Color::DarkGray).paint(label),
    );
    let _ = handle.flush();
}

/// Erase the last spinner line painted (resets to column 0 + clear-to-EOL).
pub fn clear() {
    let mut handle = stdout().lock();
    let _ = write!(handle, "\r\x1b[K");
    let _ = handle.flush();
}

/// Drive a braille spinner alongside `future`. `label_for_frame` is
/// invoked on every tick so the caller can show changing state (a
/// running token count, a status message, …) instead of a static label.
/// The spinner is erased before the future's result is returned.
pub async fn with_spinner<F, T, L>(future: F, mut label_for_frame: L) -> T
where
    F: std::future::Future<Output = T>,
    L: FnMut() -> String,
{
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut frame_index = 0usize;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            result = &mut future => {
                clear();
                return result;
            }
            _ = interval.tick() => {
                paint(
                    FRAMES[frame_index % FRAMES.len()],
                    &label_for_frame(),
                );
                frame_index = frame_index.wrapping_add(1);
            }
        }
    }
}
```

- [ ] **Step 2: Expose `spinner` as a submodule of `ui`**

In `dusk/src/dusk_prompt/src/ui/mod.rs`, add at the top of the file (just after the existing `use` lines, before any other items):

```rust
pub mod spinner;
```

- [ ] **Step 3: Remove the spinner code from `llm.rs`**

In `dusk/src/dusk_prompt/src/llm.rs`, delete:
- The `SPINNER_FRAMES` const and `SPINNER_TICK` const.
- The `paint_spinner` function.
- The `clear_progress_indicator` function.
- The `with_spinner` async function.

The `Llm::ask` method in the same file currently calls `with_spinner(...)` internally — that call site is handled in Task 4 when we rewrite the file. For this task, comment-out the body of `ask` so the file still compiles. Replace the body of `pub async fn ask` with:

```rust
pub async fn ask(&self, natural_language: &str) -> Option<LlmReply> {
    // TODO(Stage 3 Task 4): rewrite to return Result<LlmReply> and let the
    // caller drive the spinner. For now keep the signature the prompt.rs
    // currently expects.
    let _ = natural_language;
    None
}
```

This stub keeps the type-checker happy across Stage 3's intermediate tasks. Task 4 rewrites `Llm::ask` properly.

- [ ] **Step 4: Verify the crate still compiles**

```bash
cargo check -p dusk_prompt
```

Expected: `Finished` with warnings about unused imports in `llm.rs` (Color/Style/AtomicUsize/Arc/etc. from the now-removed spinner). Leave those for Task 4 to clean up — they're harmless.

- [ ] **Step 5: Commit**

```bash
git add dusk/src/dusk_prompt/src/ui/spinner.rs dusk/src/dusk_prompt/src/ui/mod.rs dusk/src/dusk_prompt/src/llm.rs
git commit -m "dusk_prompt: move spinner into ui::spinner, stub llm::Llm::ask"
```

---

## Task 3: Convert `llm.rs` into `llm/mod.rs` and wire in the snapshot

**Files:**
- Move: `dusk/src/dusk_prompt/src/llm.rs` → `dusk/src/dusk_prompt/src/llm/mod.rs`
- (Already present from Stage 2): `dusk/src/dusk_prompt/src/llm/params.rs`

- [ ] **Step 1: Move the file**

```bash
git mv dusk/src/dusk_prompt/src/llm.rs dusk/src/dusk_prompt/src/llm/mod.rs
```

- [ ] **Step 2: Update lib.rs**

Open `dusk/src/dusk_prompt/src/lib.rs` and confirm the existing module line:

```rust
pub mod llm;
```

If it was `mod llm;`, change to `pub mod llm;`. Otherwise no change.

- [ ] **Step 3: Verify the crate compiles**

```bash
cargo check -p dusk_prompt
```

Expected: `Finished` with the same unused-import warnings as Task 2. No errors.

- [ ] **Step 4: Commit**

```bash
git add dusk/src/dusk_prompt/src/lib.rs dusk/src/dusk_prompt/src/llm/mod.rs
git commit -m "dusk_prompt: convert llm into a directory module"
```

---

## Task 4: Rewrite `llm/mod.rs` against the embedded snapshot

**Files:**
- Replace contents of: `dusk/src/dusk_prompt/src/llm/mod.rs`

This is the main rewrite. After this task, `llm/mod.rs` is the final shape described in the spec.

- [ ] **Step 1: Replace the entire contents of `dusk/src/dusk_prompt/src/llm/mod.rs`**

New content (the entire file — overwrite):

```rust
//! The dusk LLM session. Two public types — [`Llm`] and [`LlmReply`] —
//! plus two methods on `Llm`. The KV-cache snapshot is produced at
//! build time by `build.rs` and embedded via `include_bytes!`. No
//! runtime warm-up, no on-disk cache, no system prompt at runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;

use dusk_program::anyhow::{Context, Result, anyhow};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

mod params;
use params::{CONTEXT_TOKENS, INFERENCE_THREADS};

// Snapshot artifacts written by build.rs in Stage 2.
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));
include!(concat!(env!("OUT_DIR"), "/snapshot_meta.rs"));
// snapshot_meta.rs provides:  pub(super) const SNAPSHOT_TOKENS: i32 = N;

const MODEL_DIRECTORY: &str = "git/model";
const MODEL_FILE_PREFIX: &str = "gemma";
const MODEL_FILE_EXTENSION: &str = "gguf";

const MAX_RESPONSE_TOKENS: i32 = 256;

// ─── public types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

#[derive(Clone)]
pub struct Llm {
    state: Arc<Mutex<State>>,
}

// ─── private state ───────────────────────────────────────────────────

enum State {
    Unloaded,
    Loading(JoinHandle<Result<Session>>),
    Ready(Session),
    Failed(String),
}

// SAFETY: `Session` and `State` hold llama.cpp objects that are not
// auto-`Send` because they contain raw pointers, but llama.cpp itself
// holds no thread-affinity state — only concurrent access is unsound.
// The surrounding `Mutex<State>` serialises every access, which makes
// moving the wrapped state between threads safe.
unsafe impl Send for Session {}
unsafe impl Send for State {}

struct Session {
    context: Option<LlamaContext<'static>>,
    sampler: LlamaSampler,
    next_position: i32,
    had_first_chat: bool,
    model: Box<LlamaModel>,
    backend: Box<LlamaBackend>,
}

// ─── public API ──────────────────────────────────────────────────────

impl Llm {
    pub fn new() -> Self {
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
        let raw = self.run_inference(message.to_string(), on_token).await?;
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
                    tokio::task::spawn_blocking(Session::load)
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

    async fn run_inference<F>(&self, message: String, mut on_token: F) -> Result<String>
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
}

// ─── session ────────────────────────────────────────────────────────

impl Session {
    fn load() -> Result<Self> {
        install_llama_log_hook();
        let model_path = locate_model_file()?;
        let backend = Box::new(LlamaBackend::init().context("initialising llama.cpp backend")?);
        let model = Box::new(
            LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())
                .with_context(|| format!("loading GGUF model from {}", model_path.display()))?,
        );

        let mut session = Self {
            context: None,
            sampler: LlamaSampler::chain_simple([LlamaSampler::greedy()]),
            next_position: SNAPSHOT_TOKENS,
            had_first_chat: false,
            model,
            backend,
        };
        let mut context = session.build_context()?;

        // SAFETY: SNAPSHOT was produced by build.rs against the same
        // context parameters (CONTEXT_TOKENS, Q8 KV, thread count). The
        // upstream API admits "help wanted: not entirely sure what the
        // safety requirements are here" — we trust the round-trip.
        let written = unsafe { context.set_state_data(SNAPSHOT) };
        if written != SNAPSHOT.len() {
            return Err(anyhow!(
                "snapshot load consumed {written} bytes, expected {}",
                SNAPSHOT.len(),
            ));
        }
        session.context = Some(context);
        Ok(session)
    }

    fn build_context(&mut self) -> Result<LlamaContext<'static>> {
        let context_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(CONTEXT_TOKENS))
            .with_type_k(KvCacheType::Q8_0)
            .with_type_v(KvCacheType::Q8_0)
            .with_n_threads(INFERENCE_THREADS)
            .with_n_threads_batch(INFERENCE_THREADS);
        let context = self
            .model
            .new_context(&self.backend, context_params)
            .context("creating llama.cpp context")?;
        // SAFETY: model and backend are boxed (stable heap addresses)
        // and declared after `context` in `Session`, so Drop order
        // dismisses context first.
        Ok(unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(context) })
    }

    fn chat<F: FnMut(usize)>(&mut self, message: String, on_token: &mut F) -> Result<String> {
        let fragment = if !self.had_first_chat {
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

        let reply = self.sample_reply(first_sample_index, on_token)?;

        let closer = self
            .model
            .str_to_token("<end_of_turn>\n", AddBos::Never)
            .context("tokenising assistant closer")?;
        self.decode_tokens(&closer)?;

        self.had_first_chat = true;
        Ok(reply.trim().to_string())
    }

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

    fn sample_reply<F: FnMut(usize)>(
        &mut self,
        first_sample_index: i32,
        on_token: &mut F,
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
            on_token(count);

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

// ─── helpers ─────────────────────────────────────────────────────────

fn parse_reply(text: &str) -> Result<LlmReply> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    let value = match stream.next() {
        Some(Ok(value)) => value,
        Some(Err(error)) => return Err(error.into()),
        None => return Err(anyhow!("no JSON object in reply")),
    };
    serde_json::from_value::<LlmReply>(value).map_err(Into::into)
}

fn locate_model_file() -> Result<PathBuf> {
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
fn install_llama_log_hook() {
    INSTALL_LLAMA_LOG_HOOK.call_once(|| {
        send_logs_to_tracing(LogOptions::default());
    });
}
```

- [ ] **Step 2: Run cargo check**

```bash
cargo check -p dusk_prompt
```

Expected: errors in `prompt.rs` because it calls `Llm::ask(text)` (one-arg) and expects `Option<String>` back. Those are fixed in Task 5. Within `llm/mod.rs` itself, no errors should appear.

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/src/llm/mod.rs
git commit -m "dusk_prompt: rewrite llm/mod.rs against embedded snapshot, drop runtime warmup"
```

---

## Task 5: Rewrite `prompt.rs`'s chat-submit path

**Files:**
- Modify: `dusk/src/dusk_prompt/src/prompt.rs`

- [ ] **Step 1: Update the imports at the top of `prompt.rs`**

Find the imports block. Replace any existing `use crate::llm::…;` and `use crate::ui::…;` lines so they end up as:

```rust
use crate::llm::{Llm, LlmReply};
use crate::ui::{
    CommandPrompt, PromptModeFlag, TOGGLE_CHAT_HOST_COMMAND, get_line_editor,
    render_keepalive_suffix,
};
use crate::ui::spinner::with_spinner;
```

Also add (somewhere near the top, sorted with other std imports):

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
```

(They will be used by the chat_submit closure pair.)

- [ ] **Step 2: Update `Prompt::new` to drop the system-prompt argument**

In `Prompt::new`, find the lines that currently look like:

```rust
let system_prompt = build_system_prompt(&available_programs_info);
let llm = Llm::new(system_prompt);
```

Replace with (one line — the system prompt is now compiled into the snapshot at build time):

```rust
let llm = Llm::new();
```

Also remove any other reference to `system_prompt` in `prompt.rs` — search with `grep -n system_prompt dusk/src/dusk_prompt/src/prompt.rs`. The `build_system_prompt` function definition itself is removed in Step 4.

- [ ] **Step 3: Rewrite `chat_submit` to use the new `ask` shape with a spinner**

Find the `chat_submit` method. Replace its entire body with:

```rust
async fn chat_submit(&mut self, natural_language: &str) -> Option<String> {
    let natural_language = natural_language.trim();
    if natural_language.is_empty() {
        return None;
    }

    let token_count = Arc::new(AtomicUsize::new(0));
    let writer = token_count.clone();
    let reader = token_count.clone();
    let result = with_spinner(
        self.llm.ask(natural_language, move |n| writer.store(n, Ordering::Relaxed)),
        move || {
            let count = reader.load(Ordering::Relaxed);
            format!(
                "{} {}",
                Style::new().fg(Color::Yellow).paint("Dusking…"),
                Style::new()
                    .fg(Color::DarkGray)
                    .paint(format!("· ↓ {count} tokens")),
            )
        },
    )
    .await;

    let reply = match result {
        Ok(reply) => reply,
        Err(error) => {
            tracing::warn!(%error, "chat request failed");
            println!(
                "{} {}",
                Style::new().fg(Color::Red).paint("chat failed:"),
                error,
            );
            return None;
        }
    };

    println!(
        "{} {}",
        Style::new().fg(Color::White).paint("●"),
        Style::new().fg(Color::White).paint(&reply.explanation),
    );
    let command = reply.command.trim().to_string();
    if command.is_empty() { None } else { Some(command) }
}
```

- [ ] **Step 4: Remove the now-unused `build_system_prompt` function and any `system_prompt` references**

Search for `fn build_system_prompt(` in `prompt.rs`. If present, delete the entire function body and its uses. Search for any remaining `system_prompt` identifier in `prompt.rs` and remove dead code.

```bash
grep -n "build_system_prompt\|system_prompt" dusk/src/dusk_prompt/src/prompt.rs
```

Expected after cleanup: zero matches (or only matches in comments).

- [ ] **Step 5: Verify the crate compiles**

```bash
cargo check -p dusk_prompt
```

Expected: `Finished` with no errors. There may be warnings about unused imports (e.g., `tokio::sync::Mutex as TokioMutex`, `JoinHandle`, etc., that the previous lazy-load code used). Clean those up in the next step.

- [ ] **Step 6: Clean up any newly unused imports in `prompt.rs`**

Run:

```bash
cargo check -p dusk_prompt 2>&1 | grep "warning: unused import"
```

For each warning, delete the corresponding import line. Common candidates after this restructure:
- `use tokio::sync::Mutex as TokioMutex;`
- `use tokio::task::JoinHandle;`
- `use std::sync::atomic::AtomicU8;`

Re-run `cargo check -p dusk_prompt` until warning-free.

- [ ] **Step 7: Commit**

```bash
git add dusk/src/dusk_prompt/src/prompt.rs
git commit -m "dusk_prompt: route chat_submit through Llm::ask + ui::spinner::with_spinner"
```

---

## Task 6: Final-binary verification

**Files:** (none modified)

- [ ] **Step 1: Build the final binary**

```bash
cargo build -p dusk_cli_bin 2>&1 | tail -5
```

Expected: `Finished` with no errors. The binary's text+rodata grows by the snapshot size (~50–250 MB) — that's normal.

- [ ] **Step 2: Smoke-test cold start (no `~/.cache/dusk/`)**

Confirm the runtime cache directory is no longer being created or consulted:

```bash
rm -rf ~/.cache/dusk
```

Start the server:

```bash
cargo run -p dusk_node
```

In another terminal, start a client:

```bash
cargo run -p dusk_cli_bin -- 127.0.0.1:9090
```

In the dusk prompt, press `Ctrl+A` to enter Ask Dusk mode. Type `list processes` and press Enter.

Expected:
- A `⠋ Dusking… · ↓ 0 tokens` spinner appears immediately (no `loading model` phase — the snapshot loads in well under a second).
- Token count climbs as the model streams output.
- Spinner clears, a `● <explanation>` line prints, and the suggested command appears in the next prompt buffer (e.g., `ps`).

```bash
ls ~/.cache/dusk 2>/dev/null
```

Expected: `No such file or directory` — runtime warm-up cache no longer exists.

- [ ] **Step 3: Exit cleanly**

In the dusk shell:

```
> exit
```

Stop the `dusk_node` process.

- [ ] **Step 4: Smoke-test second Ask Dusk in the same session**

Restart the server and client. Press Ctrl+A, send a second prompt (e.g., `show the date`). Confirm the model produces a coherent reply quickly (the spinner should clear in a few seconds with greedy decoding on a 2B model).

- [ ] **Step 5: No commit needed unless something failed**

---

## Task 7: Strip dead-code remnants in `llm/mod.rs`

**Files:** (none modified yet; this is a self-review pass)

After the rewrite, the runtime no longer touches `~/.cache/dusk/`, hashes, the system prompt string, the `LoadStage` enum, or any "warm-up" logic. Verify nothing snuck back in:

- [ ] **Step 1: Sanity-grep**

```bash
grep -n "cache_path\|warm_up\|FNV\|system_prompt\|LoadStage\|stage_label\|~/.cache" \
  dusk/src/dusk_prompt/src/llm/mod.rs dusk/src/dusk_prompt/src/prompt.rs
```

Expected: zero matches.

- [ ] **Step 2: If matches surface, delete the dead code and recommit**

If grep returns hits, that's leftover from the old runtime warm-up. Delete the relevant items, `cargo check`, commit with a message like `dusk_prompt: drop residual runtime warm-up code`.

- [ ] **Step 3: No commit needed if step 1 is clean**

---

## Self-review checklist

- [ ] `dusk_prompt::llm::Llm::new()` takes no arguments.
- [ ] `Llm::ask` returns `Result<LlmReply, anyhow::Error>`.
- [ ] `llm/mod.rs` contains no `println!`, no spinner code, no `with_spinner`, no `ask_fail`.
- [ ] `ui/spinner.rs` is the only home of `with_spinner` / `paint` / `clear`.
- [ ] `~/.cache/dusk/` is not referenced anywhere in the runtime crate.
- [ ] No `system_prompt` field on `Llm`; no `build_system_prompt` function in `prompt.rs`.
- [ ] `cargo build -p dusk_cli_bin` succeeds and produces a working binary.
- [ ] Cold-start Ask Dusk (after `rm -rf ~/.cache/dusk`) completes a chat turn without ever touching that directory.
- [ ] The spinner shows `⠋ Dusking… · ↓ N tokens` end-to-end — no "loading model" phase.
