# Stage 1 — SH_ENTRIES as Cargo.toml metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move ShEntry data (name, short_description, long_description, impl_name) out of each program crate's source code and into its Cargo.toml under `[[package.metadata.dusk.sh_entry]]`. Auto-generate the `#[distributed_slice(SH_ENTRIES)]` registration functions via a new `dusk_program_proc::sh_entries!()` proc macro that parses the crate's own Cargo.toml at compile time.

**Architecture:** Cargo.toml is the single source of truth for the catalogue. The proc macro reads `CARGO_MANIFEST_DIR/Cargo.toml` during its expansion, parses the `[[package.metadata.dusk.sh_entry]]` array (zero or more entries), and emits one `#[linkme::distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]` function per entry. Each function constructs an `ShEntry` with `program_id: Some(PROGRAM_ID)` (the crate-level const set by the existing `metadata!` macro), `version: VERSION`, the literal name/description strings from the metadata, and `program_args_builder: Rc::new(<impl_name>::default())`. The named `impl_name` must be a type in the crate's source that implements `ProgramArgsBuilder + Default`.

**Tech Stack:** Rust 2024 edition, `proc-macro2`, `quote`, `syn`, `toml` (proc-macro-side parsing), `serde`, `linkme`.

**Source of truth for current behaviour:** `programs/date/src/client.rs:98-116` shows the hand-written `sh_entry()` function and the `DateProgramArgsBuilder` struct we're going to drive from metadata. `dusk/src/dusk_program_proc/src/lib.rs` is where the new proc macro lives.

---

## File Structure

**Created:**
- (none — proc macro is added to an existing file)

**Modified:**
- `dusk/src/dusk_program_proc/Cargo.toml` — add `toml` + `serde` build-dependencies (proc macros are build-time crates; their deps are normal `[dependencies]` here).
- `dusk/src/dusk_program_proc/src/lib.rs` — add the `sh_entries!()` proc macro.
- `programs/<each>/Cargo.toml` — add `[[package.metadata.dusk.sh_entry]]` entries.
- `programs/<each>/src/client.rs` — derive `Default` on the args-builder type and replace the hand-written `#[distributed_slice(...)] pub fn sh_entry() -> ShEntry { ... }` block with `dusk_program_proc::sh_entries!();`.

**Programs to migrate (one task each):** `ps`, `kill`, `sleep`, `date`, `true`, `false`, `sh`, `init`. (Built-ins `clear`, `exit`, `help`, `functions` in `dusk_prompt::builtins` are not in `SH_ENTRIES` — they stay hand-written.)

---

## Task 1: Add `toml` parsing to `dusk_program_proc`

**Files:**
- Modify: `dusk/src/dusk_program_proc/Cargo.toml`

- [ ] **Step 1: Add the deps**

Append to `[dependencies]` (proc-macro crates use plain `[dependencies]`, not `[build-dependencies]`):

```toml
toml = "0.8"
serde = { version = "1", features = ["derive"] }
```

- [ ] **Step 2: Verify the proc-macro crate still compiles**

```bash
cargo check -p dusk_program_proc
```

Expected: `Finished` with no errors.

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_program_proc/Cargo.toml
git commit -m "dusk_program_proc: add toml + serde deps for sh_entries! macro"
```

---

## Task 2: Implement the `sh_entries!()` proc macro

**Files:**
- Modify: `dusk/src/dusk_program_proc/src/lib.rs`

- [ ] **Step 1: Add the macro at the end of `dusk/src/dusk_program_proc/src/lib.rs`**

Append (do not replace existing macros):

```rust
use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
struct ShEntrySpec {
    name: String,
    short_description: String,
    #[serde(default)]
    long_description: String,
    impl_name: String,
}

#[derive(serde::Deserialize)]
struct CratePackage {
    metadata: Option<CrateMetadata>,
}

#[derive(serde::Deserialize)]
struct CrateMetadata {
    dusk: Option<DuskMetadata>,
}

#[derive(serde::Deserialize)]
struct DuskMetadata {
    #[serde(default)]
    sh_entry: Vec<ShEntrySpec>,
}

#[derive(serde::Deserialize)]
struct CrateManifest {
    package: CratePackage,
}

/// Reads the consumer crate's `Cargo.toml` and emits one
/// `#[linkme::distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]`
/// function per entry under `[[package.metadata.dusk.sh_entry]]`.
///
/// Each entry requires `name`, `short_description`, `impl_name` (a type
/// in the crate's source that implements `ProgramArgsBuilder + Default`).
/// `long_description` is optional. The generated function name is
/// `sh_entry_<sanitised_name>`.
#[proc_macro]
pub fn sh_entries(_item: TokenStream) -> TokenStream {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set during proc-macro expansion");
    let manifest_path = std::path::Path::new(&manifest_dir).join("Cargo.toml");

    let manifest_text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "sh_entries! could not read {}: {error}",
                    manifest_path.display()
                ),
            )
            .to_compile_error()
            .into();
        }
    };

    let manifest: CrateManifest = match toml::from_str(&manifest_text) {
        Ok(value) => value,
        Err(error) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("sh_entries! failed to parse Cargo.toml: {error}"),
            )
            .to_compile_error()
            .into();
        }
    };

    let entries = manifest
        .package
        .metadata
        .and_then(|m| m.dusk)
        .map(|d| d.sh_entry)
        .unwrap_or_default();

    let mut tokens = proc_macro2::TokenStream::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for entry in entries {
        if seen.insert(entry.name.clone(), ()).is_some() {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("sh_entries!: duplicate entry name `{}`", entry.name),
            )
            .to_compile_error()
            .into();
        }

        let fn_ident = quote::format_ident!(
            "sh_entry_{}",
            entry.name.replace(|c: char| !c.is_alphanumeric(), "_"),
        );
        let name_lit = &entry.name;
        let short_lit = &entry.short_description;
        let long_lit = &entry.long_description;
        let impl_ident: syn::Path = syn::parse_str(&entry.impl_name).unwrap_or_else(|error| {
            panic!("sh_entries!: impl_name `{}` is not a valid Rust path: {error}", entry.impl_name)
        });

        tokens.extend(quote::quote! {
            #[linkme::distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
            pub fn #fn_ident() -> dusk_program_sh::entry::ShEntry {
                dusk_program_sh::entry::ShEntry {
                    info: dusk_program_sh::entry::ProgramInfo {
                        program_id: Some(PROGRAM_ID),
                        name: #name_lit,
                        short_description: #short_lit,
                        long_description: #long_lit,
                        version: VERSION,
                    },
                    program_args_builder: std::rc::Rc::new(<#impl_ident as ::core::default::Default>::default()),
                }
            }
        });
    }

    tokens.into()
}
```

- [ ] **Step 2: Verify the proc-macro crate still compiles**

```bash
cargo check -p dusk_program_proc
```

Expected: `Finished` with no errors. (The macro is not invoked yet, so existing call sites still work.)

- [ ] **Step 3: Commit**

```bash
git add dusk/src/dusk_program_proc/src/lib.rs
git commit -m "dusk_program_proc: add sh_entries! macro that reads Cargo.toml metadata"
```

---

## Task 3: Migrate `programs/date`

**Files:**
- Modify: `programs/date/Cargo.toml`
- Modify: `programs/date/src/client.rs`

- [ ] **Step 1: Add the metadata block to `programs/date/Cargo.toml`**

Insert immediately above `[dependencies]`:

```toml
[[package.metadata.dusk.sh_entry]]
name = "date"
short_description = "show or set the current time"
long_description = """
Show or set the current time.
* Use `date` to print the current time in the format "`YYYY-MM-DD HH:MM:SS` (UTC)".
* Use `date -s "YYYY-MM-DD HH:MM:SS"` to set the clock to that UTC timestamp.
* Use `date --ntp <HOST[:PORT]>` (for example `pool.ntp.org`) to query an NTP server and set the clock to its reported time. Port defaults to 123.
* Use `date --sync` to copy the wall clock from the client and set the server clock to match it.
"""
impl_name = "DateProgramArgsBuilder"
```

- [ ] **Step 2: Derive `Default` on the args-builder in `programs/date/src/client.rs`**

Find the line:

```rust
struct DateProgramArgsBuilder {}
```

Replace with:

```rust
#[derive(Default)]
struct DateProgramArgsBuilder {}
```

- [ ] **Step 3: Replace the hand-written `sh_entry` function with the macro**

Replace the block at `programs/date/src/client.rs:98-116`:

```rust
#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(date_capnp::PROGRAM_ID),
            name: "date",
            short_description: "show or set the current time",
            long_description: r#"
Show or set the current time.
* Use `date` to print the current time in the format "`YYYY-MM-DD HH:MM:SS` (UTC)".
* Use `date -s "YYYY-MM-DD HH:MM:SS"` to set the clock to that UTC timestamp.
* Use `date --ntp <HOST[:PORT]>` (for example `pool.ntp.org`) to query an NTP server and set the clock to its reported time. Port defaults to 123.
* Use `date --sync` to copy the wall clock from the client and set the server clock to match it.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(DateProgramArgsBuilder {}),
    }
}
```

with:

```rust
dusk_program_proc::sh_entries!();
```

- [ ] **Step 4: Remove the now-unused imports**

At the top of `programs/date/src/client.rs`, delete the import lines that become unused:

- `use linkme::distributed_slice;` (the macro emits the attribute itself).
- `use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};` — keep `ProgramArgsBuilder` (still used by the `impl ProgramArgsBuilder for DateProgramArgsBuilder`), drop `ProgramInfo` and `ShEntry`.
- `use std::rc::Rc;` — keep if still used by `ProgramArgs` (check `as_program_args` return). Keep if any remaining `Rc::` reference exists in the file; otherwise remove.

Verify by running `cargo check -p dusk_program_date --features client` and pruning each unused-import warning one at a time. Expected final import block:

```rust
extern crate linkme;

use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::ProgramArgsBuilder;
use std::net::{SocketAddr, ToSocketAddrs};
use std::rc::Rc;
```

Notes:
- `ProgramArgsBuilder` (trait) is still needed for `impl ProgramArgsBuilder for DateProgramArgsBuilder`.
- `ProgramInfo` and `ShEntry` are no longer named in the source — the macro emits the fully-qualified paths — so drop them from the `use` statement.
- `linkme::distributed_slice` is no longer named in the source — drop it too. The `extern crate linkme;` line stays because the macro-generated code references `linkme::distributed_slice`, which requires the crate to be linked.
- `std::rc::Rc` is needed inside `impl ProgramArgsBuilder for DateProgramArgsBuilder` (the `Rc<ProgramArgs>` return type of `build`). Keep it.

- [ ] **Step 5: Verify the crate builds**

```bash
cargo check -p dusk_program_date --features client
```

Expected: `Finished` with no errors, no warnings about unused imports.

- [ ] **Step 6: Commit**

```bash
git add programs/date/Cargo.toml programs/date/src/client.rs
git commit -m "dusk_program_date: drive sh_entry from Cargo.toml metadata"
```

---

## Task 4: Migrate `programs/ps`

**Files:**
- Modify: `programs/ps/Cargo.toml`
- Modify: `programs/ps/src/client.rs`

- [ ] **Step 1: Read the current `sh_entry` in `programs/ps/src/client.rs`**

```bash
grep -n "fn sh_entry\|short_description\|long_description\|impl ProgramArgsBuilder for\|struct.*ProgramArgsBuilder" programs/ps/src/client.rs
```

Note the exact `name`, `short_description`, `long_description`, and the builder struct name. These are the values for the metadata block.

- [ ] **Step 2: Add the metadata block to `programs/ps/Cargo.toml`**

Insert immediately above `[dependencies]`, using the exact strings extracted in Step 1:

```toml
[[package.metadata.dusk.sh_entry]]
name = "<name from current sh_entry>"
short_description = "<short_description from current sh_entry>"
long_description = """
<long_description body — preserve verbatim>
"""
impl_name = "<the struct name from current sh_entry>"
```

- [ ] **Step 3: Derive `Default` on the args-builder struct in `programs/ps/src/client.rs`**

Add `#[derive(Default)]` directly above the `struct PsProgramArgsBuilder { ... }` (or whatever the actual struct name is — confirm from Step 1).

- [ ] **Step 4: Replace the hand-written `sh_entry` function with the macro**

Delete the entire `#[distributed_slice(...)] pub fn sh_entry() -> ShEntry { ... }` block. Add at the same location:

```rust
dusk_program_proc::sh_entries!();
```

- [ ] **Step 5: Prune unused imports**

Run `cargo check -p dusk_program_ps --features client` and remove any imports the compiler flags as unused. Typical candidates: `linkme::distributed_slice`, `dusk_program_sh::entry::{ProgramInfo, ShEntry}`, `std::rc::Rc` (only if no other Rc usage remains).

- [ ] **Step 6: Verify the crate builds clean**

```bash
cargo check -p dusk_program_ps --features client
```

Expected: `Finished` with no errors and no warnings.

- [ ] **Step 7: Commit**

```bash
git add programs/ps/Cargo.toml programs/ps/src/client.rs
git commit -m "dusk_program_ps: drive sh_entry from Cargo.toml metadata"
```

---

## Task 5: Migrate `programs/kill`

**Files:**
- Modify: `programs/kill/Cargo.toml`
- Modify: `programs/kill/src/client.rs`

Repeat the exact procedure from Task 4 against `programs/kill/`. Each sub-step is identical, just with `kill` in place of `ps`.

- [ ] Step 1: extract name/short/long/struct from `programs/kill/src/client.rs`
- [ ] Step 2: add `[[package.metadata.dusk.sh_entry]]` block to `programs/kill/Cargo.toml`
- [ ] Step 3: `#[derive(Default)]` on the args-builder struct
- [ ] Step 4: delete the hand-written sh_entry block, replace with `dusk_program_proc::sh_entries!();`
- [ ] Step 5: prune unused imports until `cargo check -p dusk_program_kill --features client` is warning-free
- [ ] Step 6: commit with message `dusk_program_kill: drive sh_entry from Cargo.toml metadata`

---

## Task 6: Migrate `programs/sleep`

Same procedure as Task 4, with `sleep` in place of `ps`.

- [ ] Step 1: extract name/short/long/struct from `programs/sleep/src/client.rs`
- [ ] Step 2: add `[[package.metadata.dusk.sh_entry]]` block to `programs/sleep/Cargo.toml`
- [ ] Step 3: `#[derive(Default)]` on the args-builder struct
- [ ] Step 4: replace the hand-written sh_entry block with `dusk_program_proc::sh_entries!();`
- [ ] Step 5: prune unused imports; `cargo check -p dusk_program_sleep --features client` is warning-free
- [ ] Step 6: commit `dusk_program_sleep: drive sh_entry from Cargo.toml metadata`

---

## Task 7: Migrate `programs/true`

Same procedure, with `true` in place of `ps`. The crate name is `dusk_program_true`.

- [ ] Step 1: extract name/short/long/struct from `programs/true/src/client.rs`
- [ ] Step 2: add `[[package.metadata.dusk.sh_entry]]` block to `programs/true/Cargo.toml`
- [ ] Step 3: `#[derive(Default)]` on the args-builder struct
- [ ] Step 4: replace with `dusk_program_proc::sh_entries!();`
- [ ] Step 5: prune imports; `cargo check -p dusk_program_true --features client` clean
- [ ] Step 6: commit `dusk_program_true: drive sh_entry from Cargo.toml metadata`

---

## Task 8: Migrate `programs/false`

Same procedure with `false`.

- [ ] Step 1: extract from `programs/false/src/client.rs`
- [ ] Step 2: metadata block in `programs/false/Cargo.toml`
- [ ] Step 3: derive Default
- [ ] Step 4: macro invocation
- [ ] Step 5: prune; `cargo check -p dusk_program_false --features client` clean
- [ ] Step 6: commit `dusk_program_false: drive sh_entry from Cargo.toml metadata`

---

## Task 8.5: Migrate `programs/sh`

Same procedure with `sh`. Verified by `grep`: `programs/sh/src/client.rs:34` has `#[distributed_slice(crate::entry::SH_ENTRIES)]` with `struct ShProgramArgsBuilder {}`. Note: the distributed-slice path here is `crate::entry::SH_ENTRIES` (because `sh` is itself the crate that *defines* the slice), not `dusk_program_sh::entry::SH_ENTRIES`. The macro emits the latter; that resolves to the same slice when invoked from inside `dusk_program_sh` because `crate::entry::SH_ENTRIES` and `dusk_program_sh::entry::SH_ENTRIES` are the same item. No code change needed inside the macro.

- [ ] Step 1: extract from `programs/sh/src/client.rs` (name, short, long, struct = `ShProgramArgsBuilder`)
- [ ] Step 2: metadata block in `programs/sh/Cargo.toml`
- [ ] Step 3: derive Default on `ShProgramArgsBuilder`
- [ ] Step 4: replace the hand-written sh_entry block with `dusk_program_proc::sh_entries!();`
- [ ] Step 5: prune unused imports; `cargo check -p dusk_program_sh --features client` clean
- [ ] Step 6: commit `dusk_program_sh: drive sh_entry from Cargo.toml metadata`

---

## Task 9: Migrate `programs/init`

Same procedure with `init`. Note `init` is the boot program; the SH_ENTRY registration (if any) is in its client-feature path.

- [ ] Step 1: extract from `programs/init/src/client.rs` (skip this task if `init` has no `#[distributed_slice(SH_ENTRIES)]` registration — it might not, as `init` boots the system rather than being shell-invokable; verify with `grep -n distributed_slice programs/init/src/client.rs` and skip the entire task if no match)
- [ ] Step 2: metadata block in `programs/init/Cargo.toml`
- [ ] Step 3: derive Default
- [ ] Step 4: macro invocation
- [ ] Step 5: prune; `cargo check -p dusk_program_init --features client` clean
- [ ] Step 6: commit `dusk_program_init: drive sh_entry from Cargo.toml metadata`

---

## Task 10: End-to-end verification

**Files:** (none modified)

- [ ] **Step 1: Build the entire workspace**

```bash
cargo build
```

Expected: `Finished` with no errors.

- [ ] **Step 2: Build the final binary**

```bash
cargo build -p dusk_cli_bin
```

Expected: `Finished` with no errors. The binary should link successfully — if any migrated `sh_entries!()` invocation produced a function name that collides, the linker (linkme) would flag it.

- [ ] **Step 3: Smoke-test the shell**

In one terminal:

```bash
cargo run -p dusk_node
```

Wait for `listening on …`.

In another terminal:

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

In the dusk shell prompt:

```
> help
```

Expected: a table listing every migrated program (`ps`, `kill`, `sleep`, `date`, `true`, `false`) with their short descriptions. The descriptions must match the strings now in each program's Cargo.toml.

```
> ps
```

Expected: the process table renders, listing `init`, `sh[server]`, `ps`. This confirms `SH_ENTRIES` is being populated by the macro-generated functions.

```
> date
```

Expected: prints the current time.

```
> exit
```

Stop the `dusk_node` process.

- [ ] **Step 4: Final commit (only if any cleanup was needed across the migration)**

If everything worked without an extra commit, this task produces no commit and you can stop here.

If you discovered a missed import prune or similar during smoke-test, fix it and commit with a message describing the fixup.

---

## Self-review checklist

- [ ] Every program crate that registers an sh_entry has a matching `[[package.metadata.dusk.sh_entry]]` block.
- [ ] Every migrated `src/client.rs` ends with `dusk_program_proc::sh_entries!();` and contains no `#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]` attribute attached to a hand-written function.
- [ ] Every args-builder struct has `#[derive(Default)]`.
- [ ] No crate has a remaining unused import warning under `cargo check --features client`.
- [ ] `cargo build` from workspace root succeeds.
- [ ] Smoke test (Task 10 Step 3) shows the migrated programs in `help`.
