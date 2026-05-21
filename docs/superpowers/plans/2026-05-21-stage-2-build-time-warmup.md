# Stage 2 — Build-time LLM warm-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `build.rs` to `dusk_prompt` that, at compile time, enumerates every `[[package.metadata.dusk.sh_entry]]` from every package in the resolved dep graph (via `cargo metadata`), composes the system prompt, runs llama.cpp warm-up against the GGUF model, and writes the resulting KV-cache state bytes + token count to `OUT_DIR` for embedding by the runtime in Stage 3.

**Prerequisite:** Stage 1 is complete and merged. Every shipping program crate has its `[[package.metadata.dusk.sh_entry]]` table populated. If Stage 1 is incomplete, this stage produces an empty / wrong catalogue.

**Architecture:** The build script is the *only* compile-time consumer that runs llama.cpp. It uses `llama-cpp-2` as a build-dependency, loads the same GGUF the runtime would load, builds a context with parameters that must match the runtime (shared via `include!`-ing `src/llm/params.rs`), tokenises the system prompt, decodes it in one batch, snapshots the raw state with `copy_state_data`, and writes two files to `OUT_DIR`:
- `model.state` — raw state bytes, embedded at runtime via `include_bytes!` (Stage 3).
- `snapshot_meta.rs` — single `const SNAPSHOT_TOKENS: i32 = N;` brought into scope at runtime via `include!`.

`cargo:rerun-if-changed` directives ensure the snapshot regenerates when any sh_entry source changes (each package's `manifest_path`), when the dep set changes (`Cargo.lock`), or when context parameters change (`params.rs`).

**Tech Stack:** Rust 2024, `llama-cpp-2 = "0.1.146"` (build-dep), `cargo_metadata` (build-dep), `serde` + `serde_json` (build-dep), `anyhow` (build-dep).

**Source of truth for runtime behaviour to match:** `dusk/src/dusk_prompt/src/llm.rs` (today's runtime warm-up code) defines `CONTEXT_TOKENS = 4_096`, `INFERENCE_THREADS = 4`, KV cache type `Q8_0`, and the Gemma chat-template fragments used in `Loaded::warm_up_and_save_cache`. The build.rs reproduces that flow.

---

## File Structure

**Created:**
- `dusk/src/dusk_prompt/build.rs` — the warm-up driver.
- `dusk/src/dusk_prompt/src/llm/params.rs` — `pub const CONTEXT_TOKENS: u32; pub const INFERENCE_THREADS: i32;`.
- (Stage 3 will move `src/llm.rs` into `src/llm/mod.rs`; this stage prepares `params.rs` under a new `src/llm/` directory but leaves the existing `src/llm.rs` untouched. The runtime will adopt the new directory in Stage 3.)

**Modified:**
- `dusk/src/dusk_prompt/Cargo.toml` — add a `[build-dependencies]` section.

**Generated at build time (not committed):**
- `${OUT_DIR}/model.state` — written by `build.rs`.
- `${OUT_DIR}/snapshot_meta.rs` — written by `build.rs`.

---

## Task 1: Extract context constants into a file shared by runtime + build

**Files:**
- Create: `dusk/src/dusk_prompt/src/llm/params.rs`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p dusk/src/dusk_prompt/src/llm
```

- [ ] **Step 2: Write `params.rs`**

Path: `dusk/src/dusk_prompt/src/llm/params.rs`

```rust
pub const CONTEXT_TOKENS: u32 = 4_096;
pub const INFERENCE_THREADS: i32 = 4;
```

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/src/llm/params.rs
git commit -m "dusk_prompt: extract context params into src/llm/params.rs for build.rs sharing"
```

Note: the runtime `llm.rs` is not yet using this file. Stage 3 wires it in. The file exists now so build.rs can `include!` it.

---

## Task 2: Add `[build-dependencies]` to `dusk_prompt`

**Files:**
- Modify: `dusk/src/dusk_prompt/Cargo.toml`

- [ ] **Step 1: Append a build-dependencies section**

Add at the end of `dusk/src/dusk_prompt/Cargo.toml`:

```toml
[build-dependencies]
llama-cpp-2 = "0.1.146"
cargo_metadata = "0.18"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

- [ ] **Step 2: Sanity-check that the crate still resolves**

```bash
cargo check -p dusk_prompt
```

Expected: `Finished` (no build.rs exists yet — cargo just resolves the manifest).

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/Cargo.toml
git commit -m "dusk_prompt: add build-dependencies for build-time warm-up"
```

---

## Task 3: Write `build.rs` skeleton (no warm-up yet)

**Files:**
- Create: `dusk/src/dusk_prompt/build.rs`

- [ ] **Step 1: Create the file with parsing + rerun-if-changed directives only**

Path: `dusk/src/dusk_prompt/build.rs`

```rust
include!("src/llm/params.rs");

use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(serde::Deserialize, Clone)]
struct ShEntrySpec {
    name: String,
    short_description: String,
    #[serde(default)]
    long_description: String,
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/llm/params.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-env-changed=DUSK_MODEL_PATH");

    let catalogue = collect_catalogue()?;

    // Stage 2 (this task) stops at "we read the catalogue and can print it".
    // Stage 2 Task 5 wires in the warm-up.
    eprintln!("dusk_prompt build.rs: {} sh_entries in catalogue", catalogue.len());
    for entry in &catalogue {
        eprintln!("  - {} — {}", entry.name, entry.short_description);
    }

    Ok(())
}

fn collect_catalogue() -> Result<Vec<ShEntrySpec>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .exec()
        .context("running `cargo metadata`")?;

    let mut catalogue: Vec<ShEntrySpec> = Vec::new();
    for package in &metadata.packages {
        // Watch every package's manifest so edits to any sh_entry table re-run the build.
        println!("cargo:rerun-if-changed={}", package.manifest_path);

        let Some(entries_value) = package
            .metadata
            .get("dusk")
            .and_then(|d| d.get("sh_entry"))
        else {
            continue;
        };
        let entries: Vec<ShEntrySpec> = serde_json::from_value(entries_value.clone())
            .with_context(|| {
                format!(
                    "parsing [[package.metadata.dusk.sh_entry]] in {}",
                    package.manifest_path
                )
            })?;
        catalogue.extend(entries);
    }
    catalogue.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(catalogue)
}
```

- [ ] **Step 2: Build dusk_prompt and watch the build log**

```bash
cargo clean -p dusk_prompt
cargo build -p dusk_prompt 2>&1 | grep -E "sh_entries in catalogue|^  - "
```

Expected (after Stage 1): one line `dusk_prompt build.rs: N sh_entries in catalogue` followed by N bullets, one per migrated program.

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/build.rs
git commit -m "dusk_prompt: build.rs skeleton that reads sh_entry catalogue"
```

---

## Task 4: Add the system-prompt composer

**Files:**
- Modify: `dusk/src/dusk_prompt/build.rs`

- [ ] **Step 1: Add `compose_system_prompt` and call it after `collect_catalogue`**

Replace the body of `main` (everything inside `fn main() -> Result<()> { ... }`) with:

```rust
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/llm/params.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-env-changed=DUSK_MODEL_PATH");

    let catalogue = collect_catalogue()?;
    let system_prompt = compose_system_prompt(&catalogue);
    eprintln!(
        "dusk_prompt build.rs: composed system prompt ({} bytes, {} entries)",
        system_prompt.len(),
        catalogue.len()
    );

    Ok(())
```

Append the composer fn at the bottom of the file:

```rust
fn compose_system_prompt(catalogue: &[ShEntrySpec]) -> String {
    let mut buffer = String::from(
        "You are Duck, the assistant for the dusk shell. dusk uses POSIX-sh syntax for \
         control flow, but its programs are NOT GNU/coreutils — only the flags listed \
         below exist. Do not invent flags.\n\n\
         Grammar (everything dusk supports, nothing else exists):\n\
         - `cmd args` — words split on whitespace; quote with `'…'` or `\"…\"`.\n\
         - `a ; b` or newline — sequence.\n\
         - `a && b`, `a || b` — short-circuit AND/OR on exit status.\n\
         - `sh -d \"cmd\"` — background/detached. There is NO `&` postfix.\n\
         - `name() { body }` — define. Body is `;`-separated statements.\n\
         - `name` — call. Functions are invoked by bare name, just like bash/sh \
         (`hi() { echo hi; }` then `hi`). NEVER write `name()` to call. Recurse by \
         name; that's how you loop, since there is no `while` or `for`.\n\
         Not in dusk: pipes `|`, redirection `> >> <`, `&` postfix, subshells `(…)`, \
         `$VAR`, `$(…)`, backticks, globs. If a request needs any of these, return an \
         empty `command` and explain.\n\n\
         Reply with ONE JSON object, nothing else: \
         {\"explanation\": \"<one short sentence>\", \"command\": \"<dusk command or empty>\"}. \
         Exactly one of each key. No markdown, no code fences, no prose outside the JSON.\n\n\
         Available programs:\n",
    );
    for entry in catalogue {
        buffer.push_str("\n## ");
        buffer.push_str(&entry.name);
        buffer.push_str(" — ");
        buffer.push_str(&entry.short_description);
        buffer.push('\n');
        let long = entry.long_description.trim();
        if !long.is_empty() {
            buffer.push_str(long);
            buffer.push('\n');
        }
    }
    buffer
}
```

- [ ] **Step 2: Run a fresh build and verify the log line**

```bash
cargo clean -p dusk_prompt
cargo build -p dusk_prompt 2>&1 | grep "composed system prompt"
```

Expected: `dusk_prompt build.rs: composed system prompt (NNNN bytes, M entries)` where NNNN is several thousand bytes for the canonical catalogue.

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/build.rs
git commit -m "dusk_prompt: build.rs composes system prompt from catalogue"
```

---

## Task 5: Locate the GGUF model file in build.rs

**Files:**
- Modify: `dusk/src/dusk_prompt/build.rs`

- [ ] **Step 1: Add the locator and call it after `compose_system_prompt`**

Append the locator at the bottom of the file:

```rust
fn locate_model() -> Result<PathBuf> {
    if let Some(path) = env::var_os("DUSK_MODEL_PATH") {
        let path = PathBuf::from(path);
        if !path.exists() {
            bail!("DUSK_MODEL_PATH points at {} which does not exist", path.display());
        }
        return Ok(path);
    }
    let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?;
    let dir = PathBuf::from(home).join("git/model");
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with("gemma") && path.extension().and_then(|e| e.to_str()) == Some("gguf") {
            return Ok(path);
        }
    }
    bail!("no `gemma*.gguf` found under {}", dir.display())
}
```

Modify `main` to add a `locate_model()` call after the catalogue + system prompt step:

```rust
    let catalogue = collect_catalogue()?;
    let system_prompt = compose_system_prompt(&catalogue);
    let model_path = locate_model()?;
    println!("cargo:rerun-if-changed={}", model_path.display());
    eprintln!(
        "dusk_prompt build.rs: composed system prompt ({} bytes, {} entries), model at {}",
        system_prompt.len(),
        catalogue.len(),
        model_path.display(),
    );
    Ok(())
```

- [ ] **Step 2: Build and verify**

```bash
cargo clean -p dusk_prompt
cargo build -p dusk_prompt 2>&1 | grep "model at"
```

Expected: a line ending with the path of the model gguf, e.g. `… model at /home/<user>/git/model/gemma-4-E2B-it-Q4_K_M.gguf`.

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_prompt/build.rs
git commit -m "dusk_prompt: build.rs locates GGUF model"
```

---

## Task 6: Run the actual warm-up and write `model.state`

**Files:**
- Modify: `dusk/src/dusk_prompt/build.rs`

- [ ] **Step 1: Add llama imports + the warm-up driver**

At the top of the file, after the existing `use` statements, add:

```rust
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use std::num::NonZeroU32;
```

At the bottom of the file, add the warm-up fn:

```rust
fn run_warmup(model_path: &std::path::Path, system_prompt: &str) -> Result<(Vec<u8>, usize)> {
    let backend = LlamaBackend::init().context("initialising llama backend")?;
    let model = LlamaModel::load_from_file(&backend, model_path, &LlamaModelParams::default())
        .with_context(|| format!("loading GGUF model from {}", model_path.display()))?;
    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS))
        .with_type_k(KvCacheType::Q8_0)
        .with_type_v(KvCacheType::Q8_0)
        .with_n_threads(INFERENCE_THREADS)
        .with_n_threads_batch(INFERENCE_THREADS);
    let mut context = model
        .new_context(&backend, context_params)
        .context("creating llama context")?;

    let opening = format!("<start_of_turn>user\n{system_prompt}\n\n");
    let tokens = model
        .str_to_token(&opening, AddBos::Always)
        .context("tokenising warm-up prefix")?;
    if tokens.is_empty() {
        bail!("warm-up prefix tokenised to zero tokens");
    }

    let mut batch = LlamaBatch::new(tokens.len(), 1);
    let last_index = tokens.len() - 1;
    for (offset, token) in tokens.iter().enumerate() {
        let position: i32 = offset.try_into().context("position overflows i32")?;
        batch
            .add(*token, position, &[0], offset == last_index)
            .context("adding warm-up token to batch")?;
    }
    context.decode(&mut batch).context("decoding warm-up batch")?;

    let state_size = context.get_state_size();
    let mut buffer = vec![0u8; state_size];
    // SAFETY: buffer has state_size bytes allocated; copy_state_data writes at most that many.
    let written = unsafe { context.copy_state_data(buffer.as_mut_ptr()) };
    if written > state_size {
        bail!(
            "copy_state_data wrote {written} bytes into a {state_size}-byte buffer (corruption)"
        );
    }
    buffer.truncate(written);
    Ok((buffer, tokens.len()))
}
```

Update `main`'s tail to call the warm-up and write the files:

```rust
    let catalogue = collect_catalogue()?;
    let system_prompt = compose_system_prompt(&catalogue);
    let model_path = locate_model()?;
    println!("cargo:rerun-if-changed={}", model_path.display());

    let (state_bytes, token_count) = run_warmup(&model_path, &system_prompt)
        .context("running build-time warm-up")?;

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let state_path = out_dir.join("model.state");
    let meta_path = out_dir.join("snapshot_meta.rs");
    fs::write(&state_path, &state_bytes)
        .with_context(|| format!("writing {}", state_path.display()))?;
    let token_count_i32: i32 = token_count
        .try_into()
        .context("token count overflows i32")?;
    fs::write(
        &meta_path,
        format!("pub(super) const SNAPSHOT_TOKENS: i32 = {token_count_i32};\n"),
    )
    .with_context(|| format!("writing {}", meta_path.display()))?;

    eprintln!(
        "dusk_prompt build.rs: wrote {} ({} bytes) and {} (SNAPSHOT_TOKENS = {})",
        state_path.display(),
        state_bytes.len(),
        meta_path.display(),
        token_count_i32,
    );
    Ok(())
```

- [ ] **Step 2: Run a clean build (this is the slow one — first time may take 5–10 minutes for llama.cpp build-dep compilation, then ~30–60 seconds for warm-up)**

```bash
cargo clean -p dusk_prompt
cargo build -p dusk_prompt 2>&1 | tail -5
```

Expected: a line `dusk_prompt build.rs: wrote <target>/.../build/dusk_prompt-<hash>/out/model.state (NNNNNNNN bytes) and ... (SNAPSHOT_TOKENS = NNNN)`. Byte count is typically 100–200 MB at 4K context with Q8 KV. Token count is roughly the system prompt length in tokens.

- [ ] **Step 3: Verify the files were written**

```bash
find target -name model.state -path '*dusk_prompt*' -exec ls -lh {} \;
find target -name snapshot_meta.rs -path '*dusk_prompt*' -exec cat {} \;
```

Expected: `model.state` exists with a sane size (50+ MB), `snapshot_meta.rs` contains exactly one `pub(super) const SNAPSHOT_TOKENS: i32 = N;` line.

- [ ] **Step 4: Test rebuild idempotency**

```bash
cargo build -p dusk_prompt 2>&1 | tail -5
```

Expected: `Finished` with no `build.rs` activity in the log (build script did not re-run because nothing it watches changed).

- [ ] **Step 5: Test invalidation on a program-catalogue change**

```bash
touch programs/date/Cargo.toml
cargo build -p dusk_prompt 2>&1 | grep -E "composed system prompt|wrote .* model.state"
```

Expected: the build.rs ran again and wrote a fresh `model.state`. Restore the file's mtime afterwards if you care: `git checkout programs/date/Cargo.toml` is a no-op for content but does restore mtime.

- [ ] **Step 6: Commit**

```bash
git add dusk/src/dusk_prompt/build.rs
git commit -m "dusk_prompt: build.rs runs llama warm-up and writes model.state to OUT_DIR"
```

---

## Task 7: Self-check the rerun-if-changed coverage

**Files:** (none modified)

- [ ] **Step 1: Verify each invalidation trigger**

Run each scenario fresh and confirm the build script re-runs. Look for `dusk_prompt build.rs: composed system prompt` in the log; absence means the build script did NOT re-run.

a. **Edit a workspace program's sh_entry table.**

```bash
# Add a trailing space to long_description (no semantic change, mtime updates).
echo "" >> programs/date/Cargo.toml
cargo build -p dusk_prompt 2>&1 | grep "composed system prompt" | head -1
git checkout programs/date/Cargo.toml
```

Expected: one matching line.

b. **Touch `Cargo.lock`.**

```bash
touch Cargo.lock
cargo build -p dusk_prompt 2>&1 | grep "composed system prompt" | head -1
```

Expected: one matching line.

c. **Edit `params.rs`.**

```bash
# (Temporarily) bump CONTEXT_TOKENS.
sed -i 's/CONTEXT_TOKENS: u32 = 4_096/CONTEXT_TOKENS: u32 = 4_097/' \
  dusk/src/dusk_prompt/src/llm/params.rs
cargo build -p dusk_prompt 2>&1 | grep "composed system prompt" | head -1
git checkout dusk/src/dusk_prompt/src/llm/params.rs
```

Expected: one matching line.

d. **No changes → no rerun.**

```bash
cargo build -p dusk_prompt 2>&1 | grep -c "composed system prompt"
```

Expected: `0`.

- [ ] **Step 2: If any of (a)–(c) failed, debug the cargo:rerun-if-changed directive list in `build.rs` and re-test**

If, for example, scenario (a) didn't trigger, then either the program's `manifest_path` wasn't being emitted as a watch, or its sh_entry table wasn't found in cargo metadata (Stage 1 incomplete for that program). Inspect:

```bash
cargo metadata --format-version 1 | python3 -c "
import json, sys
m = json.load(sys.stdin)
for p in m['packages']:
    if p['name'].startswith('dusk_program_'):
        print(p['name'], p['manifest_path'], 'has_dusk_meta:', bool(p.get('metadata', {}).get('dusk')))
"
```

Each canonical program should show `has_dusk_meta: True`. Fix any `False` rows by completing Stage 1 for that program.

- [ ] **Step 3: No commit needed unless step 2 surfaced and fixed an issue**

---

## Self-review checklist

- [ ] `build.rs` exists at `dusk/src/dusk_prompt/build.rs` and the crate builds with no warnings.
- [ ] `build.rs` produces `OUT_DIR/model.state` and `OUT_DIR/snapshot_meta.rs` on first build.
- [ ] Rebuilding without changes does not re-run the warm-up (the slow path).
- [ ] Touching any program's Cargo.toml re-runs the warm-up.
- [ ] Touching `Cargo.lock` re-runs the warm-up.
- [ ] Touching `src/llm/params.rs` re-runs the warm-up.
- [ ] `model.state` is between roughly 50 MB and 250 MB (4K context Q8 KV).
- [ ] `snapshot_meta.rs` contains exactly one `pub(super) const SNAPSHOT_TOKENS: i32 = N;` line and nothing else.
- [ ] The runtime `dusk_prompt::llm` module is unchanged — Stage 3 is what wires the snapshot into the runtime.
