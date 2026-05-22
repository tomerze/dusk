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
dusk_prompt     │ build.rs reads every JSON,   │
build.rs        │  composes system prompt,     │
(compile time)  │  loads GGUF, decodes prompt, │
                │  copy_state_data →           │
                │   OUT_DIR/model.state        │
                │   OUT_DIR/snapshot_meta.rs   │
                └────────────────┬─────────────┘
                                 │
                                 ▼
                ┌──────────────────────────────┐
dusk_prompt     │ include_bytes!(model.state)  │
runtime         │ include!(snapshot_meta.rs)   │
(in binary)     │  → static SNAPSHOT: &[u8]    │
                │  → SNAPSHOT_TOKENS: i32      │
                └────────────────┬─────────────┘
                                 │ first Ask Dusk
                                 ▼
                ┌──────────────────────────────┐
Llm::ask        │ Session::load                │
                │  → load model GGUF           │
                │  → build context             │
                │  → set_state_data(SNAPSHOT)  │
                │  → next_position =           │
                │     SNAPSHOT_TOKENS          │
                │ decode user's chat turn      │
                │  from where warmup left off  │
                └──────────────────────────────┘
```

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
`dusk_prompt`. It lives outside the rlib, outside `cargo metadata`, outside
any feature gate — it's just a flat directory of small JSON files, one per
sh-entry.

The location: `OUT_DIR/../../..` (four levels up from the per-crate
build-script `OUT_DIR`) lands on the workspace `target/` directory; the
sh-entry attribute joins `.dusk_sh_entries/` onto that. Same derivation is
done at the read side, so both writer and reader agree without configuration.

## Stage 2 — `dusk_prompt`'s build.rs assembles the prompt and warms the model

`dusk_prompt` depends on `dusk_base` with `features = ["client"]`. `dusk_base`
re-exports every program crate and propagates `client` to each — so by the
time cargo gets around to running `dusk_prompt/build.rs`, every program
crate has compiled with `client` on, which means every `#[sh_entry]` has
fired and dropped its JSON.

`dusk_prompt/build.rs` then:

1. **Globs `target/.dusk_sh_entries/*.json`** into a `Vec<ShEntrySpec>`,
   sorted by name. (See `collect_sh_entries_info`.)
2. **Composes the system prompt** by gluing a fixed preamble (dusk grammar,
   output-format rules, three concrete Q/A examples) to the sorted sh-entry
   descriptions. The exact text lives in `compose_system_prompt`.
3. **Loads the GGUF model** — `DUSK_MODEL_PATH` env var if set, otherwise
   `~/git/model/gemma*.gguf`.
4. **Builds a llama.cpp context** using the same `CONTEXT_TOKENS` and
   `KV_CACHE_TYPE` constants the runtime will use (single source of truth:
   `src/llm/params.rs`). Thread count is `std::thread::available_parallelism()`
   — thread count does not affect snapshot bytes, only decode speed.
5. **Tokenises and decodes the system prompt** in one batch.
6. **Calls `copy_state_data`** to serialise the KV cache, writes the raw
   bytes to `OUT_DIR/model.state`, and writes
   `pub(super) const SNAPSHOT_TOKENS: i32 = N;` to `OUT_DIR/snapshot_meta.rs`.

A typical snapshot is single-digit megabytes (the cache only holds the
~800 tokens the warm-up actually decoded — far less than the 4K-token
reservation). The binary inflates by that amount via `include_bytes!`.

## Stage 3 — Runtime loads the embedded snapshot lazily

`src/llm/mod.rs` declares:

```rust
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));
include!(concat!(env!("OUT_DIR"), "/snapshot_meta.rs"));
// brings SNAPSHOT_TOKENS into scope
```

The `Llm` struct holds an `Arc<Mutex<State>>` where `State` is one of
`Unloaded | Loading | Ready | Failed`. `Llm::new()` is zero-arg and creates
the `Unloaded` state — no I/O, no model load, no cache file.

On the first `Llm::ask`, `ensure_loaded` spawns a blocking task that runs
`Session::load`:

1. Locate the same GGUF the build.rs used.
2. Build a context with the same `CONTEXT_TOKENS` / `KV_CACHE_TYPE` / threads
   (thread count is runtime-resolved from `DUSK_LLM_THREADS_COUNT` env or
   `available_parallelism()`).
3. **`unsafe { ctx.set_state_data(SNAPSHOT) }`** — splat the warm cache
   back. This is the symmetric inverse of `copy_state_data`.
4. Set `next_position = SNAPSHOT_TOKENS` so subsequent decodes pick up
   exactly where build.rs's tokenisation stopped.

`set_state_data` is the part of the upstream API that admits "help wanted:
not entirely sure what the safety requirements are." The contract we trust:
the snapshot was produced against the same context parameters in the same
build, so the round-trip is well-defined.

Subsequent chat turns just decode the user's message starting at
`next_position` and sample until end-of-turn. No more model loads, no
disk I/O, no system prompt — that's all already in the cache.

### Lazy load timing

The model is **only loaded on the first Ask Dusk invocation in a
session.** Starting the shell, running ordinary commands, even
toggling into Ask Dusk mode without sending a query — none of those
touch the model. The first `<Enter>` after typing a natural-language
prompt is what triggers `Session::load`.

The user sees a spinner during the load. With the snapshot already
in memory (it shipped inside the binary), load time is dominated by
mmap'ing the GGUF, building the llama.cpp context, and calling
`set_state_data`. On a warm filesystem cache this is fast — well
under a second on typical hardware. On a cold cache, the wait is
roughly however long it takes to read the GGUF file off disk.

Once `Ready`, the model stays in memory for the rest of the shell
session.

## Resource usage

Measured on the current build (numbers will drift as the system
prompt or program list changes; treat them as ballparks).

| Where                                   | Size                          |
|-----------------------------------------|-------------------------------|
| GGUF on disk (Gemma 4 E2B Q4_K_M)       | **~2.9 GB**                   |
| KV-cache snapshot embedded in binary    | **~10 MB**                    |
| `dusk` binary (debug build, snapshot included) | ~265 MB                |
| Snapshot token count                    | ~1000 tokens                  |
| Context capacity (`CONTEXT_TOKENS`)     | 4096 tokens                   |

**Working RAM once the model is loaded:** roughly **3–4 GB**,
dominated by model weights (~2.9 GB resident after llama.cpp's mmap
touches them) plus the Q8-quantised KV cache for the 4K-token context
(a few hundred MB) plus llama.cpp's internal scratch buffers. The
embedded snapshot itself is negligible — it's bytes the runtime
copies into the KV cache and then forgets about.

**Working RAM before the first Ask Dusk in a session:** essentially
just the binary's own footprint. The model isn't touched until you
ask.

## Why this design — the rebuild story

The whole point of moving warm-up to compile time is to make staleness
**impossible by construction**. The pieces:

- **`include_bytes!`** makes the snapshot bytes part of the rlib. If the
  bytes change, `dusk_prompt` recompiles; if `dusk_prompt` recompiles,
  anything depending on it (including the final binary) relinks.
- **`build.rs` emits `cargo:rerun-if-changed` directives** for every input
  the snapshot depends on:
  - `target/.dusk_sh_entries/` (the directory) and each JSON inside —
    edit any sh-entry's `short_description` and the snapshot regenerates.
  - The workspace `Cargo.lock` (absolute path) — add or remove a program
    crate and the snapshot regenerates. Earlier we had `Cargo.lock` as a
    relative path which silently always triggered a rerun; the absolute
    path via `concat!(env!("CARGO_MANIFEST_DIR"), "/../../../Cargo.lock")`
    fixes that.
  - `src/llm/params.rs` — bump `CONTEXT_TOKENS` or `KV_CACHE_TYPE` and the
    snapshot regenerates.
  - The model file path itself — swap GGUF and the snapshot regenerates.
- **`dusk_prompt` depends on `dusk_base` with `client`**, which transitively
  pulls in every program crate. Cargo schedules `dusk_prompt/build.rs` after
  every program crate is compiled, so the JSON files are guaranteed present
  when `build.rs` reads them.

## Tunables

- `DUSK_LLM_THREADS_COUNT` (runtime) — override decode thread count.
  Falls back to `std::thread::available_parallelism()`, then 1.
- `DUSK_MODEL_PATH` (build time) — override the GGUF model location.
  Falls back to `~/git/model/gemma*.gguf`.
- `src/llm/params.rs` — `CONTEXT_TOKENS` and `KV_CACHE_TYPE`. Changing
  either invalidates the snapshot via `cargo:rerun-if-changed`.

## Knobs that *must* match between build and runtime

- `CONTEXT_TOKENS` — the cache buffer is sized for `n_ctx`. Loading a 4K
  snapshot into a 2K context is undefined.
- `KV_CACHE_TYPE` — bytes are formatted per the quantisation.

Both live in `src/llm/params.rs` and are referenced from both build.rs (via
`include!`) and `llm/mod.rs` (via `mod params;`). Single source of truth,
no duplication.

## Knobs that may differ

- **Thread count.** Build-time uses host parallelism; runtime uses
  `DUSK_LLM_THREADS_COUNT` or runtime parallelism. The bytes
  `copy_state_data` writes are deterministic given the model and input —
  thread count only affects how fast the decode runs.
- **Model file location.** The build can point at any GGUF; the runtime
  finds its own. They need to be **the same model**, but not at the same
  path. (Cross-machine reproducibility: the runtime expects the model in
  the user's `~/git/model/` by default.)

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
  way to truncate `next_position` back to `SNAPSHOT_TOKENS`. Each
  turn sees every prior turn. This is fine for a few Q/As but the
  4K-token context will eventually fill — at which point decode
  fails. Workaround: exit and restart the shell to get a fresh
  context (the snapshot reloads from `include_bytes!`, the model
  reloads from the GGUF).
- **`Failed` is sticky.** If `Session::load` errors (missing GGUF,
  llama.cpp init failure, snapshot mismatch), the state transitions
  to `Failed(String)` and every subsequent `Llm::ask` returns the
  saved error immediately. No retry, no recovery within the session.
- **The build assumes the host has the GGUF.** `dusk_prompt/build.rs`
  needs to actually load the model to warm it up — there is no way
  to build `dusk_prompt` without a GGUF on disk. Set `DUSK_MODEL_PATH`
  to point at any compatible Gemma file, or drop one into
  `~/git/model/`.
