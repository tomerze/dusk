# Ask Dusk

Ask Dusk runs a tiny local LLM inside the dusk prompt to translate natural
language into dusk shell commands. It is an ergonomic convenience — a hosted
LLM is far more capable than the Gemma-4 E2B model we embed — but a zero-setup
model, with all its context pre-baked and a keystroke away, mostly gets the job
done.

Open the prompt, press **Ctrl + A** (A for Ask), and type naturally.

This page documents how it works under the hood.

## The shape of it

Two workspace crates and one C library:

- **`dusk/src/dusk_llm/`** owns the whole LLM stack: the vendored `llama.cpp`
  source, the FFI bindings (`ffi.rs`), the build-time warm-up tool, and the
  runtime `Chat` / `LlmReply` types. Its `build.rs` is the only place in the
  workspace that compiles `llama.cpp`.
- **`dusk/src/dusk_prompt/`** consumes `dusk_llm::Chat` like any other
  dependency — no `build.rs`, no awareness of llama.cpp.

`llama.cpp` is pinned as a git submodule at `vendor/llama.cpp`.
`build.rs` builds it as static archives (`libllama.a`, `libggml*.a`) and emits
`cargo:rustc-link-lib=static=…` directives, so everything links statically into
the final binary. The Rust↔C surface is hand-written `unsafe extern "C"`
declarations in `ffi.rs` — only the ~30 symbols we call, plus `#[repr(C)]`
mirrors of the few structs whose fields we touch. No `bindgen`, no wrapper crate.

The trade-off: bumping `vendor/llama.cpp` past the currently pinned version means re-verifying those
struct layouts and the `LLAMA_SESSION_VERSION` constant against the new
`llama.h`. A runtime version check on the snapshot turns session-format drift
into a clean error instead of silent UB; the `#[repr(C)]` struct mirrors have no
such guard and must be re-verified by hand.

## Pipeline at a glance

```
program crate    #[sh_entry] proc-macro writes JSON
(compile time)   → target/.dusk_sh_entries/<crate>__sh_entry.json
                                 │
                                 ▼
dusk_llm         build.rs:
build.rs           1. cmake build vendor/llama.cpp → static archives
(compile time)     2. cc warmup/main.c            → dusk_warmup binary
                   3. read sh_entries + prompts/system.md → system prompt
                   4. run dusk_warmup             → OUT_DIR/dusk_llm_kv_snapshot
                                 │
                                 ▼
dusk_llm         binary embeds two blobs:
(in binary)        • include_bytes! → SNAPSHOT (the warm KV cache)
                   • .incbin        → the GGUF model, in an ELF section
                                 │
                                 ▼ first Ask Dusk in a session
dusk_prompt      load_from_self_exe_section(section) → EmbeddedGgufFile
                 → Chat::new(embedded): build context,
                   llama_state_set_data(SNAPSHOT)
                                 │
                                 ▼ each turn
Chat::chat       decode the user's turn from where warm-up stopped
```

## Stage 1 — Programs publish their sh-entries

Every shell-invocable program crate has a `pub fn sh_entry() -> ShEntry` in its
`client.rs`, tagged with `#[dusk_program_sh_proc::sh_entry]`. At macro-expansion
the attribute:

1. Registers the function in the linker-collected
   `dusk_program_sh::entry::SH_ENTRIES` distributed slice (how the runtime
   iterates entries).
2. Parses the function body, extracts `name` / `short_description` /
   `long_description`, and writes them to
   `target/.dusk_sh_entries/<crate>__sh_entry.json`.

Those JSON files are the interchange format between program crates and
`dusk_llm` — a flat directory of small files outside the rlib, outside
`cargo metadata`, outside any feature gate. The directory lives at the workspace
`target/`; writer and reader derive the same path from `OUT_DIR`, so they agree
without configuration.

## Stage 2 — build.rs assembles the prompt and warms the model

`dusk_llm` depends on `dusk_base` with `features = ["client"]` as a regular (not
build-) dependency. `dusk_base` re-exports every program crate and propagates
`client`, so by the time `dusk_llm/build.rs` runs, every `#[sh_entry]` has fired
and dropped its JSON. (It must be a normal dependency, not a build-dependency —
the latter drags `dusk_core` into the build graph where feature unification
breaks an embassy macro.)

`build.rs` runs in order:

1. **Build `vendor/llama.cpp`** via `cmake` — static libs only, tools/examples/
   server off, `GGML_OPENMP=ON`. Host CPU features (`avx2`, `fma`, …) are
   mapped from `CARGO_CFG_TARGET_FEATURE` to `GGML_*` defines. `libstdc++`,
   `libgcc`, and OpenMP are linked statically too; only `pthread`/`m`/`dl`
   stay dynamic.
2. **Compile the warm-up tool** — `warmup/main.c` (~220 lines of C11) is linked
   against the archives from step 1 into an `OUT_DIR/dusk_warmup` executable. It
   has to be a separate binary because a build script can't link itself against
   a library it just produced.
3. **Compose the system prompt** — glob `target/.dusk_sh_entries/*.json` (sorted
   by name) and splice them into the `{{PROGRAMS}}` placeholder of
   `prompts/system.md`. That template holds the persona, the dusk grammar, the
   strict JSON output contract, and few-shot examples in Gemma turn structure.
4. **Run `dusk_warmup`** with `model n_ctx kv_type n_threads out_path prompt`,
   where the prompt is `<|turn>user\n{system_prompt}` — a deliberately
   *half-open* user turn. It loads the GGUF, tokenizes the prompt
   (`add_special=true`, `parse_special=true` so Gemma's chat markers become
   their dedicated token IDs), decodes it into the KV cache, and writes the
   cache out with `llama_state_save_file`.

The output, `OUT_DIR/dusk_llm_kv_snapshot`, is a llama.cpp session file:

```text
offset  bytes  field
0       4      u32 magic   = 0x6767736e ('ggsn')
4       4      u32 version = LLAMA_SESSION_VERSION (9 in b9282)
8       4      u32 n_tokens
12      4n     llama_token tokens[n_tokens]
12+4n   …      raw llama_state_get_data bytes (the KV cache)
```

It is roughly 30 MB — the cache holds only the tokens the warm-up
decoded, far below the 16K-token context reservation.

This effectively allows us to **skip the prefill stage at runtime**!

## Stage 3 — Runtime loads the embedded snapshot lazily

The binary embeds **two** blobs:

- The KV snapshot, via `include_bytes!(concat!(env!("OUT_DIR"),
  "/dusk_llm_kv_snapshot"))`.
- The **GGUF model itself**, via `.incbin` in a non-`ALLOC`, `SHF_GNU_RETAIN`
  section. No PHDR maps it, nothing references the
  symbol, and the retain flag survives `--gc-sections`. The model never touches
  the filesystem as a separate file — it ships *inside* the binary, which is why
  the binary is multi-gigabyte.

`dusk_llm` exposes the pieces but owns no laziness — that lives in
`dusk_prompt`. On the **first** Ask Dusk of a session, `dusk_prompt` (inside a
`spawn_blocking`) calls
`load_from_self_exe_section(GEMMA4E2B_EMBEDDED_GGUF_SECTION)`,
which opens `/proc/self/exe`, finds the section, and hands back an
`EmbeddedGgufFile` — a glibc `fopencookie` stream over it (`use_mmap=false`, so
reads land in our callbacks). It passes that to `Chat::new(embedded)`, which
loads eagerly and returns a ready `Chat` (`Arc<Mutex<LlmState>>`). `Chat::new`:

1. Installs the llama.cpp log hook and `llama_backend_init()`.
2. Parses the 12-byte snapshot header — verifies magic and version, pulls
   `n_tokens`, slices off the raw KV tail. A version mismatch is a clean error.
3. Loads the embedded GGUF over the cookie stream.
4. Builds a context with `CONTEXT_TOKENS` (16384) and `KV_CACHE_TYPE` (Q8_0);
   thread count from `DUSK_LLM_THREADS_COUNT` or `available_parallelism()`.
5. `llama_state_set_data(ctx, raw_tail)` — splats the warm cache back in (the
   inverse of the `llama_state_get_data` that built the snapshot).
6. Sets `next_position = n_tokens`, so decoding resumes exactly where warm-up
   stopped.

Each `Chat::chat` turn then decodes the user's message from `next_position` and samples
until end-of-turn or `MAX_RESPONSE_TOKENS` (1024). The first turn continues the
half-open user turn from the snapshot; later turns open their own `<|turn>user`
block. No further model loads, no disk I/O, no re-feeding the system prompt — it
is already in the cache. The reply is parsed as the first JSON object in the
output into `LlmReply { explanation, command }`.

## Tunables

- `DUSK_LLM_THREADS_COUNT` (runtime) — decode thread count. Falls back to
  `available_parallelism()`, then 1. Thread count only affects speed; the
  snapshot bytes are deterministic given model and input.

## Knobs that must match between build and runtime

These are duplicated in `build.rs` and `src/chat.rs` (build.rs predates the
source in compile order, so they can't share a constant). Change one, change the
other:

- **`CONTEXT_TOKENS`** — the cache is sized for `n_ctx`; loading a snapshot into
  a smaller context is undefined.
- **`KV_CACHE_TYPE`** — bytes are formatted per the quantisation (Q8_0).
- **`vendor/llama.cpp` revision** — the snapshot format is versioned by
  `LLAMA_SESSION_VERSION`; the runtime rejects a mismatch with a clean error.
- **`add_special` / `parse_special` flags** — warm-up tokenizes with both
  `true`; `Chat::chat` uses `parse_special=true` so turn boundaries line up with
  the cache.
