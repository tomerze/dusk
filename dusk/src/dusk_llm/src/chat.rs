use std::ffi::c_char;
use std::ptr;

use dusk_program::anyhow::{Context, Error, Result, anyhow, bail};
use serde::Deserialize;

use crate::ffi::{
    llama_batch, llama_batch_free, llama_batch_init, llama_decode, llama_model_get_vocab,
    llama_pos, llama_sampler_accept, llama_sampler_sample, llama_token, llama_token_to_piece,
    llama_tokenize, llama_vocab, llama_vocab_is_eog,
};
use crate::load::LoadedLlm;

const MAX_RESPONSE_TOKENS: i32 = 1024;

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

/// Run one chat turn against `loaded`. `on_token` fires once per generated
/// token with the running count.
pub(crate) fn chat<F: FnMut(usize)>(
    loaded: &mut LoadedLlm,
    message: &str,
    mut on_token: F,
) -> Result<LlmReply> {
    // First chat continues the user turn the snapshot left half-open;
    // subsequent turns open their own `<|turn>user` block.
    let fragment = if loaded.had_first_chat {
        format!("<|turn>user\n{message}<turn|>\n<|turn>model\n")
    } else {
        format!("{message}<turn|>\n<|turn>model\n")
    };

    let tokens = tokenize(loaded, &fragment)?;
    if tokens.is_empty() {
        bail!("chat fragment tokenised to zero tokens");
    }
    decode(loaded, &tokens)?;
    let last_index: i32 = (tokens.len() - 1)
        .try_into()
        .context("sample index overflow")?;

    let reply = sample_reply(loaded, last_index, &mut on_token)?;

    let closer = tokenize(loaded, "<end_of_turn>\n")?;
    decode(loaded, &closer)?;

    loaded.had_first_chat = true;
    parse_reply(reply.trim())
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

fn vocab(loaded: &LoadedLlm) -> *const llama_vocab {
    unsafe { llama_model_get_vocab(loaded.model) }
}

/// Tokenize a chat continuation. `add_special=false` because the snapshot
/// already contains the BOS. `parse_special=true` so chat-template
/// markers (`<|turn>`, `<turn|>`) tokenize to their dedicated special-token
/// IDs (105 and 106 for Gemma 4) instead of multi-token text.
fn tokenize(loaded: &LoadedLlm, text: &str) -> Result<Vec<llama_token>> {
    let bytes = text.as_bytes();
    let text_len: i32 = bytes
        .len()
        .try_into()
        .context("tokenizer input overflows i32")?;
    let text_ptr = bytes.as_ptr() as *const c_char;
    let vocab = vocab(loaded);

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

/// Decode `tokens` into the KV cache, starting at `loaded.next_position`.
/// Only the last token's logits are kept (used by `sample_reply`).
fn decode(loaded: &mut LoadedLlm, tokens: &[llama_token]) -> Result<()> {
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
        let position = loaded
            .next_position
            .checked_add(offset as i32)
            .ok_or_else(|| anyhow!("KV position overflows i32"))?;
        unsafe { write_batch_slot(&mut batch, offset, token, position, offset == last) };
    }
    batch.n_tokens = count;

    let status = unsafe { llama_decode(loaded.context, batch) };
    unsafe { llama_batch_free(batch) };

    if status != 0 {
        bail!("llama_decode returned {status}");
    }
    loaded.next_position = loaded
        .next_position
        .checked_add(count)
        .ok_or_else(|| anyhow!("KV position overflows i32"))?;
    Ok(())
}

/// Sample tokens until end-of-generation or `MAX_RESPONSE_TOKENS`,
/// feeding each accepted token back into the KV cache. `on_token`
/// fires with the running count after each token is decoded.
fn sample_reply<F: FnMut(usize)>(
    loaded: &mut LoadedLlm,
    first_index: i32,
    on_token: &mut F,
) -> Result<String> {
    let mut reply_bytes: Vec<u8> = Vec::new();
    let mut sample_index = first_index;
    let mut count: usize = 0;

    for _ in 0..MAX_RESPONSE_TOKENS {
        let token = unsafe { llama_sampler_sample(loaded.sampler, loaded.context, sample_index) };
        unsafe { llama_sampler_accept(loaded.sampler, token) };
        if unsafe { llama_vocab_is_eog(vocab(loaded), token) } {
            break;
        }

        let piece = token_to_piece(loaded, token)?;
        reply_bytes.extend_from_slice(&piece);
        count += 1;
        on_token(count);

        decode(loaded, &[token])?;
        // After decoding a single token, logits sit in row 0.
        sample_index = 0;
    }
    Ok(String::from_utf8_lossy(&reply_bytes).into_owned())
}

fn token_to_piece(loaded: &LoadedLlm, token: llama_token) -> Result<Vec<u8>> {
    let vocab = vocab(loaded);
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
