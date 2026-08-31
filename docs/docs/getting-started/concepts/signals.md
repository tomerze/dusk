# Signals

Dusk processes can be signaled, just like Unix-like processes.
The main usage of signals is telling processes when it is time to stop.

## The `Signal` type

Dusk maps the wire signal number to a small enum:

```rust
#[non_exhaustive]
pub enum Signal {
    Terminate,               // signal number 15
    Unknown(u64),            // any other number, carried through unchanged
}
```

`Signal` is `#[non_exhaustive]`, so a `match` on it needs a wildcard arm. Dusk can
then add a signal without breaking programs that were written before it existed.

- **`Terminate`** (15) is the request to exit; a well-behaved process returns from
  `main` when it sees it. It's the default sent by [`kill`](base.md#kill).
- **`Unknown(n)`** carries any other number through as-is, so a program can give
  additional signal numbers its own meaning.

## Receiving signals

`main` is handed a `SignalReceiver` (a `DynamicReceiver<Signal>`). The usual shape
is a loop that awaits the next signal and exits on `Terminate`:

```rust
loop {
    match signal_receiver.receive().await {
        Signal::Terminate => return Ok(()),
        Signal::Unknown(_) => { /* program-defined */ }
        _ => {}
    }
}
```

A process that also does ongoing work `select`s that work against the receiver.
See [Processes › Signals](processes.md#signals).
