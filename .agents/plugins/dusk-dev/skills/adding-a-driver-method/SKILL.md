---
name: adding-a-driver-method
description: Add a new method to the Dusk Driver trait and wire it up through all required layers. Use this skill whenever the user asks to add a method, function, or capability to the Driver trait, mentions "driver method", or wants to expose new platform functionality through the driver abstraction — even if they don't explicitly call it "adding a driver method".
---

# Adding a Driver Method to Dusk

## Step 0 — Confirm the signature

Before touching any files, ask the user for the exact method signature if they haven't provided one. You need the method name, parameters (beyond `&self`), and return type. Do not guess or assume `-> Result<String>`. Wait for confirmation before proceeding.

---

## Step 1 — Find all Driver implementations

Search the codebase for every type that implements the `Driver` trait:

```
grep -r "impl Driver for" impls/
```

Each result is a file you must edit in addition to the core trait file. Right now only `impls/nix/src/driver.rs` exists, but there may be more in the future — always discover them dynamically rather than hardcoding paths.

---

## Step 2 — Wire the four layers in `dusk/src/dusk_core/src/driver.rs`

Touch these in order. The canonical example to follow is `hostname`. Place new methods immediately after `hostname` in each location.

**Trait definition:**
```rust
fn <method>(<signature>) -> <return_type>;
```

**Inside `dusk_driver_impl!`, after the `_dusk_hostname` block:**
```rust
#[unsafe(no_mangle)]
fn _dusk_<method>(<params>) -> <return_type> {
    <$t as $crate::driver::Driver>::<method>(&$name <, args>)
}
```

**`unsafe extern "Rust"` block:**
```rust
fn _dusk_<method>(<params>) -> <return_type>;
```

**Public wrapper:**
```rust
pub fn <method>(<params>) -> <return_type> {
    unsafe { _dusk_<method>(<args>) }
}
```

---

## Step 3 — Add boilerplate to every Driver impl

For each file found in Step 1, add the method body inside `impl Driver for <Type>`, immediately after `hostname`:

```rust
fn <method>(<signature>) -> <return_type> {
    todo!()
}
```

---

## Checklist

- [ ] Signature confirmed with user
- [ ] All `impl Driver for` files discovered
- [ ] Trait method signature added
- [ ] `dusk_driver_impl!` macro `no_mangle` fn added
- [ ] `unsafe extern "Rust"` declaration added
- [ ] `pub fn` wrapper added
- [ ] Boilerplate impl added in every discovered driver

## I do not write comments

Not one — not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).
