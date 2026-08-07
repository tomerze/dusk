use anyhow::{Context, Result, anyhow, bail};
use fast_down::Event;
use fast_down::file::StdFilePusher;
use fast_down::http::{HttpPuller, Prefetch};
use fast_down::multi::{DownloadOptions, download_multi};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use url::Url;

const CONTEXT_TOKENS: u32 = 16_384;
// ggml_type::GGML_TYPE_Q8_0
const KV_CACHE_TYPE: i32 = 8;
const MODEL_MANIFEST_PATH: &str = "model.json";
const DOWNLOAD_CONNECTIONS: usize = 6;
const DOWNLOAD_MINIMUM_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
const DOWNLOAD_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(serde::Deserialize)]
struct ModelManifest {
    url: Url,
    file: String,
    sha256: String,
    directory: PathBuf,
}

#[derive(serde::Deserialize)]
struct ShEntrySpec {
    name: String,
    short_description: String,
    #[serde(default)]
    long_description: String,
}

fn main() -> Result<()> {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR is not set")?);
    for input in [
        "build.rs",
        "prompts/system.md",
        "warmup/main.c",
        MODEL_MANIFEST_PATH,
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    println!(
        "cargo:rerun-if-changed={}/../../../Cargo.lock",
        crate_root.display()
    );

    let model_path = ensure_model(&crate_root)?;
    println!("cargo:rerun-if-changed={}", model_path.display());
    println!(
        "cargo:rustc-env=DUSK_LLM_MODEL_PATH={}",
        model_path.display()
    );

    let llama_src = crate_root
        .ancestors()
        .nth(3)
        .context("dusk_llm's Cargo.toml has fewer than four ancestor directories")?
        .join("vendor/llama.cpp");
    if !llama_src.join("CMakeLists.txt").is_file() {
        bail!(
            "vendor/llama.cpp is not initialised at {}. Run `git submodule update --init vendor/llama.cpp` and build again.",
            llama_src.display()
        );
    }
    for input in ["CMakeLists.txt", "include/llama.h"] {
        println!("cargo:rerun-if-changed={}/{input}", llama_src.display());
    }

    let install_prefix = build_llama_cpp(&llama_src)?;
    emit_link_directives(&install_prefix.join("lib"))?;
    let warmup_binary = compile_warmup_binary(&llama_src, &install_prefix, &out_dir)?;

    // Gemma 4 turn token
    let prompt_text = format!("<|turn>user\n{}", system_prompt(&out_dir)?);
    let state_path = out_dir.join("dusk_llm_kv_snapshot");
    let key_path = out_dir.join("dusk_llm_kv_snapshot.key");

    // Regenerating the snapshot runs the full model over the warm-up
    // prompt and is the slow part of this build. Skip it when the inputs
    // that determine its contents are byte-for-byte unchanged: a stale
    // rerun-if-changed trigger (e.g. an unrelated program recompiling and
    // touching .dusk_sh_entries) must not force a regeneration.
    let model =
        fs::metadata(&model_path).with_context(|| format!("reading {}", model_path.display()))?;
    let modified = model
        .modified()
        .with_context(|| format!("reading the modification time of {}", model_path.display()))?;
    let mut hasher = DefaultHasher::new();
    (
        &prompt_text,
        CONTEXT_TOKENS,
        KV_CACHE_TYPE,
        model.len(),
        modified,
    )
        .hash(&mut hasher);
    let cache_key = format!("{:016x}", hasher.finish());
    if state_path.is_file() && fs::read_to_string(&key_path).ok().as_deref() == Some(&*cache_key) {
        eprintln!(
            "dusk_llm build.rs: The warm-up snapshot {} is up to date, so it is not regenerated.",
            state_path.display(),
        );
        return Ok(());
    }

    if state_path.exists() {
        fs::remove_file(&state_path)
            .with_context(|| format!("removing stale {}", state_path.display()))?;
    }
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    let output = run(
        Command::new(&warmup_binary).args([
            model_path.as_os_str().to_owned(),
            OsString::from(CONTEXT_TOKENS.to_string()),
            OsString::from(KV_CACHE_TYPE.to_string()),
            OsString::from(threads.to_string()),
            state_path.as_os_str().to_owned(),
            OsString::from(&prompt_text),
        ]),
        "dusk_warmup",
    )?;
    if !state_path.exists() {
        bail!(
            "dusk_warmup exited 0 but did not write {}.\n--- stderr ---\n{}",
            state_path.display(),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    fs::write(&key_path, &cache_key)
        .with_context(|| format!("writing cache key {}", key_path.display()))?;

    eprintln!(
        "dusk_llm build.rs: Wrote the warm-up snapshot {} ({} bytes).",
        state_path.display(),
        fs::metadata(&state_path)?.len(),
    );
    Ok(())
}

fn ensure_model(crate_root: &Path) -> Result<PathBuf> {
    use sha2::Digest;

    let manifest_path = crate_root.join(MODEL_MANIFEST_PATH);
    let manifest_text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: ModelManifest = serde_json::from_str(&manifest_text)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let directory = crate_root.join(&manifest.directory);
    let model_path = directory.join(&manifest.file);
    if model_path.is_file() {
        return Ok(model_path);
    }

    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    let partial_path = model_path.with_extension("partial");
    eprintln!(
        "dusk_llm build.rs: Downloading {} from {}.",
        manifest.file, manifest.url
    );
    tokio::runtime::Runtime::new()
        .context("starting a runtime for the download")?
        .block_on(download_model(&manifest.url, &partial_path))?;

    let mut hasher = sha2::Sha256::new();
    std::io::copy(
        &mut fs::File::open(&partial_path)
            .with_context(|| format!("opening {}", partial_path.display()))?,
        &mut hasher,
    )
    .with_context(|| format!("reading {}", partial_path.display()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != manifest.sha256 {
        bail!(
            "What {} served is not the model that {} names.\n  Expected sha256 {}\n  Actual   sha256 {}\n\
             The partial download is left at {} for inspection; delete it and build again to retry.",
            manifest.url,
            manifest_path.display(),
            manifest.sha256,
            actual,
            partial_path.display(),
        );
    }

    fs::rename(&partial_path, &model_path).with_context(|| {
        format!(
            "renaming {} to {}",
            partial_path.display(),
            model_path.display()
        )
    })?;
    eprintln!(
        "dusk_llm build.rs: Downloaded {} ({} bytes).",
        model_path.display(),
        fs::metadata(&model_path)?.len(),
    );
    Ok(model_path)
}

async fn download_model(url: &Url, destination: &Path) -> Result<()> {
    let client = reqwest::Client::new();
    let (info, _) = Prefetch::<reqwest::Client>::prefetch(&client, url.clone())
        .await
        .map_err(|(error, _)| anyhow!("The request for {url} failed: {error:?}."))?;
    let file = tokio::fs::File::create(destination)
        .await
        .with_context(|| format!("creating {}", destination.display()))?;
    let pusher = StdFilePusher::new(file, info.size, DOWNLOAD_BUFFER_BYTES, true)
        .await
        .with_context(|| format!("opening {} to write", destination.display()))?;

    let result = download_multi(
        HttpPuller::new(info.final_url.clone(), client, None, info.file_id),
        pusher,
        DownloadOptions {
            download_chunks: core::iter::once(0..info.size),
            concurrent: if info.supports_range {
                DOWNLOAD_CONNECTIONS
            } else {
                1
            },
            retry_gap: Duration::from_secs(1),
            pull_timeout: Duration::from_secs(30),
            push_queue_cap: 1024,
            min_chunk_size: DOWNLOAD_MINIMUM_CHUNK_BYTES,
            max_speculative: 3,
        },
    );

    while let Ok(event) = result.event_chain.recv().await {
        if matches!(
            event,
            Event::PullError(..)
                | Event::PullTimeout(..)
                | Event::PushError(..)
                | Event::FlushError(..)
        ) {
            eprintln!("dusk_llm build.rs: The download reported {event:?}.");
        }
    }
    result
        .join()
        .await
        .map_err(|error| anyhow!("The download did not finish: {error}."))?;
    Ok(())
}

fn build_llama_cpp(source: &Path) -> Result<PathBuf> {
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

    let mut config = cmake::Config::new(source);
    for (define, value) in [
        ("BUILD_SHARED_LIBS", "OFF"),
        ("LLAMA_BUILD_TESTS", "OFF"),
        ("LLAMA_BUILD_EXAMPLES", "OFF"),
        ("LLAMA_BUILD_SERVER", "OFF"),
        ("LLAMA_BUILD_TOOLS", "OFF"),
        ("LLAMA_BUILD_APP", "OFF"),
        ("LLAMA_BUILD_COMMON", "OFF"),
        ("LLAMA_CURL", "OFF"),
        ("GGML_OPENMP", "ON"),
        ("CMAKE_POSITION_INDEPENDENT_CODE", "ON"),
        ("CMAKE_INSTALL_RPATH_USE_LINK_PATH", "ON"),
    ] {
        config.define(define, value);
    }

    let target_features =
        env::var("CARGO_CFG_TARGET_FEATURE").context("CARGO_CFG_TARGET_FEATURE is not set")?;
    for (feature, flag) in FEATURE_TO_GGML_FLAG {
        if target_features
            .split(',')
            .any(|enabled| enabled == *feature)
        {
            config.define(flag, "ON");
        }
    }
    Ok(config.profile("Release").build())
}

fn emit_link_directives(lib_dir: &Path) -> Result<()> {
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    for archive in ["llama", "ggml", "ggml-cpu", "ggml-base"] {
        println!("cargo:rustc-link-lib=static={archive}");
    }
    // libstdc++ (libllama is C++) and libgomp (ggml's OpenMP runtime)
    // are linked statically — final binary has no .so dep on either.
    for archive in ["stdc++", openmp_archive_name()] {
        let path = static_archive(archive)?;
        let directory = path
            .parent()
            .with_context(|| format!("{} has no parent directory", path.display()))?;
        println!("cargo:rustc-link-search=native={}", directory.display());
        println!("cargo:rustc-link-lib=static={archive}");
    }
    // libgcc_s is suppressed via `-C link-arg=-static-libgcc` in the
    // workspace .cargo/config.toml; can't be done from a library
    // build.rs because rustc-link-arg only propagates to the package
    // that emits it (and dusk_llm is an rlib, not a bin).
    //
    // libpthread, libm, libdl are part of glibc; they remain dynamic.
    for library in ["pthread", "m", "dl"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    Ok(())
}

/// `gomp` for GCC, `omp` for Clang, decided by `$CC`'s `--version` output.
fn openmp_archive_name() -> &'static str {
    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    match Command::new(cc).arg("--version").output() {
        Ok(output) if String::from_utf8_lossy(&output.stdout).contains("clang") => "omp",
        _ => "gomp",
    }
}

/// Ask the C compiler where `lib<name>.a` lives. It echoes the input back
/// verbatim when it cannot find one.
fn static_archive(name: &str) -> Result<PathBuf> {
    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let archive = format!("lib{name}.a");
    let output = run(
        Command::new(&cc).arg(format!("-print-file-name={archive}")),
        &format!("`{cc} -print-file-name={archive}`"),
    )?;
    let reported = String::from_utf8(output.stdout)
        .with_context(|| format!("reading the path `{cc}` reported for {archive}"))?;
    let reported = reported.trim();
    if reported.is_empty() || reported == archive {
        bail!(
            "The compiler `{cc}` could not locate {archive}. Install it with your package manager and build again."
        );
    }
    Ok(PathBuf::from(reported))
}

fn compile_warmup_binary(
    llama_src: &Path,
    install_prefix: &Path,
    out_dir: &Path,
) -> Result<PathBuf> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("warmup/main.c");
    if !source.is_file() {
        bail!("The warm-up source {} is missing.", source.display());
    }
    let binary = out_dir.join("dusk_warmup");
    let cc = env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let openmp = openmp_archive_name();

    run(
        Command::new(&cc)
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-o"])
            .args([&binary, &source])
            .args([
                format!("-I{}/include", install_prefix.display()),
                format!("-I{}/ggml/include", llama_src.display()),
                format!("-L{}/lib", install_prefix.display()),
            ])
            // start-group lets the linker resolve circular static-archive
            // references between llama, ggml, libgomp, libstdc++, libgcc.
            .args([
                "-Wl,--start-group",
                "-lllama",
                "-lggml",
                "-lggml-cpu",
                "-lggml-base",
            ])
            .args([
                static_archive(openmp)?,
                static_archive("stdc++")?,
                static_archive("gcc")?,
            ])
            .args(["-Wl,--end-group", "-lpthread", "-lm", "-ldl"]),
        "Compiling the warm-up binary",
    )?;
    Ok(binary)
}

fn system_prompt(out_dir: &Path) -> Result<String> {
    let directory = out_dir
        .ancestors()
        .nth(4)
        .context("OUT_DIR has fewer than four ancestor directories, so cargo's layout has changed")?
        .join(".dusk_sh_entries");
    if !directory.exists() {
        bail!(
            "The shell-entry directory {} does not exist. It is written by the programs that declare an `sh_entry`, so no program emitted one.",
            directory.display()
        );
    }
    println!("cargo:rerun-if-changed={}", directory.display());

    let mut specs: Vec<ShEntrySpec> = Vec::new();
    for entry in
        fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let path = entry?.path();
        if path.extension() != Some("json".as_ref()) {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        specs.push(
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        );
    }
    specs.sort_by(|first, second| first.name.cmp(&second.name));

    let mut programs = String::new();
    for spec in &specs {
        programs += &format!("\n## {} — {}\n", spec.name, spec.short_description);
        let long = spec.long_description.trim();
        if !long.is_empty() {
            programs += long;
            programs.push('\n');
        }
    }
    Ok(
        include_str!("prompts/system.md")
            .replace("{{PROGRAMS}}", programs.trim_start_matches('\n')),
    )
}

fn run(command: &mut Command, what: &str) -> Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("invoking {what}"))?;
    if !output.status.success() {
        bail!(
            "{what} failed (exit {:?}).\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(output)
}
