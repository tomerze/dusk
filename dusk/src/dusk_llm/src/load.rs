use std::ffi::{c_char, c_int, c_void};
use std::fs::File;
use std::os::unix::io::IntoRawFd;
use std::ptr;

use dusk_program::anyhow::{Context, Result, anyhow, bail};
use object::Object;
use object::ObjectSection;
use object::read::ReadCache;

use crate::ffi::{
    FILE, SEEK_CUR, SEEK_END, SEEK_SET, close, cookie_io_functions_t, fclose, fopencookie,
};

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

struct GgufCookie {
    /// Base of the `mmap`ed region (page-aligned) and its length, kept for
    /// `munmap` at close.
    mapping: *mut c_void,
    mapping_len: usize,
    /// Start of the GGUF bytes within the mapping (`mapping + alignment delta`).
    data: *const u8,
    size: i64,
    position: i64,
}

pub struct EmbeddedGgufFile {
    pub(crate) file: *mut FILE,
}

impl Drop for EmbeddedGgufFile {
    fn drop(&mut self) {
        if !self.file.is_null() {
            unsafe { fclose(self.file) };
            self.file = ptr::null_mut();
        }
    }
}

/// Open a GGUF embedded in this executable's own ELF as a named section,
/// returning a glibc `fopencookie` stream over it. The section is `mmap`ed
/// once (with `use_mmap=false` on the model so llama reads through this
/// stream); cookie reads then `memcpy` from the mapping instead of issuing
/// a syscall per read.
pub fn load_from_self_exe_section(section_name: &str) -> Result<EmbeddedGgufFile> {
    let exe = File::open("/proc/self/exe").context("opening /proc/self/exe")?;
    let (offset, size) = {
        let cache = ReadCache::new(&exe);
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

    // `mmap` demands a page-aligned file offset, so map from the aligned
    // offset and skip the leading `delta` bytes to reach the section.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        bail!("sysconf(_SC_PAGESIZE) returned {page_size}");
    }
    let page_size = page_size as i64;
    let aligned_offset = offset as i64 & !(page_size - 1);
    let delta = offset as i64 - aligned_offset;
    let mapping_len = (size + delta as u64) as usize;

    let fd = exe.into_raw_fd();
    let mapping = unsafe {
        libc::mmap(
            ptr::null_mut(),
            mapping_len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            fd,
            aligned_offset,
        )
    };
    // The mapping holds its own reference to the file; the fd is no longer needed.
    unsafe { close(fd) };
    if mapping == libc::MAP_FAILED {
        bail!("mmap of embedded GGUF section `{section_name}` failed");
    }

    // Kick off async readahead so llama's reads hit warm page cache. Advisory
    // only — it populates the page cache (no second copy), and a failure just
    // means no prefetch.
    let advised = unsafe { libc::madvise(mapping, mapping_len, libc::MADV_WILLNEED) };
    if advised != 0 {
        tracing::warn!("madvise(MADV_WILLNEED) on embedded GGUF failed");
    }

    let cookie = Box::into_raw(Box::new(GgufCookie {
        mapping,
        mapping_len,
        data: unsafe { (mapping as *const u8).add(delta as usize) },
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
        unsafe { libc::munmap(cookie.mapping, cookie.mapping_len) };
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
    unsafe {
        ptr::copy_nonoverlapping(
            cookie.data.add(cookie.position as usize),
            buf as *mut u8,
            to_read,
        );
    }
    cookie.position += to_read as i64;
    to_read as isize
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
    unsafe { libc::munmap(cookie.mapping, cookie.mapping_len) }
}
