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

- **`dusk/src/dusk_llm/`** owns the whole LLM stack: the vendored
  `ik_llama.cpp` source, the FFI bindings (`ffi.rs`), the build-time warm-up
  tool, and the runtime `Chat` / `LlmReply` types. Its `build.rs` is the only
  place in the workspace that compiles `ik_llama.cpp`.
- **`dusk/src/dusk_prompt/`** consumes `dusk_llm::Chat` like any other
  dependency — no `build.rs`, no awareness of ik_llama.cpp.

[`ik_llama.cpp`](https://github.com/ikawrakow/ik_llama.cpp) is pinned as a git
submodule at `vendor/ik_llama.cpp`. It is a fork of llama.cpp whose CPU
inference kernels are faster, which is the whole reason we are on it — Ask Dusk
never runs on a GPU. `build.rs` builds it as static archives (`libllama.a`,
`libggml.a`) and emits `cargo:rustc-link-lib=static=…` directives, so everything
links statically into the final binary. The Rust↔C surface is hand-written
`unsafe extern "C"` declarations in `ffi.rs` — only the ~25 symbols we call, plus
`#[repr(C)]` mirrors of the few structs whose fields we touch. No `bindgen`, no
wrapper crate.

The archives are read out of the CMake build tree rather than the install
prefix, because ik_llama.cpp installs its libraries only when
`BUILD_SHARED_LIBS` is on.

The trade-off: bumping `vendor/ik_llama.cpp` past the currently pinned revision
means re-verifying those struct layouts and the `LLAMA_SESSION_VERSION` constant
against the new `llama.h`. A runtime version check on the snapshot turns
session-format drift into a clean error instead of silent UB; the `#[repr(C)]`
struct mirrors have no such guard and must be re-verified by hand — `sizeof` and
`offsetof` from a C program against the header, compared with `size_of` and
`offset_of!` from the Rust mirrors.

## Pipeline at a glance

```
program crate    #[sh_entry] proc-macro writes JSON
(compile time)   → target/.dusk_sh_entries/<crate>__sh_entry.json
                                 │
                                 ▼
dusk_llm         build.rs:
build.rs           1. fetch + verify model.json's gguf (first build only)
(compile time)     2. cmake build vendor/ik_llama.cpp → static archives
                   3. cc warmup/main.c            → dusk_warmup binary
                   4. read sh_entries + prompts/system.md → system prompt
                   5. run dusk_warmup             → OUT_DIR/dusk_llm_kv_snapshot
                                 │
                                 ▼
dusk_llm         binary embeds two blobs:
(in binary)        • include_bytes! → SNAPSHOT (the warm KV cache)
                   • .incbin        → the GGUF model, in an ELF section
                                 │
                                 ▼ first Ask Dusk in a session
dusk_prompt      load_from_self_exe_section(section) → EmbeddedGgufFile
                 (ELF section → memfd)
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

1. **Fetch and verify the model** — `model.json` holds the `url` the GGUF is
   fetched from, the `file` it is saved as, the SHA-256 its bytes must have, and
   the `directory` it lands in (relative to the crate root). The URL must
   address one immutable set of bytes — pin a commit, a digest or a version,
   never a branch — so the host cannot start serving different weights. Nothing
   in the manifest is specific to a particular host; the current URL happens to
   be a Hugging Face commit. If `directory/file` is absent it is downloaded
   (2.89 GiB) with [`fast-down`](https://crates.io/crates/fast-down) over six
   connections at once, because a single one is throttled well below a fast
   link — 15 MiB/s against 28 MiB/s on a 280 Mbit line. It lands via a
   `.partial` file, so an interrupted download is never mistaken for a complete
   model. Whether it was just downloaded or was already there, its SHA-256 is
   checked. A mismatch fails the build and removes nothing — the GGUF is
   embedded verbatim into the binary, so building on unknown bytes is never the
   lesser evil; delete the file and build again to re-fetch it. This is the only
   step that needs the network, and only on the first build.

   `directory` and `file` are also the only place the model's location is
   written down: `build.rs` passes the resulting path to `rustc` as
   `DUSK_LLM_MODEL_PATH`, which is what `src/load.rs` `.incbin`s in stage 3.
2. **Build `vendor/ik_llama.cpp`** via `cmake` — static libs only, tests/
   examples/server off, `GGML_OPENMP=ON`, and the fork's own CPU kernels named
   explicitly (`GGML_IQK_MUL_MAT`, `GGML_IQK_FLASH_ATTENTION`,
   `GGML_IQK_FA_ALL_QUANTS`).

    `GGML_NATIVE` is off, so the build is not pinned to the machine that ran
    it. It cannot simply be off, though: ik_llama.cpp then leaves every
    instruction-set option off too, which compiles no SIMD — and the IQK
    kernels are behind `#if defined __AVX2__`, so they would silently
    disappear. x86-64 therefore gets an explicit floor of AVX2 + FMA + F16C
    (Haswell and newer); aarch64 needs none, as ggml compiles NEON there
    unconditionally. Anything the target asks for above that floor is mapped
    from `CARGO_CFG_TARGET_FEATURE` to `GGML_*` defines on top.

    `libstdc++`, `libgcc`, and OpenMP are linked statically; only
   `pthread`/`m`/`dl` stay dynamic.
3. **Compile the warm-up tool** — `warmup/main.c` (~220 lines of C11) is linked
   against the archives from step 2 into an `OUT_DIR/dusk_warmup` executable. It
   has to be a separate binary because a build script can't link itself against
   a library it just produced.
4. **Compose the system prompt** — glob `target/.dusk_sh_entries/*.json` (sorted
   by name) and splice them into the `{{PROGRAMS}}` placeholder of
   `prompts/system.md`. That template holds the persona, the dusk grammar, the
   strict JSON output contract, and few-shot examples in Gemma turn structure.
5. **Run `dusk_warmup`** with `model n_ctx kv_type n_threads out_path prompt`,
   where the prompt is `<|turn>user\n{system_prompt}` — a deliberately
   *half-open* user turn. It loads the GGUF, tokenizes the prompt
   (`add_special=true`, `parse_special=true` so Gemma's chat markers become
   their dedicated token IDs), decodes it into the KV cache, and writes the
   cache out with `llama_state_save_file`.

The output, `OUT_DIR/dusk_llm_kv_snapshot`, is an ik_llama.cpp session file:

```text
offset  bytes  field
0       4      u32 magic   = 0x6767736e ('ggsn')
4       4      u32 version = LLAMA_SESSION_VERSION (10 in ik_llama.cpp)
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
which opens `/proc/self/exe`, finds the section, and `sendfile`s its bytes into
a `memfd` — an anonymous in-memory file, handed back as an `EmbeddedGgufFile`.
The copy is there because ik_llama.cpp loads a model only from a path it opens
itself (it has no counterpart to stock llama.cpp's
`llama_model_load_from_file_ptr`), and a `memfd` is the only path-addressable
file whose first byte can be the GGUF's first byte. ik_llama.cpp then `mmap`s
it, so the model stays resident once. It passes that to `Chat::new(embedded)`,
which loads eagerly and returns a ready `Chat` (`Arc<Mutex<LlmState>>`).
`Chat::new`:

1. Installs the ik_llama.cpp log hook and `llama_backend_init()`.
2. Parses the 12-byte snapshot header — verifies magic and version, pulls
   `n_tokens`, slices off the raw KV tail. A version mismatch is a clean error.
3. Loads the embedded GGUF from `/proc/self/fd/<memfd>`.
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
- **`vendor/ik_llama.cpp` revision** — the snapshot format is versioned by
  `LLAMA_SESSION_VERSION`; the runtime rejects a mismatch with a clean error.
  The snapshot's build-time cache key folds in that revision (the submodule's
  checked-out commit) and the `libllama.a` built from it, so moving the
  submodule regenerates the snapshot instead of reusing one written by a
  different library.
- **`n_batch`** — restoring the snapshot replays the output id the warm-up
  asked for logits on, and an id beyond the context's batch size is rejected
  (`invalid output id, N does not fit in batch size of M`). The warm-up decodes
  its prompt in one batch of `CONTEXT_TOKENS`, so the runtime context sets
  `n_batch` to the same value. It costs nothing: `n_ubatch`, left at its
  default, is what sizes the compute buffer.
- **`add_special` / `parse_special` flags** — warm-up tokenizes with both
  `true`; `Chat::chat` uses `parse_special=true` so turn boundaries line up with
  the cache.
