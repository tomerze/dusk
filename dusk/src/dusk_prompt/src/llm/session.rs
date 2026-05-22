use dusk_program::anyhow::{Context, Result, anyhow};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

const MAX_RESPONSE_TOKENS: i32 = 256;

pub(super) struct Session {
    context: LlamaContext<'static>,
    sampler: LlamaSampler,
    next_position: i32,
    had_first_chat: bool,
    // SAFETY: `context` borrows from `model` and `backend` via the
    // `'static` lifetime-laundering in `Llm::load_session`. They must
    // outlive `context`, so they appear after it in the struct (Drop
    // runs in field declaration order — `context` is dismissed first).
    #[allow(dead_code)]
    model: Box<LlamaModel>,
    #[allow(dead_code)]
    backend: Box<LlamaBackend>,
}

// SAFETY: `Session` holds llama.cpp objects that are not auto-`Send`
// because they contain raw pointers, but llama.cpp itself holds no
// thread-affinity state — only concurrent access is unsound. The
// `Mutex<State>` in `Llm` serialises every access, making it safe to
// move between threads.
unsafe impl Send for Session {}

impl Session {
    pub(super) fn new(
        context: LlamaContext<'static>,
        model: Box<LlamaModel>,
        backend: Box<LlamaBackend>,
        starting_position: i32,
    ) -> Self {
        Self {
            context,
            sampler: LlamaSampler::chain_simple([LlamaSampler::greedy()]),
            next_position: starting_position,
            had_first_chat: false,
            model,
            backend,
        }
    }

    pub(super) fn chat<F: FnMut(usize)>(
        &mut self,
        message: String,
        on_token: &mut F,
    ) -> Result<String> {
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
        self.context.decode(&mut batch).context("decoding tokens")?;
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
        let mut reply = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut sample_idx = first_sample_index;
        let mut count: usize = 0;
        let mut batch = LlamaBatch::new(1, 1);
        for _ in 0..MAX_RESPONSE_TOKENS {
            let token = self.sampler.sample(&self.context, sample_idx);
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
            self.context
                .decode(&mut batch)
                .context("decoding sampled token")?;
            sample_idx = 0;
        }
        Ok(reply)
    }
}
