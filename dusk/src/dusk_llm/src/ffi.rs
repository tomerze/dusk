//! Raw FFI declarations for the subset of ik_llama.cpp we call. Mirrors
//! `vendor/ik_llama.cpp/include/llama.h` exactly — pinned to the version of
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
pub struct llama_context {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct llama_vocab {
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
    pub devices: *const c_char,
    pub n_gpu_layers: i32,
    pub mla: i32,
    pub split_mode: c_int,
    pub main_gpu: i32,
    pub max_gpu: i32,
    pub ncmoe: i32,
    pub type_k: c_int,
    pub type_v: c_int,
    pub idx_type_k: c_int,
    pub max_ctx_size: u32,
    pub n_seq_max: i32,
    pub n_ubatch: i32,
    pub amb: i32,
    pub fit_margin: i32,
    pub fit: bool,
    pub worst_graph_tokens: i32,
    pub type_k_first: c_int,
    pub type_k_last: c_int,
    pub type_v_first: c_int,
    pub type_v_last: c_int,
    pub n_k_first: i32,
    pub n_k_last: i32,
    pub n_v_first: i32,
    pub n_v_last: i32,
    pub extra_output_type: c_int,
    pub tensor_split: *const c_float,
    pub fit_margin_array: *const c_int,
    pub rpc_servers: *const c_char,
    pub progress_callback: llama_progress_callback,
    pub progress_callback_user_data: *mut c_void,
    pub kv_overrides: *const c_void,
    pub tensor_buft_overrides: *const c_void,
    pub vocab_only: bool,
    pub use_mmap: bool,
    pub use_mlock: bool,
    pub check_tensors: bool,
    pub repack_tensors: bool,
    pub use_thp: bool,
    pub validate_quants: bool,
    pub merge_qkv: bool,
    pub merge_up_gate_exps: bool,
    pub mtp: bool,
    pub dry_run: bool,
    pub flash_attn: bool,
    pub defer_experts: bool,
    pub swa_compress: bool,
}

#[repr(C)]
pub struct llama_context_params {
    pub seed: u32,
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub n_seq_max: u32,
    pub n_threads: u32,
    pub n_threads_batch: u32,
    pub max_extra_alloc: i32,
    pub worst_case_tokens: i32,
    pub rope_scaling_type: c_int,
    pub pooling_type: c_int,
    pub attention_type: c_int,
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
    pub idx_type_k: c_int,
    pub type_reduce: c_int,
    pub type_graph_attn: c_int,
    pub type_k_first: c_int,
    pub type_k_last: c_int,
    pub type_v_first: c_int,
    pub type_v_last: c_int,
    pub n_k_first: i32,
    pub n_k_last: i32,
    pub n_v_first: i32,
    pub n_v_last: i32,
    pub logits_all: bool,
    pub embeddings: bool,
    pub offload_kqv: bool,
    pub flash_attn: bool,
    pub mla_attn: c_int,
    pub attn_max_batch: c_int,
    pub fused_moe_up_gate: bool,
    pub grouped_expert_routing: bool,
    pub fused_up_gate: bool,
    pub fused_mmad: bool,
    pub rope_cache: bool,
    pub graph_reuse: bool,
    pub dsa: bool,
    pub fused_idx_topk: bool,
    pub swa_compress: bool,
    pub dsa_top_k: c_int,
    pub min_experts: c_int,
    pub thresh_experts: c_float,
    pub only_active_experts: bool,
    pub prefetch_experts: bool,
    pub prefetch_experts_threads: c_int,
    pub k_cache_hadamard: bool,
    pub v_cache_hadamard: bool,
    pub split_mode_graph_scheduling: bool,
    pub scheduler_async: bool,
    pub mtp: bool,
    pub mtp_op_type: c_int,
    pub abort_callback: ggml_abort_callback,
    pub abort_callback_data: *mut c_void,
    pub offload_policy: *mut c_void,
    pub cuda_params: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct llama_token_data {
    pub id: llama_token,
    pub logit: c_float,
    pub p: c_float,
}

#[repr(C)]
pub struct llama_token_data_array {
    pub data: *mut llama_token_data,
    pub size: usize,
    /// Index into `data` (not a token id) of the token the last sampler picked.
    pub selected: i64,
    pub sorted: bool,
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
    pub all_pos_0: llama_pos,
    pub all_pos_1: llama_pos,
    pub all_seq_id: llama_seq_id,
}

unsafe extern "C" {
    pub fn llama_backend_init();

    pub fn llama_log_set(callback: ggml_log_callback, user_data: *mut c_void);

    pub fn llama_model_default_params() -> llama_model_params;
    pub fn llama_context_default_params() -> llama_context_params;

    pub fn llama_model_load_from_file(
        path_model: *const c_char,
        params: llama_model_params,
    ) -> *mut llama_model;
    pub fn llama_free_model(model: *mut llama_model);
    pub fn llama_model_get_vocab(model: *const llama_model) -> *const llama_vocab;
    pub fn llama_n_vocab(model: *const llama_model) -> i32;

    pub fn llama_init_from_model(
        model: *mut llama_model,
        params: llama_context_params,
    ) -> *mut llama_context;
    pub fn llama_free(ctx: *mut llama_context);

    pub fn llama_state_set_data(ctx: *mut llama_context, src: *const u8, size: usize) -> usize;

    pub fn llama_vocab_tokenize(
        vocab: *const llama_vocab,
        text: *const c_char,
        text_len: i32,
        tokens: *mut llama_token,
        n_tokens_max: i32,
        add_special: bool,
        parse_special: bool,
    ) -> i32;

    pub fn llama_token_to_piece_vocab(
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

    pub fn llama_get_logits_ith(ctx: *mut llama_context, i: i32) -> *mut c_float;

    pub fn llama_sample_top_k(
        ctx: *mut llama_context,
        candidates: *mut llama_token_data_array,
        k: i32,
        min_keep: usize,
    );
    pub fn llama_sample_temp(
        ctx: *mut llama_context,
        candidates: *mut llama_token_data_array,
        temp: c_float,
    );
    pub fn llama_sample_token(
        ctx: *mut llama_context,
        candidates: *mut llama_token_data_array,
    ) -> llama_token;
}
