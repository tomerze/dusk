# Ask Dusk

# Intro

Ask Dusk is a feature which runs a tiny local llm inside the dusk prompt with the purpose
of translating natural language into dusk shell commands.

This feature is mainly for ergonomics, non local LLMs are far more capable than the tiny
Gemma-4 e2b model we use. And yet there is something nice about a zero setup LLM with the
context pre-inserted that mostly gets the job done and is just a key stroke away.

Open the prompt press Ctrl + A (A stands for Ask or AI), and that's it you can just type naturally.

This page documents how that works under the hood.

## The pipeline at a glance

```
                ┌──────────────────────────────┐
program crate   │ #[sh_entry] proc-macro       │
(compile time)  │  writes JSON to              │
                │  target/.dusk_sh_entries/    │
                └────────────────┬─────────────┘
                                 │
                                 ▼
                ┌──────────────────────────────┐
dusk_llm        │ build.rs                     │
build.rs        │  ┌───────────────────────┐   │
(compile time)  │  │ cmake build           │   │
                │  │  vendor/llama.cpp →   │   │
                │  │  libllama.a + ggml*.a │   │
                │  └──────────┬────────────┘   │
                │             │                │
                │  ┌──────────▼────────────┐   │
                │  │ cc build_assets/      │   │
                │  │   warmup.c            │   │
                │  │  → dusk_warmup binary │   │
                │  └──────────┬────────────┘   │
                │             │                │
                │  ┌──────────▼────────────┐   │
                │  │ read sh_entries/*.json│   │
                │  │ compose system prompt │   │
                │  │ exec dusk_warmup →    │   │
                │  │   OUT_DIR/model.state │   │
                │  └──────────┬────────────┘   │
                └─────────────┼────────────────┘
                              │
                              ▼
                ┌──────────────────────────────┐
dusk_llm        │ include_bytes!(model.state)  │
runtime         │  → static SNAPSHOT: &[u8]    │
(in binary)     │ parse 12-byte header at      │
                │ load: 'ggsn' u32 | version   │
                │ u32 | n_tokens u32           │
                │  → SNAPSHOT_TOKENS           │
                └────────────────┬─────────────┘
                                 │ first Ask Dusk
                                 ▼
                ┌──────────────────────────────┐
Llm::ask        │ load_session()               │
                │  → llama_model_load_from_file│
                │  → llama_init_from_model     │
                │  → llama_state_set_data(tail)│
                │  → next_position = n_tokens  │
                │ decode user's chat turn      │
                │  from where warmup left off  │
                └──────────────────────────────┘
```

## The big picture: two crates, one C library

Ask Dusk lives in two workspace crates:

- **`dusk/src/dusk_llm/`** — owns the entire LLM stack: the vendored
  `llama.cpp` source, the FFI bindings, the build-time warm-up tool, the
  runtime `Llm` / `LlmReply` types. `dusk_llm/build.rs` is the only place
  in the workspace that compiles `llama.cpp`.
- **`dusk/src/dusk_prompt/`** — consumes `dusk_llm::Llm` like any other
  dependency. No `build.rs`, no awareness of llama.cpp, no snapshot file.

The `llama.cpp` C/C++ source is pinned as a git submodule at
`vendor/llama.cpp` (tag `b9282`). `dusk_llm/build.rs` invokes cmake to
build it as static libraries; the resulting `libllama.a`, `libggml.a`,
`libggml-cpu.a`, `libggml-base.a` are emitted as `cargo:rustc-link-lib=static=…`
directives, so the final `dusk` binary statically links everything.
No system `libllama.so` is consulted.

The Rust↔C surface is hand-written `unsafe extern "C"` declarations in
`dusk/src/dusk_llm/src/ffi.rs` — only the ~20 symbols we actually call,
plus `#[repr(C)]` mirrors of the few llama.cpp structs whose fields we
touch (`llama_model_params`, `llama_context_params`,
`llama_sampler_chain_params`, `llama_batch`). No `bindgen`, no
`llama-cpp-rs`-style wrapper crate. The trade-off: if `vendor/llama.cpp`
is bumped past `b9282`, those struct layouts and the
`LLAMA_SESSION_VERSION` constant must be re-verified against the new
`llama.h`. There's a runtime check on the version field of the saved
snapshot, so a layout drift surfaces as a clean error message rather
than silent UB.

## Stage 1 — Program crates publish their sh-entries

Every shell-invocable program crate has a `pub fn sh_entry() -> ShEntry { … }`
in its `client.rs`. The function is tagged with
`#[dusk_program_sh_proc::sh_entry]` (see `programs/sh/proc/`). At
macro-expansion time, the attribute does two things:

1. Emits `#[linkme::distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]`
   on the function so the runtime can iterate every registered entry via
   the linker-collected slice.
2. **Parses the function body**, extracts the three string-literal fields
   from the `EntryInfo { name: …, short_description: …, long_description: …
   }` initialiser, and writes a JSON file to
   `<target>/.dusk_sh_entries/<crate>__sh_entry.json`.

That JSON file is the **interchange format** between program crates and
`dusk_llm`. It lives outside the rlib, outside `cargo metadata`, outside
any feature gate — it's just a flat directory of small JSON files, one per
sh-entry.

The location: `OUT_DIR/../../..` (four levels up from the per-crate
build-script `OUT_DIR`) lands on the workspace `target/` directory; the
sh-entry attribute joins `.dusk_sh_entries/` onto that. Same derivation is
done at the read side, so both writer and reader agree without configuration.

## Stage 2 — `dusk_llm`'s build.rs assembles the prompt and warms the model

`dusk_llm` depends on `dusk_base` with `features = ["client"]` as a
regular dependency. `dusk_base` re-exports every program crate and
propagates `client` to each — so by the time cargo gets around to running
`dusk_llm/build.rs`, every program crate has compiled with `client` on,
which means every `#[sh_entry]` has fired and dropped its JSON.

> Aside: this dep is purely for build ordering. We tried it first in
> `[build-dependencies]`, but that puts `dusk_core` into the
> build-dependency graph, where resolver-v2 feature unification doesn't
> pick up the `nightly` feature on `embassy-executor` from `impls/nix`
> (which only lives in the target graph). `dusk_core`'s
> `#[embassy_executor::task]` macro then can't see the public
> `TaskReturnValue` trait, and `cargo check --workspace` breaks. Keeping
> `dusk_base` in the target graph dodges the issue.

`dusk_llm/build.rs` runs four phases in order:

### 2a. Build `vendor/llama.cpp`

`cmake::Config::new("vendor/llama.cpp")` with
`BUILD_SHARED_LIBS=OFF`, all tools/examples/server/app disabled,
`GGML_OPENMP=ON`, `LLAMA_CURL=OFF`. The static archives land in
`${OUT_DIR}/lib/`, and the linker directives below ship them into the
final binary:

```text
cargo:rustc-link-search=native=<OUT_DIR>/lib
cargo:rustc-link-lib=static=llama
cargo:rustc-link-lib=static=ggml
cargo:rustc-link-lib=static=ggml-cpu
cargo:rustc-link-lib=static=ggml-base
cargo:rustc-link-lib=dylib=stdc++   # libllama is C++
cargo:rustc-link-lib=dylib=pthread
cargo:rustc-link-lib=dylib=m
cargo:rustc-link-lib=dylib=dl
```

### 2b. Compile the C warm-up tool

`build_assets/warmup.c` is a small (~200 lines) C11 program that exists
solely to run inside `build.rs`. The build script invokes `cc` (or
`$CC`) directly to link it against the static archives produced in 2a:

```text
cc -std=c11 -O2 -o <OUT_DIR>/dusk_warmup build_assets/warmup.c \
    -I<OUT_DIR>/include -I<vendor>/ggml/include \
    -L<OUT_DIR>/lib \
    -Wl,--start-group -lllama -lggml -lggml-cpu -lggml-base -Wl,--end-group \
    -lstdc++ -lpthread -lm -ldl
```

The chicken-and-egg constraint that makes this awkward: a normal Cargo
build script cannot link itself to a library that the build script
itself just produced. We need the warm-up *as a separate executable*
that the build script then runs. `cc` directly is the smallest tool
that does the job. `cc::Build` from the `cc` crate only really targets
static-archive output, not executables — invoking `cc` by hand is
shorter and more honest.

### 2c. Compose the system prompt

`collect_sh_entries_info()` globs `target/.dusk_sh_entries/*.json` into
a `Vec<ShEntrySpec>`, sorted by name. `compose_system_prompt()` glues
a fixed preamble (dusk grammar, output-format rules, three concrete
Q/A examples) to the sorted entries.

### 2d. Run `dusk_warmup`

```text
dusk_warmup MODEL_PATH N_CTX KV_TYPE N_THREADS OUTPUT_PATH PROMPT
```

The build script picks the model path (`DUSK_MODEL_PATH` env or
`dusk/src/dusk_llm/models/gemma*.gguf`), `N_CTX=16384`, `KV_TYPE=8` (= `GGML_TYPE_Q8_0`),
and host parallelism for `N_THREADS`. The C program:

1. `llama_backend_init()`
2. `llama_model_load_from_file(path, default_params)`
3. `llama_init_from_model(model, params_with_n_ctx_n_batch_n_threads_set)`
4. `llama_tokenize(vocab, prompt, …, add_special=true, parse_special=true)`
5. Build a `llama_batch` with `n_seq_id=1`, `seq_id=[0]`,
   `logits[last]=1`, decode it.
6. `llama_state_save_file(ctx, OUTPUT_PATH, tokens, n_tokens)`

The two `_special=true` flags are important and match the runtime
exactly. `add_special` prepends BOS. `parse_special` makes Gemma's chat
template markers (`<start_of_turn>`, `<end_of_turn>`, etc.) tokenize to
their dedicated special token IDs rather than as multi-token literal
text. (This is **the** behaviour difference vs. the previous
`llama_cpp_2`-based pipeline; that crate's `str_to_token` hardcoded
`parse_special=false` and so the model saw chat markers as gibberish
text. We now feed the model what its chat template was trained on.)

The file `dusk_warmup` writes is a `llama_state_save_file` session
file. Layout:

```text
offset  bytes  field
0       4      u32 magic    = 0x6767736e ('ggsn', little-endian)
4       4      u32 version  = LLAMA_SESSION_VERSION (== 9 in llama.cpp b9282)
8       4      u32 n_tokens
12      4n     llama_token tokens[n_tokens]
12+4n   …      raw `llama_state_get_data` bytes (the KV cache snapshot)
```

The whole file lands at `${OUT_DIR}/model.state`. A typical snapshot
is single-digit megabytes (~10 MB for ~1000 prompt tokens, the cache
only holds the tokens the warm-up actually decoded — far less than the
16K-token context reservation). The binary inflates by that amount via
`include_bytes!`.

## Stage 3 — Runtime loads the embedded snapshot lazily

`dusk_llm/src/lib.rs` declares:

```rust
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));

const LLAMA_SESSION_MAGIC:   u32 = 0x6767736e; // 'ggsn'
const LLAMA_SESSION_VERSION: u32 = 9;
```

The `Llm` struct holds an `Arc<Mutex<State>>` where `State` is one of
`Unloaded | Loading | Ready | Failed`. `Llm::new()` is zero-arg and creates
the `Unloaded` state — no I/O, no model load.

On the first `Llm::ask`, `ensure_loaded` spawns a blocking task that runs
`load_session(SNAPSHOT)`:

1. **Parse the 12-byte snapshot header** — verify magic and version,
   pull `n_tokens` out, slice off the trailing raw KV bytes.
   Mismatched magic or version triggers a clean error (typically
   meaning `vendor/llama.cpp` was bumped without rebuilding).
2. `llama_backend_init()`
3. Locate the same GGUF the build.rs used.
4. `llama_model_load_from_file(path, llama_model_default_params())`.
5. Build a context with the same `CONTEXT_TOKENS` (= 16384) and
   `KV_CACHE_TYPE` (= `GGML_TYPE_Q8_0`); thread count is runtime-resolved
   from `DUSK_LLM_THREADS_COUNT` env var or `available_parallelism()`.
6. **`unsafe { llama_state_set_data(ctx, raw_tail.as_ptr(), raw_tail.len()) }`**
   — splat the warm cache back. This is the symmetric inverse of
   `llama_state_get_data` (which `llama_state_save_file` writes into
   the file in step 2d above).
7. Set `next_position = n_tokens` so subsequent decodes pick up
   exactly where build.rs's tokenisation stopped.

Subsequent chat turns just decode the user's message starting at
`next_position` and sample until end-of-turn. No more model loads, no
disk I/O, no system prompt — that's all already in the cache.

Sampling is a three-stage chain (`llama_sampler_chain_init` +
top-K=20 + temperature=0.6 + dist with `LLAMA_DEFAULT_SEED`): the top-K
stage narrows the candidate set, temperature rescales logits, and the
dist stage samples from the resulting distribution with a fresh random
seed picked at init.

### Lazy load timing

The model is **only loaded on the first Ask Dusk invocation in a
session.** Starting the shell, running ordinary commands, even
toggling into Ask Dusk mode without sending a query — none of those
touch the model. The first `<Enter>` after typing a natural-language
prompt is what triggers `load_session`.

The user sees a spinner during the load. With the snapshot already
in memory (it shipped inside the binary), load time is dominated by
mmap'ing the GGUF, building the llama.cpp context, and calling
`llama_state_set_data`. On a warm filesystem cache this is fast — well
under a second on typical hardware. On a cold cache, the wait is
roughly however long it takes to read the GGUF file off disk.

Once `Ready`, the model stays in memory for the rest of the shell
session.

## Resource usage

Measured on the current build (numbers will drift as the system
prompt or program list changes; treat them as ballparks).

| Where                                          | Size            |
|------------------------------------------------|-----------------|
| GGUF on disk (Gemma 4 E2B Q4_K_M)              | **~2.9 GB**     |
| KV-cache snapshot embedded in binary           | **~10 MB**      |
| `dusk` binary (debug build, snapshot included) | ~270 MB         |
| Snapshot token count                           | ~1000 tokens    |
| Context capacity (`CONTEXT_TOKENS`)            | 16384 tokens    |

**Working RAM once the model is loaded:** roughly **3–4 GB**,
dominated by model weights (~2.9 GB resident after llama.cpp's mmap
touches them) plus the Q8-quantised KV cache for the 16K-token context
(a few hundred MB) plus llama.cpp's internal scratch buffers. The
embedded snapshot itself is negligible — it's bytes the runtime
copies into the KV cache and then forgets about.

**Working RAM before the first Ask Dusk in a session:** essentially
just the binary's own footprint. The model isn't touched until you
ask.

## Why this design — the rebuild story

The whole point of moving warm-up to compile time is to make staleness
**impossible by construction**. The pieces:

- **`include_bytes!`** makes the snapshot bytes part of the `dusk_llm`
  rlib. If the bytes change, `dusk_llm` recompiles; if `dusk_llm`
  recompiles, `dusk_prompt` relinks; the final binary picks up the
  new snapshot.
- **`build.rs` emits `cargo:rerun-if-changed` directives** for every
  input the snapshot depends on:
  - `target/.dusk_sh_entries/` (the directory) and each JSON inside —
    edit any sh-entry's `short_description` and the snapshot regenerates.
  - The workspace `Cargo.lock` (absolute path) — add or remove a program
    crate and the snapshot regenerates.
  - `vendor/llama.cpp/CMakeLists.txt` and `vendor/llama.cpp/include/llama.h` —
    bump the submodule and llama.cpp rebuilds and the warm-up reruns.
  - `build_assets/warmup.c` — change the warm-up tool and it recompiles.
  - The model file path itself — swap GGUF and the snapshot regenerates.
- **`dusk_llm` depends on `dusk_base` with `client`**, which transitively
  pulls in every program crate. Cargo schedules `dusk_llm/build.rs`
  after every program crate is compiled, so the JSON files are
  guaranteed present when `build.rs` reads them.

## Tunables

- `DUSK_LLM_THREADS_COUNT` (runtime) — override decode thread count.
  Falls back to `std::thread::available_parallelism()`, then 1.
- `DUSK_MODEL_PATH` (build time) — override the GGUF model location.
  Falls back to `dusk/src/dusk_llm/models/gemma*.gguf`.

## Knobs that *must* match between build and runtime

- **`CONTEXT_TOKENS`** — the cache buffer is sized for `n_ctx`. Loading a
  snapshot built at one `n_ctx` into a context with a smaller `n_ctx` is undefined.
- **`KV_CACHE_TYPE`** — bytes are formatted per the quantisation.
- **`vendor/llama.cpp` revision** — the snapshot file format is versioned
  by `LLAMA_SESSION_VERSION` (currently 9 in tag `b9282`). The runtime
  rejects snapshots with a different version, so a stale build surfaces
  as a clean error.
- **`parse_special` / `add_special` tokenizer flags** — the warm-up
  tokenises with both `true`. The runtime in `session.rs` must do the
  same when extending the chat — otherwise turn boundaries don't line
  up with what's in the KV cache.

The first two constants live in **two** places that must be kept in
sync:

- `dusk/src/dusk_llm/build.rs` — `const CONTEXT_TOKENS: u32 = 16_384;`
  and `const KV_CACHE_TYPE: i32 = 8;` (passed to `dusk_warmup` as
  command-line arguments).
- `dusk/src/dusk_llm/src/lib.rs` — `CONTEXT_TOKENS: u32` and
  `KV_CACHE_TYPE: c_int`, used by `build_context`.

These are duplicated rather than `include!`'d because `build.rs`
predates `src/lib.rs` in the compile order. If you change one,
change the other.

## Knobs that may differ

- **Thread count.** Build-time uses host parallelism; runtime uses
  `DUSK_LLM_THREADS_COUNT` or runtime parallelism. The bytes
  `llama_state_save_file` writes are deterministic given the model
  and input — thread count only affects how fast the decode runs.
- **Model file location.** The build can point at any GGUF; the runtime
  finds its own. They need to be **the same model**, but not at the same
  path. (Cross-machine reproducibility: the build expects the model in
  `dusk/src/dusk_llm/models/` by default.)

## Caveats

- **No unloading once `Ready`.** The state machine is one-way:
  `Unloaded → Loading → Ready → Failed`. Once a session loads, the
  ~3 GB working set sticks around for the rest of the shell's
  lifetime. There is no API to evict it, no "I'm done with Ask Dusk,
  free the model" button. If you opened the shell, asked one
  question, and don't plan to ask again — the model is still in RAM.
  Exit the shell to reclaim.
- **No context reset within a session.** Every chat turn extends the
  KV cache. There is no `/clear`, no fresh-conversation command, no
  way to truncate `next_position` back to its post-snapshot value.
  Each turn sees every prior turn. This is fine for a few Q/As but the
  16K-token context will eventually fill — at which point decode
  fails. Workaround: exit and restart the shell to get a fresh
  context (the snapshot reloads from `include_bytes!`, the model
  reloads from the GGUF).
- **`Failed` is sticky.** If `load_session` errors (missing GGUF,
  llama.cpp init failure, snapshot magic/version mismatch), the state
  transitions to `Failed(String)` and every subsequent `Llm::ask`
  returns the saved error immediately. No retry, no recovery within
  the session.
- **The build assumes the host has the GGUF.** `dusk_llm/build.rs`
  needs to actually load the model to warm it up — there is no way
  to build `dusk_llm` without a GGUF on disk. Set `DUSK_MODEL_PATH`
  to point at any compatible Gemma file, or drop one into
  `dusk/src/dusk_llm/models/`.
- **The build assumes the host has a C compiler and cmake.**
  `dusk_llm/build.rs` shells out to `cmake` for llama.cpp and `cc`
  (or `$CC`) for the warm-up binary. On a fresh box: `apt install
  build-essential cmake` (or distro equivalent).
- **First-time build is slow.** cmake compiles all of llama.cpp +
  ggml from scratch — a couple of minutes on a typical workstation.
  Subsequent builds are cached. `cargo clean` triggers another full
  llama.cpp rebuild.
- **`vendor/llama.cpp` is a submodule.** After cloning, run
  `git submodule update --init vendor/llama.cpp`. Bumping the
  submodule past `b9282` requires re-verifying that the
  `#[repr(C)]` structs in `dusk_llm/src/ffi.rs` still match
  `llama.h` and that `LLAMA_SESSION_VERSION` is still `9`.
