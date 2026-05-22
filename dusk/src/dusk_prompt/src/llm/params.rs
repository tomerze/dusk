use llama_cpp_2::context::params::KvCacheType;

pub(crate) const CONTEXT_TOKENS: u32 = 4_096;
pub(crate) const KV_CACHE_TYPE: KvCacheType = KvCacheType::Q8_0;
