use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::{Arc, Once};

use dusk_program::anyhow::{Context, Error, Result, anyhow, bail};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::ffi::{
    GGML_TYPE_Q8_0, ggml_log_level, llama_backend_init, llama_batch, llama_batch_free,
    llama_batch_init, llama_context, llama_context_default_params, llama_decode, llama_free,
    llama_init_from_model, llama_log_set, llama_model, llama_model_default_params,
    llama_model_free, llama_model_get_vocab, llama_model_load_from_file_ptr, llama_pos,
    llama_sampler, llama_sampler_accept, llama_sampler_chain_add,
    llama_sampler_chain_default_params, llama_sampler_chain_init, llama_sampler_free,
    llama_sampler_init_dist, llama_sampler_init_temp, llama_sampler_init_top_k,
    llama_sampler_sample, llama_state_set_data, llama_token, llama_token_to_piece, llama_tokenize,
    llama_vocab, llama_vocab_is_eog,
};
use crate::load::EmbeddedGgufFile;

/// KV-cache snapshot produced at build time by `dusk_warmup`.
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dusk_llm_kv_snapshot"));

// llama.cpp session file header: u32 magic, u32 version, u32 n_tokens,
// llama_token[n_tokens], then raw `llama_state_get_data` bytes. Pinned to
// the submodule revision (vendor/llama.cpp, tag b9282).
const LLAMA_SESSION_MAGIC: u32 = 0x6767736e; // 'ggsn'
const LLAMA_SESSION_VERSION: u32 = 9;

// Must match build.rs (the warmup binary builds the KV cache with these).
const CONTEXT_TOKENS: u32 = 16_384;
const KV_CACHE_TYPE: c_int = GGML_TYPE_Q8_0;

// Sampler chain: top-K narrows the candidate set, temperature rescales,
// dist samples from the resulting distribution.
const SAMPLER_TOP_K: i32 = 20;
const SAMPLER_TEMPERATURE: f32 = 0.6;
// LLAMA_DEFAULT_SEED in llama.h — pick a fresh random seed at init.
const SAMPLER_SEED: u32 = 0xFFFF_FFFF;

const MAX_RESPONSE_TOKENS: i32 = 1024;

pub(crate) struct LlmState {
    pub(crate) context: *mut llama_context,
    pub(crate) model: *mut llama_model,
    pub(crate) sampler: *mut llama_sampler,
    pub(crate) next_position: i32,
    pub(crate) had_first_chat: bool,
}

// SAFETY: llama.cpp has no thread-affinity state — only concurrent access
// is unsound. The `Mutex<LlmState>` in `Chat` serialises every access.
unsafe impl Send for LlmState {}

impl Drop for LlmState {
    fn drop(&mut self) {
        // Reverse construction order: sampler refs context, context refs model.
        unsafe {
            llama_sampler_free(self.sampler);
            llama_free(self.context);
            llama_model_free(self.model);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

#[derive(Clone)]
pub struct Chat {
    loaded: Arc<Mutex<LlmState>>,
}

impl Chat {
    /// Load the model from an already-opened embedded GGUF. Blocks — call
    /// from `spawn_blocking`.
    pub fn new(embedded: EmbeddedGgufFile) -> Result<Self> {
        install_llama_log_hook(); // llama.cpp logs a bunch of trash, this hooks it into traces
        let state = build_llm_state(embedded)?;
        Ok(Self {
            loaded: Arc::new(Mutex::new(state)),
        })
    }

    /// Run one chat turn. `on_token` fires once per generated token with the
    /// running count.
    pub async fn chat(
        &self,
        message: &str,
        on_token: impl FnMut(usize) + Send + 'static,
    ) -> Result<LlmReply> {
        let message = message.to_string();
        let loaded = self.loaded.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = loaded.blocking_lock();
            Self::chat_inner(&mut guard, &message, on_token)
        })
        .await
        .context("llm task panicked")?
    }

    /// Run one chat turn against `state`. `on_token` fires once per generated
    /// token with the running count.
    fn chat_inner<F: FnMut(usize)>(
        state: &mut LlmState,
        message: &str,
        mut on_token: F,
    ) -> Result<LlmReply> {
        // First chat continues the user turn the snapshot left half-open;
        // subsequent turns open their own `<|turn>user` block.
        let fragment = if state.had_first_chat {
            format!("<|turn>user\n{message}<turn|>\n<|turn>model\n")
        } else {
            format!("{message}<turn|>\n<|turn>model\n")
        };

        let tokens = tokenize(state, &fragment)?;
        if tokens.is_empty() {
            bail!("chat fragment tokenised to zero tokens");
        }
        decode(state, &tokens)?;
        let last_index: i32 = (tokens.len() - 1)
            .try_into()
            .context("sample index overflow")?;

        let reply = sample_reply(state, last_index, &mut on_token)?;

        let closer = tokenize(state, "<end_of_turn>\n")?;
        decode(state, &closer)?;

        state.had_first_chat = true;
        parse_reply(reply.trim())
    }
}

/// Build a context from an already-opened embedded GGUF and apply the
/// build-time KV-cache snapshot. Blocks — call from `spawn_blocking`.
fn build_llm_state(embedded: EmbeddedGgufFile) -> Result<LlmState> {
    unsafe { llama_backend_init() };

    let (snapshot_tokens, raw_state) = parse_snapshot(SNAPSHOT)?;

    let mut model_params = unsafe { llama_model_default_params() };
    model_params.use_mmap = false;
    let model = unsafe { llama_model_load_from_file_ptr(embedded.file, model_params) };
    if model.is_null() {
        bail!("llama_model_load_from_file_ptr returned null for embedded GGUF");
    }

    let context = match build_context(model) {
        Ok(context) => context,
        Err(error) => {
            unsafe { llama_model_free(model) };
            return Err(error);
        }
    };

    let written = unsafe { llama_state_set_data(context, raw_state.as_ptr(), raw_state.len()) };
    if written != raw_state.len() {
        unsafe { llama_free(context) };
        unsafe { llama_model_free(model) };
        bail!(
            "snapshot load consumed {written} bytes, expected {}",
            raw_state.len()
        );
    }

    let sampler = match build_sampler() {
        Ok(sampler) => sampler,
        Err(error) => {
            unsafe { llama_free(context) };
            unsafe { llama_model_free(model) };
            return Err(error);
        }
    };

    Ok(LlmState {
        context,
        model,
        sampler,
        next_position: snapshot_tokens,
        had_first_chat: false,
    })
}

/// Parse a `llama_state_save_file` session file. Returns `(n_tokens, raw_kv_bytes)`.
fn parse_snapshot(bytes: &[u8]) -> Result<(i32, &[u8])> {
    if bytes.len() < 12 {
        bail!("snapshot truncated: {} bytes < 12-byte header", bytes.len());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let n_tokens = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if magic != LLAMA_SESSION_MAGIC {
        bail!("snapshot magic mismatch: got {magic:#010x}, expected {LLAMA_SESSION_MAGIC:#010x}");
    }
    if version != LLAMA_SESSION_VERSION {
        bail!(
            "snapshot version mismatch: got {version}, expected {LLAMA_SESSION_VERSION} \
             (vendor/llama.cpp moved out of sync with this build)"
        );
    }
    let tokens_byte_len = (n_tokens as usize)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("snapshot token count overflows usize: {n_tokens}"))?;
    let tail_start = 12usize
        .checked_add(tokens_byte_len)
        .ok_or_else(|| anyhow!("snapshot header arithmetic overflowed"))?;
    if bytes.len() < tail_start {
        bail!(
            "snapshot truncated: header claims {n_tokens} tokens but only {} bytes available",
            bytes.len() - 12
        );
    }
    let n_tokens_i32: i32 = n_tokens
        .try_into()
        .context("snapshot token count overflows i32")?;
    Ok((n_tokens_i32, &bytes[tail_start..]))
}

fn build_context(model: *mut llama_model) -> Result<*mut llama_context> {
    let threads = inference_threads();
    let mut params = unsafe { llama_context_default_params() };
    params.n_ctx = CONTEXT_TOKENS;
    params.n_threads = threads;
    params.n_threads_batch = threads;
    params.type_k = KV_CACHE_TYPE;
    params.type_v = KV_CACHE_TYPE;

    let context = unsafe { llama_init_from_model(model, params) };
    if context.is_null() {
        bail!("llama_init_from_model returned null");
    }
    Ok(context)
}

fn build_sampler() -> Result<*mut llama_sampler> {
    let params = unsafe { llama_sampler_chain_default_params() };
    let chain = unsafe { llama_sampler_chain_init(params) };
    if chain.is_null() {
        bail!("llama_sampler_chain_init returned null");
    }
    let stages: [(*mut llama_sampler, &str); 3] = unsafe {
        [
            (
                llama_sampler_init_top_k(SAMPLER_TOP_K),
                "llama_sampler_init_top_k",
            ),
            (
                llama_sampler_init_temp(SAMPLER_TEMPERATURE),
                "llama_sampler_init_temp",
            ),
            (
                llama_sampler_init_dist(SAMPLER_SEED),
                "llama_sampler_init_dist",
            ),
        ]
    };
    for (stage, name) in stages {
        if stage.is_null() {
            unsafe { llama_sampler_free(chain) };
            bail!("{name} returned null");
        }
        unsafe { llama_sampler_chain_add(chain, stage) };
    }
    Ok(chain)
}

/// Honour `DUSK_LLM_THREADS_COUNT` if it's a positive integer; otherwise
/// fall back to host parallelism, then 1.
fn inference_threads() -> i32 {
    if let Ok(value) = std::env::var("DUSK_LLM_THREADS_COUNT")
        && let Ok(parsed) = value.parse::<i32>()
        && parsed > 0
    {
        return parsed;
    }
    std::thread::available_parallelism()
        .ok()
        .and_then(|count| i32::try_from(count.get()).ok())
        .unwrap_or(1)
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

/// Extract the first JSON object from the model's raw reply. The model
/// occasionally prefixes with stray few-shot echo text, so we skip to the
/// first `{` and parse one value off the stream (ignoring any trailing
/// noise).
fn parse_reply(text: &str) -> Result<LlmReply> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("no `{{` in reply; raw reply: {text:?}"))?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<LlmReply>();
    match stream.next() {
        Some(Ok(reply)) => Ok(reply),
        Some(Err(error)) => Err(Error::new(error).context(format!(
            "model did not return a valid LlmReply; raw reply: {text:?}"
        ))),
        None => Err(anyhow!("no JSON object in reply; raw reply: {text:?}")),
    }
}

fn vocab(state: &LlmState) -> *const llama_vocab {
    unsafe { llama_model_get_vocab(state.model) }
}

/// Tokenize a chat continuation. `add_special=false` because the snapshot
/// already contains the BOS. `parse_special=true` so chat-template
/// markers (`<|turn>`, `<turn|>`) tokenize to their dedicated special-token
/// IDs (105 and 106 for Gemma 4) instead of multi-token text.
fn tokenize(state: &LlmState, text: &str) -> Result<Vec<llama_token>> {
    let bytes = text.as_bytes();
    let text_len: i32 = bytes
        .len()
        .try_into()
        .context("tokenizer input overflows i32")?;
    let text_ptr = bytes.as_ptr() as *const c_char;
    let vocab = vocab(state);

    // Probe: null buffer returns -(required slots), or i32::MIN on overflow.
    let probe =
        unsafe { llama_tokenize(vocab, text_ptr, text_len, ptr::null_mut(), 0, false, true) };
    if probe == i32::MIN {
        bail!("tokenization overflowed i32");
    }
    let needed: usize = (-probe)
        .try_into()
        .context("tokenizer reported negative capacity")?;
    let mut tokens = vec![0_i32; needed];
    let written = unsafe {
        llama_tokenize(
            vocab,
            text_ptr,
            text_len,
            tokens.as_mut_ptr(),
            needed as i32,
            false,
            true,
        )
    };
    if written < 0 {
        bail!("llama_tokenize failed (returned {written})");
    }
    tokens.truncate(written as usize);
    Ok(tokens)
}

/// Decode `tokens` into the KV cache, starting at `state.next_position`.
/// Only the last token's logits are kept (used by `sample_reply`).
fn decode(state: &mut LlmState, tokens: &[llama_token]) -> Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }
    let count: i32 = tokens
        .len()
        .try_into()
        .context("token batch overflows i32")?;
    let mut batch = unsafe { llama_batch_init(count, 0, 1) };

    let last = tokens.len() - 1;
    for (offset, &token) in tokens.iter().enumerate() {
        let position = state
            .next_position
            .checked_add(offset as i32)
            .ok_or_else(|| anyhow!("KV position overflows i32"))?;
        unsafe { write_batch_slot(&mut batch, offset, token, position, offset == last) };
    }
    batch.n_tokens = count;

    let status = unsafe { llama_decode(state.context, batch) };
    unsafe { llama_batch_free(batch) };

    if status != 0 {
        bail!("llama_decode returned {status}");
    }
    state.next_position = state
        .next_position
        .checked_add(count)
        .ok_or_else(|| anyhow!("KV position overflows i32"))?;
    Ok(())
}

/// Sample tokens until end-of-generation or `MAX_RESPONSE_TOKENS`,
/// feeding each accepted token back into the KV cache. `on_token`
/// fires with the running count after each token is decoded.
fn sample_reply<F: FnMut(usize)>(
    state: &mut LlmState,
    first_index: i32,
    on_token: &mut F,
) -> Result<String> {
    let mut reply_bytes: Vec<u8> = Vec::new();
    let mut sample_index = first_index;
    let mut count: usize = 0;

    for _ in 0..MAX_RESPONSE_TOKENS {
        let token = unsafe { llama_sampler_sample(state.sampler, state.context, sample_index) };
        unsafe { llama_sampler_accept(state.sampler, token) };
        if unsafe { llama_vocab_is_eog(vocab(state), token) } {
            break;
        }

        let piece = token_to_piece(state, token)?;
        reply_bytes.extend_from_slice(&piece);
        count += 1;
        on_token(count);

        decode(state, &[token])?;
        // After decoding a single token, logits sit in row 0.
        sample_index = 0;
    }
    Ok(String::from_utf8_lossy(&reply_bytes).into_owned())
}

fn token_to_piece(state: &LlmState, token: llama_token) -> Result<Vec<u8>> {
    let vocab = vocab(state);
    let required = unsafe { llama_token_to_piece(vocab, token, ptr::null_mut(), 0, 0, false) };
    if required == 0 {
        return Ok(Vec::new());
    }
    let needed = required.unsigned_abs() as usize;
    let mut buffer = vec![0_u8; needed];
    let written = unsafe {
        llama_token_to_piece(
            vocab,
            token,
            buffer.as_mut_ptr() as *mut c_char,
            needed as i32,
            0,
            false,
        )
    };
    if written < 0 {
        bail!("llama_token_to_piece failed (returned {written})");
    }
    buffer.truncate(written as usize);
    Ok(buffer)
}

/// Fill slot `index` of a `llama_batch_init`-allocated batch. Sequence id
/// is always 0 (single-sequence inference).
///
/// # Safety
/// `batch` must have been created with `llama_batch_init(capacity, _, 1)`
/// with `capacity > index`.
unsafe fn write_batch_slot(
    batch: &mut llama_batch,
    index: usize,
    token: llama_token,
    position: llama_pos,
    keep_logits: bool,
) {
    unsafe {
        *batch.token.add(index) = token;
        *batch.pos.add(index) = position;
        *batch.n_seq_id.add(index) = 1;
        // batch.seq_id[index] points at a pre-allocated sub-array of length n_seq_max (= 1).
        *(*batch.seq_id.add(index)) = 0;
        *batch.logits.add(index) = if keep_logits { 1 } else { 0 };
    }
}
