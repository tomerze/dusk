use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};

use dusk_program::anyhow::{Context, Result, anyhow, bail};
use object::Object;
use object::ObjectSection;
use object::read::ReadCache;

// Embed the GGUF into the binary via `.incbin` in a NON-ALLOC section.
// `build.rs` sets `DUSK_LLM_MODEL_PATH` from the `directory` and `file` in
// `model.json`, which is also where it downloaded and verified those bytes.
core::arch::global_asm!(concat!(
    ".section .llm_gguf, \"R\", @progbits\n",
    ".global llm_gguf_start\n",
    "llm_gguf_start:\n",
    ".incbin \"",
    env!("DUSK_LLM_MODEL_PATH"),
    "\"\n",
    "llm_gguf_end:\n",
    ".global llm_gguf_end\n",
));

pub const GEMMA4E2B_EMBEDDED_GGUF_SECTION: &str = ".llm_gguf";

/// `sendfile` refuses counts above this, so the copy runs in chunks.
const SENDFILE_MAXIMUM_COUNT: u64 = 0x7fff_f000;

/// The embedded GGUF, republished as an anonymous in-memory file that
/// ik_llama.cpp can open by path.
pub struct EmbeddedGgufFile {
    file: File,
}

impl EmbeddedGgufFile {
    /// The `/proc/self/fd` path of the anonymous file, for
    /// `llama_model_load_from_file`.
    pub(crate) fn path(&self) -> CString {
        CString::new(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
            .expect("a formatted file descriptor number holds no interior nul")
    }
}

/// Copy a GGUF embedded in this executable's own ELF as a named section into
/// an anonymous file, and hand back that file.
///
/// The copy exists because ik_llama.cpp loads a model only from a path it
/// opens itself — it has no counterpart to stock llama.cpp's
/// `llama_model_load_from_file_ptr`, which took an already-open stream. A
/// `memfd` is the only path-addressable file whose first byte can be the
/// GGUF's first byte, since the bytes sit at an offset inside the
/// executable. ik_llama.cpp then `mmap`s it, so the model is resident once.
pub fn load_from_self_exe_section(section_name: &str) -> Result<EmbeddedGgufFile> {
    let executable = File::open("/proc/self/exe").context("opening /proc/self/exe")?;
    let (offset, size) = {
        let cache = ReadCache::new(&executable);
        let elf = object::read::elf::ElfFile64::<object::Endianness, _>::parse(&cache)
            .context("parsing /proc/self/exe as ELF64")?;
        let section = elf.section_by_name(section_name).ok_or_else(|| {
            anyhow!(
                "section `{section_name}` not found in /proc/self/exe — \
                 was the .incbin embed stripped?"
            )
        })?;
        section
            .file_range()
            .ok_or_else(|| anyhow!("section `{section_name}` has no file range"))?
    };
    if size == 0 {
        bail!("embedded GGUF section `{section_name}` is empty");
    }

    let descriptor = unsafe { libc::memfd_create(c"dusk_llm_gguf".as_ptr(), libc::MFD_CLOEXEC) };
    if descriptor < 0 {
        bail!(
            "memfd_create for the embedded GGUF failed: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: `memfd_create` just returned this descriptor and nothing else
    // owns it.
    let gguf = unsafe { File::from_raw_fd(descriptor) };

    let mut source_offset = i64::try_from(offset).context("the section offset overflows off_t")?;
    let mut remaining = size;
    while remaining > 0 {
        let copied = unsafe {
            libc::sendfile(
                gguf.as_raw_fd(),
                executable.as_raw_fd(),
                &mut source_offset,
                remaining.min(SENDFILE_MAXIMUM_COUNT) as usize,
            )
        };
        if copied <= 0 {
            bail!(
                "copying the embedded GGUF into memory stopped {remaining} bytes short: {}",
                std::io::Error::last_os_error()
            );
        }
        remaining -= copied as u64;
    }

    Ok(EmbeddedGgufFile { file: gguf })
}
