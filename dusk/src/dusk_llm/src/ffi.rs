//! Raw FFI declarations for the subset of llama.cpp we call. Mirrors
//! `vendor/llama.cpp/include/llama.h` exactly — pinned to the version of
//! the submodule. Touch this only when the submodule moves.

#![allow(non_camel_case_types, non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::{c_char, c_float, c_int, c_void};

pub type llama_pos = i32;
pub type llama_token = i32;
pub type llama_seq_id = i32;

#[repr(C)]
pub struct llama_model {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct FILE {
    _opaque: [u8; 0],
}

// `whence` values for `fopencookie`'s seek callback.
pub const SEEK_SET: c_int = 0;
pub const SEEK_CUR: c_int = 1;
pub const SEEK_END: c_int = 2;

/// Glibc `cookie_io_functions_t` (the struct passed to `fopencookie`).
/// All four callbacks are optional; nulls mean "operation not supported".
#[repr(C)]
pub struct cookie_io_functions_t {
    pub read:
        Option<unsafe extern "C" fn(cookie: *mut c_void, buf: *mut c_char, size: usize) -> isize>,
    pub write:
        Option<unsafe extern "C" fn(cookie: *mut c_void, buf: *const c_char, size: usize) -> isize>,
    pub seek:
        Option<unsafe extern "C" fn(cookie: *mut c_void, offset: *mut i64, whence: c_int) -> c_int>,
    pub close: Option<unsafe extern "C" fn(cookie: *mut c_void) -> c_int>,
}

#[repr(C)]
pub struct llama_context {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct llama_vocab {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct llama_sampler {
    _opaque: [u8; 0],
}

pub const GGML_TYPE_Q8_0: c_int = 8;

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
pub enum ggml_log_level {
    GGML_LOG_LEVEL_NONE = 0,
    GGML_LOG_LEVEL_DEBUG = 1,
    GGML_LOG_LEVEL_INFO = 2,
    GGML_LOG_LEVEL_WARN = 3,
    GGML_LOG_LEVEL_ERROR = 4,
    GGML_LOG_LEVEL_CONT = 5,
}

pub type ggml_log_callback = Option<
    unsafe extern "C" fn(level: ggml_log_level, text: *const c_char, user_data: *mut c_void),
>;

pub type llama_progress_callback =
    Option<unsafe extern "C" fn(progress: c_float, user_data: *mut c_void) -> bool>;

pub type ggml_abort_callback = Option<unsafe extern "C" fn(data: *mut c_void) -> bool>;

pub type ggml_backend_sched_eval_callback =
    Option<unsafe extern "C" fn(tensor: *mut c_void, ask: bool, user_data: *mut c_void) -> bool>;

#[repr(C)]
pub struct llama_model_params {
    pub devices: *mut *mut c_void,
    pub tensor_buft_overrides: *const c_void,
    pub n_gpu_layers: i32,
    pub split_mode: c_int,
    pub main_gpu: i32,
    pub tensor_split: *const c_float,
    pub progress_callback: llama_progress_callback,
    pub progress_callback_user_data: *mut c_void,
    pub kv_overrides: *const c_void,
    pub vocab_only: bool,
    pub use_mmap: bool,
    pub use_direct_io: bool,
    pub use_mlock: bool,
    pub check_tensors: bool,
    pub use_extra_bufts: bool,
    pub no_host: bool,
    pub no_alloc: bool,
}

#[repr(C)]
pub struct llama_context_params {
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub n_seq_max: u32,
    pub n_rs_seq: u32,
    pub n_threads: i32,
    pub n_threads_batch: i32,
    pub ctx_type: c_int,
    pub rope_scaling_type: c_int,
    pub pooling_type: c_int,
    pub attention_type: c_int,
    pub flash_attn_type: c_int,
    pub rope_freq_base: c_float,
    pub rope_freq_scale: c_float,
    pub yarn_ext_factor: c_float,
    pub yarn_attn_factor: c_float,
    pub yarn_beta_fast: c_float,
    pub yarn_beta_slow: c_float,
    pub yarn_orig_ctx: u32,
    pub defrag_thold: c_float,
    pub cb_eval: ggml_backend_sched_eval_callback,
    pub cb_eval_user_data: *mut c_void,
    pub type_k: c_int,
    pub type_v: c_int,
    pub abort_callback: ggml_abort_callback,
    pub abort_callback_data: *mut c_void,
    pub embeddings: bool,
    pub offload_kqv: bool,
    pub no_perf: bool,
    pub op_offload: bool,
    pub swa_full: bool,
    pub kv_unified: bool,
    pub samplers: *mut c_void,
    pub n_samplers: usize,
}

#[repr(C)]
pub struct llama_sampler_chain_params {
    pub no_perf: bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct llama_batch {
    pub n_tokens: i32,
    pub token: *mut llama_token,
    pub embd: *mut c_float,
    pub pos: *mut llama_pos,
    pub n_seq_id: *mut i32,
    pub seq_id: *mut *mut llama_seq_id,
    pub logits: *mut i8,
}

unsafe extern "C" {
    pub fn llama_backend_init();

    pub fn llama_log_set(callback: ggml_log_callback, user_data: *mut c_void);

    pub fn llama_model_default_params() -> llama_model_params;
    pub fn llama_context_default_params() -> llama_context_params;
    pub fn llama_sampler_chain_default_params() -> llama_sampler_chain_params;

    pub fn llama_model_load_from_file_ptr(
        file: *mut FILE,
        params: llama_model_params,
    ) -> *mut llama_model;
    pub fn llama_model_free(model: *mut llama_model);
    pub fn llama_model_get_vocab(model: *const llama_model) -> *const llama_vocab;

    pub fn llama_init_from_model(
        model: *mut llama_model,
        params: llama_context_params,
    ) -> *mut llama_context;
    pub fn llama_free(ctx: *mut llama_context);

    pub fn llama_state_set_data(ctx: *mut llama_context, src: *const u8, size: usize) -> usize;

    pub fn llama_tokenize(
        vocab: *const llama_vocab,
        text: *const c_char,
        text_len: i32,
        tokens: *mut llama_token,
        n_tokens_max: i32,
        add_special: bool,
        parse_special: bool,
    ) -> i32;

    pub fn llama_token_to_piece(
        vocab: *const llama_vocab,
        token: llama_token,
        buf: *mut c_char,
        length: i32,
        lstrip: i32,
        special: bool,
    ) -> i32;

    pub fn llama_vocab_is_eog(vocab: *const llama_vocab, token: llama_token) -> bool;

    pub fn llama_batch_init(n_tokens: i32, embd: i32, n_seq_max: i32) -> llama_batch;
    pub fn llama_batch_free(batch: llama_batch);

    pub fn llama_decode(ctx: *mut llama_context, batch: llama_batch) -> i32;

    pub fn llama_sampler_chain_init(params: llama_sampler_chain_params) -> *mut llama_sampler;
    pub fn llama_sampler_chain_add(chain: *mut llama_sampler, smpl: *mut llama_sampler);
    pub fn llama_sampler_init_top_k(k: i32) -> *mut llama_sampler;
    pub fn llama_sampler_init_temp(t: f32) -> *mut llama_sampler;
    pub fn llama_sampler_init_dist(seed: u32) -> *mut llama_sampler;
    pub fn llama_sampler_sample(
        smpl: *mut llama_sampler,
        ctx: *mut llama_context,
        idx: i32,
    ) -> llama_token;
    pub fn llama_sampler_accept(smpl: *mut llama_sampler, token: llama_token);
    pub fn llama_sampler_free(smpl: *mut llama_sampler);
}

unsafe extern "C" {
    pub fn fopencookie(
        cookie: *mut c_void,
        mode: *const c_char,
        io_funcs: cookie_io_functions_t,
    ) -> *mut FILE;
    pub fn fclose(stream: *mut FILE) -> c_int;
    pub fn pread(fd: c_int, buf: *mut c_void, count: usize, offset: i64) -> isize;
    pub fn close(fd: c_int) -> c_int;
}
