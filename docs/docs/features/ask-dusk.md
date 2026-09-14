# Ask Dusk

Ask Dusk turns plain English into [shell](shell.md) commands, right inside the
`dusk` prompt. You don't have to remember a program's name or flags - describe
what you want, and Dusk drafts the command.

## Setting it up

Ask Dusk asks a model you choose, so point it at one first. It works with any
endpoint that speaks the OpenAI chat-completions API - a commercial provider, a
gateway, or a model you host yourself:

```sh
export DUSK_LLM_URL='https://api.openai.com/v1/chat/completions'
export DUSK_LLM_MODEL='gpt-4o-mini'
export DUSK_LLM_API_KEY='sk-…'          # skip it if your endpoint needs none
```

| Variable | Required | What it is |
|---|---|---|
| `DUSK_LLM_URL` | yes | The full chat-completions URL, `https://` or `http://`. |
| `DUSK_LLM_MODEL` | yes | The model name to ask that endpoint for. |
| `DUSK_LLM_API_KEY` | no | Sent as `Authorization: Bearer …` when set. |
| `DUSK_LLM_TLS_NO_VERIFY` | no | Set to `1` to keep TLS but stop checking the certificate. |

A model on your own machine or LAN usually wants one of the two weaker
transports:

```sh
export DUSK_LLM_URL='http://localhost:11434/v1/chat/completions'   # plaintext
```

```sh
export DUSK_LLM_URL='https://gpu-box.lan/v1/chat/completions'      # self-signed
export DUSK_LLM_TLS_NO_VERIFY=1
```

Know what each one gives up. **`http://` is plaintext** - anything on the path
between you and the endpoint reads your question and your node's program list.
**`DUSK_LLM_TLS_NO_VERIFY=1` keeps the encryption but drops the identity
check** - a passive listener still sees nothing, but Ask Dusk can no longer tell
your endpoint from something that answered in its place, so it is for a
certificate you issued yourself, not a way past a certificate error on the open
internet. Either one logs a warning naming what stopped protecting the traffic.

Press **Ctrl + A** before setting these and Ask Dusk tells you exactly what is
missing rather than failing quietly.

## Using it

Open the prompt, press **Ctrl + A** (A for *Ask*), and type naturally:

```
> (Ctrl+A) show me everything running and stop the one called sleep
```

Ask Dusk replies with the command it would run (e.g. `ps` then `kill …`) plus a
short explanation, so you stay in control of what actually executes.

## Why it's there

Diagnosing a device under pressure is exactly when you don't want to be looking
up syntax. Ask Dusk lowers that barrier, which makes it a natural companion to
[diagnosis](diagnosis.md).

Ask Dusk brings its own knowledge of Dusk to whatever model you point it at: the
grammar, the strict output contract, and the list of programs your node actually
runs all travel with the question, so the model drafts commands that exist rather
than plausible-looking GNU ones.

Note what leaves the machine. Every question sends your text and your node's
program list to the endpoint you configured. Even over checked HTTPS the
operator of that endpoint sees all of it - choose one you are willing to show
your fleet's command surface.

For how it works under the hood - the request it builds and the prompt it
assembles - see [Development › Ask Dusk](../development/ask_dusk.md).
