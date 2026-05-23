use std::ffi::{c_char, c_int, c_void};
use std::fs::File;
use std::os::unix::io::IntoRawFd;
use std::ptr;

use dusk_program::anyhow::{Context, Result, anyhow, bail};
use object::Object;
use object::ObjectSection;
use object::read::ReadCache;

use crate::ffi::{
    FILE, GGML_TYPE_Q8_0, SEEK_CUR, SEEK_END, SEEK_SET, close, cookie_io_functions_t, fclose,
    fopencookie, llama_backend_init, llama_context, llama_context_default_params, llama_free,
    llama_init_from_model, llama_model, llama_model_default_params, llama_model_free,
    llama_model_load_from_file_ptr, llama_sampler, llama_sampler_chain_add,
    llama_sampler_chain_default_params, llama_sampler_chain_init, llama_sampler_free,
    llama_sampler_init_dist, llama_sampler_init_temp, llama_sampler_init_top_k,
    llama_state_set_data, pread,
};

/// KV-cache snapshot produced at build time by `dusk_warmup`.
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.state"));

// Embed the GGUF into the binary via `.incbin` in a NON-ALLOC section
// flagged SHF_GNU_RETAIN. Three properties make this work where
// `include_bytes!` cannot:
//   1. No SHF_ALLOC bit, so no PHDR maps it. The bytes exist in the ELF
//      file but never receive a virtual address — they don't sit in
//      `.rodata` and don't displace small-model PC32 relocs in `.text`.
//   2. Nothing in Rust references the symbol, so no reloc to the embed
//      is emitted — its size is irrelevant to the link.
//   3. The `R` flag (SHF_GNU_RETAIN) keeps the section through
//      `--gc-sections` even with no references.
// Runtime still loads the model with `llama_model_load_from_file_ptr`
// against a cookie stream that reads from this section.
core::arch::global_asm!(concat!(
    ".section .dusk_embedded_model, \"R\", @progbits\n",
    ".global dusk_embedded_model_start\n",
    "dusk_embedded_model_start:\n",
    ".incbin \"",
    env!("DUSK_MODEL_GGUF_PATH"),
    "\"\n",
    "dusk_embedded_model_end:\n",
    ".global dusk_embedded_model_end\n",
));

const EMBEDDED_GGUF_SECTION: &str = ".dusk_embedded_model";

// llama.cpp session file header: u32 magic, u32 version, u32 n_tokens,
// llama_token[n_tokens], then raw `llama_state_get_data` bytes. Pinned to
// the submodule revision (vendor/llama.cpp, tag b9282).
const LLAMA_SESSION_MAGIC: u32 = 0x6767736e; // 'ggsn'
const LLAMA_SESSION_VERSION: u32 = 9;

// Must match build.rs (the warmup binary builds the KV cache with these).
const CONTEXT_TOKENS: u32 = 16_384;
const KV_CACHE_TYPE: c_int = GGML_TYPE_Q8_0;

// Sampler chain: top-K narrows the candidate set, temperature rescales,
// dist samples from the resulting distribution.
const SAMPLER_TOP_K: i32 = 20;
const SAMPLER_TEMPERATURE: f32 = 0.6;
// LLAMA_DEFAULT_SEED in llama.h — pick a fresh random seed at init.
const SAMPLER_SEED: u32 = 0xFFFF_FFFF;

pub(crate) struct LoadedLlm {
    pub(crate) context: *mut llama_context,
    pub(crate) model: *mut llama_model,
    pub(crate) sampler: *mut llama_sampler,
    pub(crate) next_position: i32,
    pub(crate) had_first_chat: bool,
}

// SAFETY: llama.cpp has no thread-affinity state — only concurrent access
// is unsound. The `Mutex<Option<LoadedLlm>>` in `Llm` serialises every access.
unsafe impl Send for LoadedLlm {}

impl Drop for LoadedLlm {
    fn drop(&mut self) {
        // Reverse construction order: sampler refs context, context refs model.
        unsafe {
            llama_sampler_free(self.sampler);
            llama_free(self.context);
            llama_model_free(self.model);
        }
    }
}

/// Load the embedded GGUF, build a context, and apply the build-time
/// KV-cache snapshot. Blocks — call from `spawn_blocking`.
pub(crate) fn load() -> Result<LoadedLlm> {
    // SAFETY: idempotent.
    unsafe { llama_backend_init() };

    let (snapshot_tokens, raw_state) = parse_snapshot(SNAPSHOT)?;

    // The GGUF lives in our own ELF and is read via `fopencookie`
    // (no fd → llama's mmap path is disabled; reads land in our
    // callbacks).
    let embedded = open_embedded_gguf()?;
    let mut model_params = unsafe { llama_model_default_params() };
    model_params.use_mmap = false;
    let model = unsafe { llama_model_load_from_file_ptr(embedded.file, model_params) };
    if model.is_null() {
        bail!("llama_model_load_from_file_ptr returned null for embedded GGUF");
    }

    let context = match build_context(model) {
        Ok(context) => context,
        Err(error) => {
            unsafe { llama_model_free(model) };
            return Err(error);
        }
    };

    let written = unsafe { llama_state_set_data(context, raw_state.as_ptr(), raw_state.len()) };
    if written != raw_state.len() {
        unsafe { llama_free(context) };
        unsafe { llama_model_free(model) };
        bail!(
            "snapshot load consumed {written} bytes, expected {}",
            raw_state.len()
        );
    }

    let sampler = match build_sampler() {
        Ok(sampler) => sampler,
        Err(error) => {
            unsafe { llama_free(context) };
            unsafe { llama_model_free(model) };
            return Err(error);
        }
    };

    Ok(LoadedLlm {
        context,
        model,
        sampler,
        next_position: snapshot_tokens,
        had_first_chat: false,
    })
}

/// Parse a `llama_state_save_file` session file. Returns `(n_tokens, raw_kv_bytes)`.
fn parse_snapshot(bytes: &[u8]) -> Result<(i32, &[u8])> {
    if bytes.len() < 12 {
        bail!("snapshot truncated: {} bytes < 12-byte header", bytes.len());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let n_tokens = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if magic != LLAMA_SESSION_MAGIC {
        bail!("snapshot magic mismatch: got {magic:#010x}, expected {LLAMA_SESSION_MAGIC:#010x}");
    }
    if version != LLAMA_SESSION_VERSION {
        bail!(
            "snapshot version mismatch: got {version}, expected {LLAMA_SESSION_VERSION} \
             (vendor/llama.cpp moved out of sync with this build)"
        );
    }
    let tokens_byte_len = (n_tokens as usize)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("snapshot token count overflows usize: {n_tokens}"))?;
    let tail_start = 12usize
        .checked_add(tokens_byte_len)
        .ok_or_else(|| anyhow!("snapshot header arithmetic overflowed"))?;
    if bytes.len() < tail_start {
        bail!(
            "snapshot truncated: header claims {n_tokens} tokens but only {} bytes available",
            bytes.len() - 12
        );
    }
    let n_tokens_i32: i32 = n_tokens
        .try_into()
        .context("snapshot token count overflows i32")?;
    Ok((n_tokens_i32, &bytes[tail_start..]))
}

fn build_context(model: *mut llama_model) -> Result<*mut llama_context> {
    let threads = inference_threads();
    let mut params = unsafe { llama_context_default_params() };
    params.n_ctx = CONTEXT_TOKENS;
    params.n_threads = threads;
    params.n_threads_batch = threads;
    params.type_k = KV_CACHE_TYPE;
    params.type_v = KV_CACHE_TYPE;

    let context = unsafe { llama_init_from_model(model, params) };
    if context.is_null() {
        bail!("llama_init_from_model returned null");
    }
    Ok(context)
}

fn build_sampler() -> Result<*mut llama_sampler> {
    let params = unsafe { llama_sampler_chain_default_params() };
    let chain = unsafe { llama_sampler_chain_init(params) };
    if chain.is_null() {
        bail!("llama_sampler_chain_init returned null");
    }
    let stages: [(*mut llama_sampler, &str); 3] = unsafe {
        [
            (
                llama_sampler_init_top_k(SAMPLER_TOP_K),
                "llama_sampler_init_top_k",
            ),
            (
                llama_sampler_init_temp(SAMPLER_TEMPERATURE),
                "llama_sampler_init_temp",
            ),
            (
                llama_sampler_init_dist(SAMPLER_SEED),
                "llama_sampler_init_dist",
            ),
        ]
    };
    for (stage, name) in stages {
        if stage.is_null() {
            unsafe { llama_sampler_free(chain) };
            bail!("{name} returned null");
        }
        unsafe { llama_sampler_chain_add(chain, stage) };
    }
    Ok(chain)
}

/// Honour `DUSK_LLM_THREADS_COUNT` if it's a positive integer; otherwise
/// fall back to host parallelism, then 1.
fn inference_threads() -> i32 {
    if let Ok(value) = std::env::var("DUSK_LLM_THREADS_COUNT")
        && let Ok(parsed) = value.parse::<i32>()
        && parsed > 0
    {
        return parsed;
    }
    std::thread::available_parallelism()
        .ok()
        .and_then(|count| i32::try_from(count.get()).ok())
        .unwrap_or(1)
}

// ---------------------------------------------------------------------------
// Embedded GGUF: glibc cookie stream over our own `.dusk_embedded_model`
// ELF section. llama.cpp sees a 2.9 GB `FILE*` whose offset 0 is the start
// of the GGUF.
// ---------------------------------------------------------------------------

#[repr(C)]
struct GgufCookie {
    fd: c_int,
    base: i64,
    size: i64,
    position: i64,
}

struct EmbeddedGgufFile {
    file: *mut FILE,
}

impl Drop for EmbeddedGgufFile {
    fn drop(&mut self) {
        if !self.file.is_null() {
            unsafe { fclose(self.file) };
            self.file = ptr::null_mut();
        }
    }
}

fn open_embedded_gguf() -> Result<EmbeddedGgufFile> {
    let exe = File::open("/proc/self/exe").context("opening /proc/self/exe")?;
    let (offset, size) = {
        let cache = ReadCache::new(&exe);
        let elf = object::read::elf::ElfFile64::<object::Endianness, _>::parse(&cache)
            .context("parsing /proc/self/exe as ELF64")?;
        let section = elf.section_by_name(EMBEDDED_GGUF_SECTION).ok_or_else(|| {
            anyhow!(
                "section `{EMBEDDED_GGUF_SECTION}` not found in /proc/self/exe — \
                 was the .incbin embed stripped?"
            )
        })?;
        section
            .file_range()
            .ok_or_else(|| anyhow!("section `{EMBEDDED_GGUF_SECTION}` has no file range"))?
    };
    if size == 0 {
        bail!("embedded GGUF section `{EMBEDDED_GGUF_SECTION}` is empty");
    }

    let fd = exe.into_raw_fd();
    let cookie = Box::into_raw(Box::new(GgufCookie {
        fd,
        base: offset as i64,
        size: size as i64,
        position: 0,
    }));

    let io_funcs = cookie_io_functions_t {
        read: Some(gguf_cookie_read),
        write: None,
        seek: Some(gguf_cookie_seek),
        close: Some(gguf_cookie_close),
    };

    let file = unsafe { fopencookie(cookie as *mut c_void, c"rb".as_ptr(), io_funcs) };
    if file.is_null() {
        let cookie = unsafe { Box::from_raw(cookie) };
        unsafe { close(cookie.fd) };
        bail!("fopencookie returned null for embedded GGUF");
    }
    Ok(EmbeddedGgufFile { file })
}

unsafe extern "C" fn gguf_cookie_read(
    cookie: *mut c_void,
    buf: *mut c_char,
    requested: usize,
) -> isize {
    let cookie = unsafe { &mut *(cookie as *mut GgufCookie) };
    let remaining = (cookie.size - cookie.position).max(0) as usize;
    let to_read = requested.min(remaining);
    if to_read == 0 {
        return 0;
    }
    let got = unsafe {
        pread(
            cookie.fd,
            buf as *mut c_void,
            to_read,
            cookie.base + cookie.position,
        )
    };
    if got < 0 {
        return -1;
    }
    cookie.position += got as i64;
    got
}

unsafe extern "C" fn gguf_cookie_seek(
    cookie: *mut c_void,
    offset: *mut i64,
    whence: c_int,
) -> c_int {
    let cookie = unsafe { &mut *(cookie as *mut GgufCookie) };
    let requested = unsafe { *offset };
    let new_position = match whence {
        SEEK_SET => requested,
        SEEK_CUR => cookie.position + requested,
        SEEK_END => cookie.size + requested,
        _ => return -1,
    };
    if new_position < 0 || new_position > cookie.size {
        return -1;
    }
    cookie.position = new_position;
    // glibc expects the new absolute position written back.
    unsafe { *offset = new_position };
    0
}

unsafe extern "C" fn gguf_cookie_close(cookie: *mut c_void) -> c_int {
    let cookie = unsafe { Box::from_raw(cookie as *mut GgufCookie) };
    unsafe { close(cookie.fd) }
}
