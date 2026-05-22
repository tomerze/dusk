include!("src/llm/params.rs");

use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use std::num::NonZeroU32;

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
    println!(
        "cargo:rerun-if-changed={}/../../../Cargo.lock",
        env!("CARGO_MANIFEST_DIR")
    );
    println!("cargo:rerun-if-env-changed=DUSK_MODEL_PATH");

    let sh_entries_info = collect_sh_entries_info()?;
    let system_prompt = compose_system_prompt(&sh_entries_info);
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
}

fn run_warmup(model_path: &Path, system_prompt: &str) -> Result<(Vec<u8>, usize)> {
    let threads: i32 = std::thread::available_parallelism()
        .ok()
        .and_then(|count| i32::try_from(count.get()).ok())
        .unwrap_or(1);
    let backend = LlamaBackend::init().context("initialising llama backend")?;
    let model = LlamaModel::load_from_file(&backend, model_path, &LlamaModelParams::default())
        .with_context(|| format!("loading GGUF model from {}", model_path.display()))?;
    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS))
        .with_type_k(KV_CACHE_TYPE)
        .with_type_v(KV_CACHE_TYPE)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
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

fn collect_sh_entries_info() -> Result<Vec<ShEntrySpec>> {
    let dir = locate_entries_info_dir()?;
    if !dir.exists() {
        bail!(
            "entries-info dir {} does not exist — are the program crates compiling with `client` enabled?",
            dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut sh_entries_info: Vec<ShEntrySpec> = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());

        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let spec: ShEntrySpec = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        sh_entries_info.push(spec);
    }
    sh_entries_info.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sh_entries_info)
}

fn compose_system_prompt(sh_entries_info: &[ShEntrySpec]) -> String {
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
         OUTPUT FORMAT — non-negotiable. Your ENTIRE reply must be ONE JSON \
         object on a single line, with exactly two keys: `explanation` (a short \
         sentence) and `command` (a dusk command string, or empty). Output starts \
         with `{` and ends with `}`. Nothing before, nothing after. No markdown. \
         No code fences. No prose. No greeting. No apology. If you cannot fulfil \
         the request, still reply with valid JSON: an empty `command` and an \
         explanation of why.\n\n\
         Examples — copy this format exactly:\n\
         Q: list processes\n\
         A: {\"explanation\":\"Use ps to list running processes.\",\"command\":\"ps\"}\n\
         Q: show me the weather\n\
         A: {\"explanation\":\"dusk has no networking command for fetching weather.\",\"command\":\"\"}\n\
         Q: sleep one second then list processes\n\
         A: {\"explanation\":\"Chain sleep and ps with a semicolon.\",\"command\":\"sleep 1000 ; ps\"}\n\n\
         Available programs:\n",
    );
    for entry in sh_entries_info {
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

fn locate_model() -> Result<PathBuf> {
    if let Some(path) = env::var_os("DUSK_MODEL_PATH") {
        let path = PathBuf::from(path);
        if !path.exists() {
            bail!("DUSK_MODEL_PATH points at {} which does not exist", path.display());
        }
        return Ok(path);
    }
    let home = env::var_os("HOME").context("HOME not set")?;
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

fn locate_entries_info_dir() -> Result<PathBuf> {
    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let target = Path::new(&out_dir)
        .ancestors()
        .nth(4)
        .context("OUT_DIR has fewer than four ancestors — cargo layout changed?")?;
    Ok(target.join(".dusk_sh_entries"))
}