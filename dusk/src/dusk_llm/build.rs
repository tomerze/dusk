use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONTEXT_TOKENS: u32 = 16_384;
// ggml_type::GGML_TYPE_Q8_0
const KV_CACHE_TYPE: i32 = 8;
const MODEL_GGUF_PATH: &str = "models/gemma-4-E2B-it-Q4_K_M.gguf";

#[derive(serde::Deserialize, Clone)]
struct ShEntrySpec {
    name: String,
    short_description: String,
    #[serde(default)]
    long_description: String,
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=prompts/system.md");
    println!("cargo:rerun-if-changed=warmup/main.c");
    println!(
        "cargo:rerun-if-changed={}/../../../Cargo.lock",
        env!("CARGO_MANIFEST_DIR")
    );

    let llama_src = locate_llama_src()?;
    println!(
        "cargo:rerun-if-changed={}/CMakeLists.txt",
        llama_src.display()
    );
    println!(
        "cargo:rerun-if-changed={}/include/llama.h",
        llama_src.display()
    );

    let install_prefix = build_llama_cpp(&llama_src).context("building vendored llama.cpp")?;
    let lib_dir = install_prefix.join("lib");
    let include_dir = install_prefix.join("include");

    emit_link_directives(&lib_dir)?;

    let warmup_binary = compile_warmup_binary(&llama_src, &lib_dir, &include_dir)
        .context("compiling dusk_warmup binary")?;

    let sh_entries_info = collect_sh_entries_info()?;
    let system_prompt = compose_system_prompt(&sh_entries_info);
    let model_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(MODEL_GGUF_PATH);
    if !model_path.is_file() {
        bail!("model GGUF not found at {}", model_path.display());
    }
    println!("cargo:rerun-if-changed={}", model_path.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let state_path = out_dir.join("dusk_llm_kv_snapshot");
    let key_path = out_dir.join("dusk_llm_kv_snapshot.key");

    // Gemma 4 turn token
    let prompt_text = format!("<|turn>user\n{system_prompt}");

    // Regenerating the snapshot runs the full model over the warm-up
    // prompt and is the slow part of this build. Skip it when the inputs
    // that determine its contents are byte-for-byte unchanged: a stale
    // rerun-if-changed trigger (e.g. an unrelated program recompiling and
    // touching .dusk_sh_entries) must not force a regeneration.
    let cache_key = warmup_cache_key(&prompt_text, &model_path)?;
    let cached = state_path.is_file()
        && fs::read_to_string(&key_path).ok().as_deref() == Some(cache_key.as_str());
    if cached {
        eprintln!(
            "dusk_llm build.rs: warm-up snapshot {} up to date, skipping regeneration",
            state_path.display(),
        );
        return Ok(());
    }

    write_warmup_snapshot(&warmup_binary, &model_path, &prompt_text, &state_path)
        .context("producing build-time warm-up snapshot")?;
    fs::write(&key_path, &cache_key)
        .with_context(|| format!("writing cache key {}", key_path.display()))?;

    eprintln!(
        "dusk_llm build.rs: wrote {} ({} bytes)",
        state_path.display(),
        fs::metadata(&state_path)?.len(),
    );
    Ok(())
}

/// Content key for the warm-up snapshot: changes whenever an input that
/// affects the snapshot's bytes changes (the exact warm-up prompt, the
/// context/KV-cache parameters, or the model file's size and mtime).
/// Used to short-circuit regeneration when a `rerun-if-changed` trigger
/// fires without any real change. Not cryptographic — collision
/// resistance is not needed for a same-machine build cache.
fn warmup_cache_key(prompt_text: &str, model_path: &Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::UNIX_EPOCH;

    let mut hasher = DefaultHasher::new();
    prompt_text.hash(&mut hasher);
    CONTEXT_TOKENS.hash(&mut hasher);
    KV_CACHE_TYPE.hash(&mut hasher);

    let metadata =
        fs::metadata(model_path).with_context(|| format!("stat {}", model_path.display()))?;
    metadata.len().hash(&mut hasher);
    let modified = metadata
        .modified()
        .with_context(|| format!("mtime unavailable for {}", model_path.display()))?;
    let since_epoch = modified
        .duration_since(UNIX_EPOCH)
        .context("model mtime is before the unix epoch")?;
    since_epoch.as_secs().hash(&mut hasher);
    since_epoch.subsec_nanos().hash(&mut hasher);

    Ok(format!("{:016x}", hasher.finish()))
}

fn locate_llama_src() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .ancestors()
        .nth(3)
        .context("dusk_llm Cargo.toml has fewer than four ancestors")?
        .join("vendor/llama.cpp");
    if !path.join("CMakeLists.txt").is_file() {
        bail!(
            "vendor/llama.cpp not initialised at {}, run `git submodule update --init vendor/llama.cpp`",
            path.display()
        );
    }
    Ok(path)
}

fn build_llama_cpp(source: &Path) -> Result<PathBuf> {
    let mut config = cmake::Config::new(source);

    config
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_BUILD_APP", "OFF")
        .define("LLAMA_BUILD_COMMON", "OFF")
        .define("LLAMA_CURL", "OFF")
        .define("GGML_OPENMP", "ON")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_INSTALL_RPATH_USE_LINK_PATH", "ON");

    let target_features = std::env::var("CARGO_CFG_TARGET_FEATURE")
        .expect("Env var CARGO_CFG_TARGET_FEATURE not found.");

    const FEATURE_TO_GGML_FLAG: &[(&str, &str)] = &[
        ("avx", "GGML_AVX"),
        ("avx2", "GGML_AVX2"),
        ("avx512bf16", "GGML_AVX512_BF16"),
        ("avx512vbmi", "GGML_AVX512_VBMI"),
        ("avx512vnni", "GGML_AVX512_VNNI"),
        ("avxvnni", "GGML_AVX_VNNI"),
        ("f16c", "GGML_F16C"),
        ("bmi2", "GGML_BMI2"),
        ("sse4.2", "GGML_SSE42"),
        ("fma", "GGML_FMA"),
    ];

    for feature in target_features.split(',') {
        if let Some((_, flag)) = FEATURE_TO_GGML_FLAG
            .iter()
            .find(|(name, _)| *name == feature)
        {
            config.define(flag, "ON");
        }
    }
    let build = config.profile("Release").build();
    Ok(build)
}

fn emit_link_directives(lib_dir: &Path) -> Result<()> {
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=llama");
    println!("cargo:rustc-link-lib=static=ggml");
    println!("cargo:rustc-link-lib=static=ggml-cpu");
    println!("cargo:rustc-link-lib=static=ggml-base");
    // libstdc++ (libllama is C++) and libgomp (ggml's OpenMP runtime)
    // are linked statically — final binary has no .so dep on either.
    for archive in ["stdc++", openmp_static_lib_name()?] {
        let dir = locate_static_archive_dir(archive)?;
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static={archive}");
    }
    // libgcc_s is suppressed via `-C link-arg=-static-libgcc` in the
    // workspace .cargo/config.toml; can't be done from a library
    // build.rs because rustc-link-arg only propagates to the package
    // that emits it (and dusk_llm is an rlib, not a bin).
    //
    // libpthread, libm, libdl are part of glibc; they remain dynamic.
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    Ok(())
}

/// Pick the OpenMP archive name (`gomp` for GCC, `omp` for Clang) by
/// probing `$CC`'s `--version` output.
fn openmp_static_lib_name() -> Result<&'static str> {
    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let probe = Command::new(&cc).arg("--version").output();
    let is_clang = probe
        .ok()
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains("clang"));
    Ok(if is_clang { "omp" } else { "gomp" })
}

/// Resolve the directory containing `lib<name>.a` by asking the C
/// compiler with `cc -print-file-name=lib<name>.a`. Bails with an
/// actionable message if the archive isn't on the compiler's search
/// path (the compiler echoes the input verbatim in that case).
fn locate_static_archive_dir(name: &str) -> Result<PathBuf> {
    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let archive_name = format!("lib{name}.a");
    let output = Command::new(&cc)
        .arg(format!("-print-file-name={archive_name}"))
        .output()
        .with_context(|| format!("invoking `{cc} -print-file-name={archive_name}`"))?;
    if !output.status.success() {
        bail!(
            "{cc} -print-file-name={archive_name} failed (exit {:?})",
            output.status.code()
        );
    }
    let reported = String::from_utf8(output.stdout)
        .with_context(|| format!("compiler returned non-UTF-8 path for {archive_name}"))?;
    let reported = reported.trim();
    if reported == archive_name {
        bail!(
            "compiler `{cc}` could not locate {archive_name}, install it on the system via your package manager"
        );
    }
    let archive_path = PathBuf::from(reported);
    let archive_dir = archive_path
        .parent()
        .ok_or_else(|| anyhow!("archive path has no parent: {}", archive_path.display()))?
        .to_path_buf();
    Ok(archive_dir)
}

fn compile_warmup_binary(llama_src: &Path, lib_dir: &Path, include_dir: &Path) -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("warmup/main.c");
    if !source.is_file() {
        bail!("missing {}", source.display());
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let binary = out_dir.join("dusk_warmup");

    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let ggml_include = llama_src.join("ggml/include");

    let omp_name = openmp_static_lib_name()?;
    let stdcxx_archive = locate_static_archive_dir("stdc++")?.join("libstdc++.a");
    let gcc_archive = locate_static_archive_dir("gcc")?.join("libgcc.a");
    let omp_archive = locate_static_archive_dir(omp_name)?.join(format!("lib{omp_name}.a"));

    let mut cmd = Command::new(&cc);
    cmd.arg("-std=c11")
        .arg("-O2")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .arg(format!("-I{}", include_dir.display()))
        .arg(format!("-I{}", ggml_include.display()))
        .arg(format!("-L{}", lib_dir.display()))
        // start-group lets the linker resolve circular static-archive
        // references between llama, ggml, libgomp, libstdc++, libgcc.
        .arg("-Wl,--start-group")
        .arg("-lllama")
        .arg("-lggml")
        .arg("-lggml-cpu")
        .arg("-lggml-base")
        .arg(&omp_archive)
        .arg(&stdcxx_archive)
        .arg(&gcc_archive)
        .arg("-Wl,--end-group")
        .arg("-lpthread")
        .arg("-lm")
        .arg("-ldl");

    let output = cmd
        .output()
        .with_context(|| format!("invoking {cc} to link warmup binary"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "warmup compilation failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            stdout,
            stderr,
        );
    }
    Ok(binary)
}

fn write_warmup_snapshot(
    warmup_binary: &Path,
    model_path: &Path,
    prompt_text: &str,
    state_path: &Path,
) -> Result<()> {
    if state_path.exists() {
        fs::remove_file(state_path)
            .with_context(|| format!("removing stale {}", state_path.display()))?;
    }
    let threads = std::thread::available_parallelism()
        .ok()
        .map(|count| count.get().to_string())
        .unwrap_or_else(|| String::from("1"));

    let args: Vec<OsString> = vec![
        model_path.as_os_str().to_owned(),
        OsString::from(CONTEXT_TOKENS.to_string()),
        OsString::from(KV_CACHE_TYPE.to_string()),
        OsString::from(&threads),
        state_path.as_os_str().to_owned(),
        OsString::from(prompt_text),
    ];

    let output = Command::new(warmup_binary)
        .args(&args)
        .output()
        .with_context(|| format!("invoking {}", warmup_binary.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "dusk_warmup failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            stdout,
            stderr,
        );
    }
    if !state_path.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "dusk_warmup exited 0 but did not write {} — stderr:\n{}",
            state_path.display(),
            stderr,
        );
    }
    Ok(())
}

fn collect_sh_entries_info() -> Result<Vec<ShEntrySpec>> {
    let dir = locate_entries_info_dir()?;
    if !dir.exists() {
        bail!(
            "could find entries-info {} dir, are there programs compiling with `sh_entry`s emitted?",
            dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut sh_entries_info: Vec<ShEntrySpec> = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());

        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let spec: ShEntrySpec =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        sh_entries_info.push(spec);
    }
    sh_entries_info.sort_by(|first, second| first.name.cmp(&second.name));
    Ok(sh_entries_info)
}

fn compose_system_prompt(sh_entries_info: &[ShEntrySpec]) -> String {
    let template = include_str!("prompts/system.md");
    let mut programs_block = String::new();
    for entry in sh_entries_info {
        programs_block.push_str("\n## ");
        programs_block.push_str(&entry.name);
        programs_block.push_str(" — ");
        programs_block.push_str(&entry.short_description);
        programs_block.push('\n');
        let long = entry.long_description.trim();
        if !long.is_empty() {
            programs_block.push_str(long);
            programs_block.push('\n');
        }
    }
    template.replace("{{PROGRAMS}}", programs_block.trim_start_matches('\n'))
}

fn locate_entries_info_dir() -> Result<PathBuf> {
    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let target = Path::new(&out_dir)
        .ancestors()
        .nth(4)
        .context("OUT_DIR has fewer than four ancestors — cargo layout changed?")?;
    Ok(target.join(".dusk_sh_entries"))
}
