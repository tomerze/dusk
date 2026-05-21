# LLM warm-up as a compile-time artifact

**Status:** approved (brainstorming, 2026-05-21)
**Stages:** 3 — see `docs/superpowers/plans/2026-05-21-stage-{1,2,3}-*.md`

## Problem

Today `dusk_prompt::llm` lazily loads the model on the first `Ask Duck` and decodes the system prompt at runtime. With a ~2K-token system prompt that costs the user 30–60 seconds the first time. We then cache the warmed KV state under `~/.cache/dusk/duck-<hash>.state` so subsequent shells are fast. That works, but:

1. The "system prompt" the cache is keyed on doesn't actually track the truth — when programs are added / removed / changed, the cache may not invalidate.
2. The first run on every machine pays the warm-up cost.
3. `llm.rs` is doing too many jobs: backend, KV cache file persistence, UI spinner, JSON parsing, error printing.

We want:

- **Warmup is compile-time only.** Ship a pre-warmed KV state with the binary.
- **The catalogue the LLM sees is the catalogue the user gets.** `SH_ENTRIES` (the linkme `#[distributed_slice]` populated by all `dusk_program_*` crates) is the source of truth, and Cargo.toml metadata mirrors that truth so build.rs can read it.
- **No runtime warmup, no `~/.cache/dusk/` cache.** Hash-check / fallback paths are gone.
- **Artifact bin stays one line.** `artifacts/dusk_cli/src/main.rs` is `fn main() -> Result<()> { dusk_cli::main() }` — no `install_snapshot` wiring, no per-program `black_box` for snapshot purposes (linker registration is a separate concern).
- **GUI out of the LLM module.** `llm.rs` has zero stdout writes; the spinner lives in a `ui::spinner` module that prompt.rs uses.

## Three stages

Each stage is shippable on its own and has its own implementation plan.

### Stage 1 — `sh_entries` as Cargo.toml metadata

Today every program crate hand-writes its `#[distributed_slice(SH_ENTRIES)] pub fn sh_entry() -> ShEntry { ... }` in source. We move the data half (name, short_description, long_description, impl_name) into the crate's Cargo.toml under `[[package.metadata.dusk.sh_entry]]` and have a proc macro generate the registration functions from it.

A program crate can declare multiple entries (the array form `[[ ]]`). The `impl_name` field names a type in the crate's source that implements `ProgramArgsBuilder` — the macro substitutes `Rc::new(<impl_name> {})`.

This is the only stage that changes the API of `dusk_program_proc`. After it, the same Cargo.toml table is what build.rs of `dusk_prompt` will read in stage 2 and what gets linked into the binary as `SH_ENTRIES` at runtime. They can no longer drift.

### Stage 2 — `dusk_prompt/build.rs` runs the warm-up

A new `build.rs` in `dusk/src/dusk_prompt/`:
- Runs `cargo metadata` (build-dep `cargo_metadata`), enumerates every package's `[[package.metadata.dusk.sh_entry]]` array, builds the catalogue.
- Composes the system prompt (fixed grammar text + the catalogue).
- Loads the GGUF model (build-dep `llama-cpp-2`), builds a context with the same params as runtime, decodes the warm-up prefix, calls `copy_state_data`.
- Writes `OUT_DIR/model.state` (raw bytes) and `OUT_DIR/snapshot_meta.rs` (containing `pub(super) const SNAPSHOT_TOKENS: i32 = N;`).
- Emits `cargo:rerun-if-changed` for: each package's `manifest_path`, `Cargo.lock`, `params.rs`, `build.rs` itself.

LLM context tunables (`CONTEXT_TOKENS`, `INFERENCE_THREADS`) live in `src/llm/params.rs`, included by both runtime (`mod params;`) and build.rs (`include!("src/llm/params.rs")`). KV dtype (`Q8_0`) is duplicated as a literal because `KvCacheType` is an enum import that doesn't survive `include!` cleanly.

### Stage 3 — Runtime loads the embedded snapshot, no runtime warm-up

`src/llm.rs` becomes `src/llm/mod.rs`. `src/reedline.rs` becomes `src/ui/mod.rs`. A new `src/ui/spinner.rs` holds `with_spinner` + the painting helpers (moved out of llm.rs).

Runtime flow:
- `static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));`
- `include!(concat!(env!("OUT_DIR"), "/snapshot_meta.rs"));` brings `SNAPSHOT_TOKENS` into scope.
- `Session::load` loads the model, builds the context, calls `unsafe { ctx.set_state_data(SNAPSHOT) }`, sets `next_position = SNAPSHOT_TOKENS`. That's it.

Code that disappears: `system_prompt` field on `Llm`; `LoadStage` enum + stage atomic; `state_cache_path`, FNV hash, `~/.cache/dusk/` paths; `warm_up_and_save_cache`; `Loaded::load` cache-miss branch; `build_system_prompt` (the build.rs has its own composer now); `ask_fail` and the `println!("chat failed: …")` inside `ask`. Spinner + paint helpers move to `ui::spinner`.

`Llm::ask` returns `Result<LlmReply>` (not `Option`). The caller (`prompt.rs`) wraps it in `with_spinner` and prints errors itself. Empty-input checking is the caller's job.

## Public surfaces at the end

```rust
// dusk_prompt::llm
pub struct Llm;                              // lazy facade
pub struct LlmReply { explanation, command }
impl Llm {
    pub fn new() -> Self;
    pub async fn ask(
        &self,
        message: &str,
        on_token: impl FnMut(usize) + Send + 'static,
    ) -> anyhow::Result<LlmReply>;
}

// dusk_prompt::ui::spinner
pub async fn with_spinner<F, T, L>(future: F, label_for_frame: L) -> T
where F: Future<Output = T>, L: FnMut() -> String;
```

## Drift guarantees

The Cargo.toml `[[package.metadata.dusk.sh_entry]]` table is the single source of truth.

- `dusk_program_proc::sh_entries!()` (stage 1) generates `#[distributed_slice(SH_ENTRIES)]` from it → runtime `SH_ENTRIES` is exactly that.
- `dusk_prompt/build.rs` (stage 2) reads it via `cargo metadata` → snapshot is built against exactly that.

Both halves consume the same table. They cannot disagree.

`cargo:rerun-if-changed` on each package's `manifest_path` catches edits. `cargo:rerun-if-changed=Cargo.lock` catches additions / removals / version bumps. Whether a program crate is from a workspace path, git, or crates.io does not matter — `cargo metadata` returns every resolved package.

## Naming placeholders

These are working names; rename to taste before merge.

- `sh_entries!` proc macro (or could fold into the existing `metadata!`).
- `Session` (replaces today's `Loaded`).
- `SNAPSHOT_TOKENS`, `model.state`, `snapshot_meta.rs` file names.
- `ui::spinner` module name.

## Flagged tradeoffs

- `[build-dependencies] llama-cpp-2` triggers a second llama.cpp compile for the host target on first build. On a host==target dev box this is once. On cross-compilation it builds twice.
- Snapshot embedded via `include_bytes!` adds the snapshot size (~150 MB at 4K context with Q8 KV) to the binary. `include_bytes!` handles 100+ MB without the linker memory issues we hit at 5.5 GB.
- `set_state_data` is `unsafe` (upstream doc: "help wanted: not entirely sure what the safety requirements are"). We carry that.
- A new program crate must define its `[[package.metadata.dusk.sh_entry]]` block AND a type matching its `impl_name`. The macro errors loudly if the type doesn't exist (compile failure with a clear span).
