/*
 * dusk_warmup — produce an ik_llama.cpp KV-cache snapshot for a primed
 * system prompt. Invoked at build time by `dusk_llm/build.rs`.
 *
 * Usage:
 *
 *   dusk_warmup MODEL_PATH N_CTX KV_TYPE N_THREADS OUTPUT_PATH PROMPT
 *
 *     MODEL_PATH   absolute path to the GGUF model file
 *     N_CTX        context size (uint32, in tokens) — also used as the
 *                  batch size so the whole prompt fits in one decode
 *     KV_TYPE      ggml_type integer for K and V cache (e.g. 8 == Q8_0)
 *     N_THREADS    number of CPU threads for both prompt and batch eval
 *     OUTPUT_PATH  absolute path to write the snapshot to (overwritten)
 *     PROMPT       the literal text to prime the model with
 *
 * The snapshot is written in `llama_state_save_file` format:
 *   u32 magic ('ggsn') | u32 version | u32 n_tokens
 *     | llama_token[n_tokens] | raw KV state bytes
 * `dusk_llm` parses this header at runtime and feeds the trailing KV
 * bytes to `llama_state_set_data`.
 *
 * Tokenization matches the runtime in `chat.rs` exactly:
 *   add_special   = true   (BOS prepended)
 *   parse_special = true   (Gemma chat-template markers like
 *                          `<|turn>` / `<turn|>` get their dedicated
 *                          special token IDs, not literal text)
 *
 * Exits 0 on success; any non-zero exit prints a diagnostic on stderr.
 */

#include <errno.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ggml.h"
#include "llama.h"

#define EXIT_USAGE 2
#define EXIT_FAILURE_DUSK 1

static int parse_u32(const char *text, uint32_t *out) {
    char *end = NULL;
    errno = 0;
    unsigned long value = strtoul(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value > UINT32_MAX) {
        return -1;
    }
    *out = (uint32_t)value;
    return 0;
}

static int parse_i32(const char *text, int32_t *out) {
    char *end = NULL;
    errno = 0;
    long value = strtol(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value < INT32_MIN ||
        value > INT32_MAX) {
        return -1;
    }
    *out = (int32_t)value;
    return 0;
}

static void usage(void) {
    (void)fputs(
        "usage: dusk_warmup MODEL_PATH N_CTX KV_TYPE N_THREADS OUTPUT_PATH PROMPT\n",
        stderr);
}

int main(int argc, char **argv) {
    if (argc != 7) {
        usage();
        return EXIT_USAGE;
    }

    const char *model_path = argv[1];
    const char *output_path = argv[5];
    const char *prompt = argv[6];

    uint32_t n_ctx = 0;
    int32_t kv_type_int = 0;
    int32_t n_threads = 0;

    if (parse_u32(argv[2], &n_ctx) != 0) {
        (void)fprintf(stderr, "dusk_warmup: N_CTX %s is not a u32\n", argv[2]);
        return EXIT_USAGE;
    }
    if (n_ctx == 0) {
        (void)fputs("dusk_warmup: N_CTX must be > 0\n", stderr);
        return EXIT_USAGE;
    }
    if (parse_i32(argv[3], &kv_type_int) != 0) {
        (void)fprintf(stderr, "dusk_warmup: KV_TYPE %s is not an i32\n", argv[3]);
        return EXIT_USAGE;
    }
    if (parse_i32(argv[4], &n_threads) != 0 || n_threads <= 0) {
        (void)fprintf(stderr, "dusk_warmup: N_THREADS %s is not a positive i32\n",
                      argv[4]);
        return EXIT_USAGE;
    }

    int exit_code = EXIT_FAILURE_DUSK;
    struct llama_model *model = NULL;
    struct llama_context *context = NULL;
    llama_token *tokens = NULL;
    bool batch_initialised = false;
    struct llama_batch batch = {0};

    llama_backend_init();

    struct llama_model_params model_params = llama_model_default_params();
    // No GPU backend is compiled in, and ik_llama.cpp's default (-1) asks
    // for every layer to be offloaded to one.
    model_params.n_gpu_layers = 0;
    model = llama_model_load_from_file(model_path, model_params);
    if (model == NULL) {
        (void)fprintf(stderr,
                      "dusk_warmup: llama_model_load_from_file(\"%s\") returned NULL\n",
                      model_path);
        goto cleanup;
    }

    struct llama_context_params context_params = llama_context_default_params();
    context_params.n_ctx = n_ctx;
    // Match the runtime: the prompt is decoded in a single batch, so
    // n_batch / n_ubatch must accommodate the full token count.
    context_params.n_batch = n_ctx;
    context_params.n_ubatch = n_ctx;
    context_params.n_threads = n_threads;
    context_params.n_threads_batch = n_threads;
    context_params.type_k = (enum ggml_type)kv_type_int;
    context_params.type_v = (enum ggml_type)kv_type_int;

    context = llama_init_from_model(model, context_params);
    if (context == NULL) {
        (void)fputs("dusk_warmup: llama_init_from_model returned NULL\n", stderr);
        goto cleanup;
    }

    const struct llama_vocab *vocab = llama_model_get_vocab(model);
    const int32_t prompt_length = (int32_t)strlen(prompt);

    // Probe call: with a NULL output buffer the tokenizer returns the
    // negation of the required slot count (or INT32_MIN on overflow).
    int32_t probe = llama_vocab_tokenize(vocab, prompt, prompt_length,
                                         /*tokens=*/NULL, /*n_tokens_max=*/0,
                                         /*add_special=*/true,
                                         /*parse_special=*/true);
    if (probe == INT32_MIN) {
        (void)fputs("dusk_warmup: llama_vocab_tokenize probe overflowed i32\n", stderr);
        goto cleanup;
    }
    if (probe >= 0) {
        (void)fprintf(stderr,
                      "dusk_warmup: probe call returned %" PRId32
                      " (expected a negative slot-count); empty prompt?\n",
                      probe);
        goto cleanup;
    }
    const int32_t n_tokens = -probe;
    if ((uint32_t)n_tokens > n_ctx) {
        (void)fprintf(stderr,
                      "dusk_warmup: prompt tokenises to %" PRId32
                      " tokens but N_CTX is %" PRIu32 "\n",
                      n_tokens, n_ctx);
        goto cleanup;
    }

    tokens = (llama_token *)malloc((size_t)n_tokens * sizeof(llama_token));
    if (tokens == NULL) {
        (void)fputs("dusk_warmup: malloc for token buffer failed\n", stderr);
        goto cleanup;
    }
    int32_t written =
        llama_vocab_tokenize(vocab, prompt, prompt_length, tokens, n_tokens,
                             /*add_special=*/true, /*parse_special=*/true);
    if (written != n_tokens) {
        (void)fprintf(stderr,
                      "dusk_warmup: second llama_vocab_tokenize wrote %" PRId32
                      " tokens, expected %" PRId32 "\n",
                      written, n_tokens);
        goto cleanup;
    }

    batch = llama_batch_init(n_tokens, /*embd=*/0, /*n_seq_max=*/1);
    batch_initialised = true;
    for (int32_t i = 0; i < n_tokens; ++i) {
        batch.token[i] = tokens[i];
        batch.pos[i] = (llama_pos)i;
        batch.n_seq_id[i] = 1;
        batch.seq_id[i][0] = 0;
        // Logits are only needed on the very last token for the sampler
        // to read after restore; we don't actually sample here, but
        // matching the runtime's batch shape keeps the KV state
        // byte-identical to "decode then save" elsewhere.
        batch.logits[i] = (int8_t)(i == n_tokens - 1 ? 1 : 0);
    }
    batch.n_tokens = n_tokens;

    int32_t decode_status = llama_decode(context, batch);
    if (decode_status != 0) {
        (void)fprintf(stderr, "dusk_warmup: llama_decode returned %" PRId32 "\n",
                      decode_status);
        goto cleanup;
    }

    if (!llama_state_save_file(context, output_path, tokens, (size_t)n_tokens)) {
        (void)fprintf(stderr,
                      "dusk_warmup: llama_state_save_file(\"%s\") returned false\n",
                      output_path);
        goto cleanup;
    }

    exit_code = 0;

cleanup:
    if (batch_initialised) {
        llama_batch_free(batch);
    }
    free(tokens);
    if (context != NULL) {
        llama_free(context);
    }
    if (model != NULL) {
        llama_free_model(model);
    }
    return exit_code;
}
