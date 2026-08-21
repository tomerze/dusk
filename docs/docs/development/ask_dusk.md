# Ask Dusk

Ask Dusk translates natural language into dusk shell commands from inside the
`dusk` prompt. It asks a model over HTTP; it does not run one. Any endpoint that
speaks the OpenAI chat-completions API will do, and the user chooses which one —
see [Ask Dusk](../features/ask-dusk.md) for the variables.

Open the prompt, press **Ctrl + A** (A for Ask), and type naturally.

This page documents how it works under the hood.

## The shape of it

Two workspace crates, no C, no build script:

- **`dusk/src/dusk_llm/`** owns the prompt and the HTTP call: `prompts/system.md`
  (persona, grammar, output contract), `prompts/examples.json` (the few-shot
  exchanges), and `src/chat.rs` (`Endpoint`, `Chat`, `LlmReply`).
- **`dusk/src/dusk_prompt/`** consumes `dusk_llm::Chat` like any other
  dependency, and supplies the list of programs the connected node can run.

The whole crate is about 300 lines of Rust and one `reqwest` client.

## Configuration

`Endpoint::from_environment()` reads four variables:

| Variable | Required | Meaning |
|---|---|---|
| `DUSK_LLM_URL` | yes | The full chat-completions URL, `https://` or `http://`. |
| `DUSK_LLM_MODEL` | yes | The model name sent as `model`. |
| `DUSK_LLM_API_KEY` | no | Sent as `Authorization: Bearer …` when non-empty. |
| `DUSK_LLM_TLS_NO_VERIFY` | no | `1` turns off certificate and hostname checks. |

The scheme and that flag resolve to a `Transport`, which `Endpoint::transport()`
exposes:

| `Transport` | Reached by | What it protects |
|---|---|---|
| `Https` | an `https://` URL | Encrypted, and the endpoint proved who it is. |
| `TlsNoVerify` | `https://` plus `DUSK_LLM_TLS_NO_VERIFY=1` | Encrypted against a listener, but an impostor passes. For a certificate you issued yourself. |
| `Http` | an `http://` URL | Nothing. The question and the program list cross the network in the clear. |

Any other scheme is refused before a request is built — there is no way to reach
an HTTP API over `file://`. The two weakened transports each log a `warn` naming
what stopped protecting the traffic, so a machine configured this way says so in
its own logs rather than only in whoever's shell exported the variable.

`Endpoint::new` is also the injection point: a Rust caller that does not want the
environment consulted builds an `Endpoint` itself and calls
`Chat::with_endpoint`.

When a required variable is unset, `from_environment` fails with the setup text
the prompt prints — the variables, an example of each, and which ones are still
missing. Nothing about Ask Dusk works before that, so the first run is expected
to end there, and it says so in full rather than logging a warning the user
never sees.

## The request

`Chat::new(programs)` builds a preamble once:

1. `prompts/system.md` with `{{PROGRAMS}}` replaced by the rendered program list.
   That template holds the persona, the dusk grammar, and the strict
   one-JSON-object output contract.
2. `prompts/examples.json`, expanded into alternating `user` / `assistant`
   messages. These are few-shot demonstrations of the reply format and the
   voice; they are ordinary messages, so they carry across models rather than
   depending on one model's turn syntax.

`Chat::chat` then appends the conversation so far and posts with `stream: true`.
Every delta increments the counter behind the `↓ N tokens` spinner — a reasoning
model's `reasoning_content` as well as the reply's `content`, since a turn is
mostly reasoning and a counter that ignored it would sit at zero until the answer
landed. Where the endpoint honours `stream_options.include_usage`, its own
`completion_tokens` replaces the tally. Until the first token lands the spinner
reads `waiting for tokens` rather than `↓ 0 tokens`, because an endpoint that
reasons before it answers transmits nothing for the first several seconds of a
turn.

The reply is parsed as the first JSON object in the output into
`LlmReply { explanation, command }` — models occasionally wrap the object in
stray prose, so the parser skips to the first `{` and reads one value off the
stream.

Only the most recent `HISTORY_LIMIT` (20) messages are replayed. The endpoint
bounds and bills the whole conversation, so an unbounded history would eventually
be refused outright rather than gradually degrade.

## Where the program list comes from

`dusk_prompt` already holds an `EntryInfo` per shell-invocable program — name,
short description, long description — and uses it for `help`. `chat_submit`
sorts that list by name and renders it into the `{{PROGRAMS}}` block.

This is the same data the model has always been given; it is simply read at
runtime now. Earlier, the prompt was assembled at compile time, so the
`#[sh_entry]` macro had to write each program's metadata to
`target/.dusk_sh_entries/*.json` for a build script to glob back up. That
side-channel — and the mtime guard that stopped it retriggering an expensive
build step — is gone.

## Failure modes

Every one of these is shown in the prompt, not just logged:

- **Unset configuration** — the setup text above.
- **A `DUSK_LLM_URL` that is neither http nor https** — refused before any
  request is made.
- **`402` / `429`** — the endpoint's budget or rate limit for this caller is
  spent. The message says to wait or point the variables at an account with more
  room.
- **Any other non-2xx** — the status and the endpoint's own response body,
  verbatim, since that is where a provider explains itself.
- **A reply that is not valid JSON** — reported with the raw reply attached.

A turn that fails leaves the history as it was, so a retry is not sent a
conversation containing a question that was never answered.
