use std::env;

use dusk_program::anyhow::{Context, Error, Result, anyhow, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// The full chat-completions URL Ask Dusk posts to.
pub const ENDPOINT_URL_VARIABLE: &str = "DUSK_LLM_URL";
/// The model Ask Dusk asks that endpoint for.
pub const MODEL_VARIABLE: &str = "DUSK_LLM_MODEL";
/// The bearer token for that endpoint. Optional - some endpoints want none.
pub const API_KEY_VARIABLE: &str = "DUSK_LLM_API_KEY";
/// Set to `1` to keep TLS but stop checking the certificate behind it.
pub const TLS_NO_VERIFY_VARIABLE: &str = "DUSK_LLM_TLS_NO_VERIFY";

/// Sent on every request. Hosts commonly sit behind bot filters that reject
/// clients which do not identify themselves at all, so this is not left to the
/// HTTP library's default.
const USER_AGENT: &str = concat!(
    "dusk_llm/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/tomerze/dusk)"
);

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Ceiling on a turn's output. Sized with headroom because a model that
/// reasons before answering spends this budget on reasoning it never shows:
/// one measured turn reported 64 visible tokens against 404 hidden ones.
const MAX_RESPONSE_TOKENS: u32 = 8192;
const TEMPERATURE: f32 = 0.6;

/// How many past messages a chat carries forward, newest first, not counting
/// the system prompt. The endpoint charges for and bounds the whole
/// conversation, so an unbounded history would eventually be refused outright
/// rather than degrade.
const HISTORY_LIMIT: usize = 20;

/// How much the transport is trusted to protect a turn on its way out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Plaintext. Anything on the path reads the question and the node's
    /// program list.
    Http,
    /// TLS with the certificate checked against the system roots.
    Https,
    /// TLS with certificate and hostname checks turned off - encrypted against
    /// a passive listener, but it cannot tell the endpoint from an impostor.
    TlsNoVerify,
}

/// Where Ask Dusk sends its requests, and as whom.
#[derive(Clone, Debug)]
pub struct Endpoint {
    url: reqwest::Url,
    model: String,
    api_key: Option<String>,
    transport: Transport,
}

impl Endpoint {
    /// Build an endpoint. `tls_no_verify` only means anything for an `https`
    /// URL, where it keeps the encryption and drops the identity check - for a
    /// self-signed certificate on a model you host yourself.
    pub fn new(
        url: &str,
        model: impl Into<String>,
        api_key: Option<String>,
        tls_no_verify: bool,
    ) -> Result<Self> {
        let url = reqwest::Url::parse(url).with_context(|| format!("parsing {url} as a URL"))?;
        let transport = match (url.scheme(), tls_no_verify) {
            ("https", false) => Transport::Https,
            ("https", true) => Transport::TlsNoVerify,
            ("http", _) => Transport::Http,
            (scheme, _) => bail!(
                "the Ask Dusk endpoint {url} is {scheme}, which is not a way to reach an HTTP \
                 API. Set {ENDPOINT_URL_VARIABLE} to an http:// or https:// URL."
            ),
        };
        Ok(Self {
            url,
            model: model.into(),
            api_key,
            transport,
        })
    }

    /// The endpoint named by `DUSK_LLM_URL`, `DUSK_LLM_MODEL`,
    /// `DUSK_LLM_API_KEY` and `DUSK_LLM_TLS_NO_VERIFY`. Ask Dusk ships pointed
    /// at nothing, so a missing URL or model is reported as the setup step it
    /// is, before the prompt takes a question it would have to discard.
    pub fn from_environment() -> Result<Self> {
        let read = |variable| env::var(variable).ok().filter(|value| !value.is_empty());
        match (read(ENDPOINT_URL_VARIABLE), read(MODEL_VARIABLE)) {
            (Some(url), Some(model)) => Self::new(
                &url,
                model,
                read(API_KEY_VARIABLE),
                matches!(read(TLS_NO_VERIFY_VARIABLE).as_deref(), Some("1")),
            ),
            _ => bail!("{}", setup_help()),
        }
    }

    /// What protects - or does not protect - a turn in transit.
    pub fn transport(&self) -> Transport {
        self.transport
    }
}

/// Every variable Ask Dusk reads, and an example for the one whose shape is not
/// obvious from its description.
const HELP_ENTRIES: &[(&str, &str, Option<&str>)] = &[
    (
        ENDPOINT_URL_VARIABLE,
        "OpenAI API compatible chat-completions URL",
        Some("https://api.openai.com/v1/chat/completions"),
    ),
    (MODEL_VARIABLE, "the model to ask that endpoint for", None),
    (
        API_KEY_VARIABLE,
        "your API key - leave unset if it needs none",
        None,
    ),
    (
        TLS_NO_VERIFY_VARIABLE,
        "set to 1 to skip certificate checks",
        None,
    ),
];

/// The gap between the longest variable name and the descriptions.
const HELP_GAP: usize = 2;

/// What to set. Ask Dusk's first Ctrl+A ends here, so this is the whole of its
/// setup documentation as far as most users see it. It is logged as one warning,
/// hence the leading blank line: it has to read as a block under the log prefix.
fn setup_help() -> String {
    // Derived rather than fixed: a name longer than a hardcoded column silently
    // loses its padding and runs into its own description.
    let indent = HELP_ENTRIES
        .iter()
        .map(|(variable, _, _)| variable.len())
        .max()
        .unwrap_or(0)
        + HELP_GAP;

    let mut help = String::from("\n\nTo use Ask Dusk, set these, then press Ctrl+A again:\n\n");
    for (variable, description, example) in HELP_ENTRIES {
        help += &format!("  {variable:<indent$}{description}\n");
        if let Some(example) = example {
            help += &format!("  {:<indent$}e.g. {example}\n", "");
        }
    }
    help
}

#[derive(Debug, Deserialize)]
pub struct LlmReply {
    pub explanation: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

/// One past exchange, replayed ahead of the user's question so the model has
/// the reply format and the voice demonstrated rather than only described.
#[derive(Deserialize)]
struct Example {
    user: String,
    assistant: String,
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    stream_options: StreamOptions,
    temperature: f32,
    max_tokens: u32,
}

/// Asks the endpoint to report what the turn actually cost. Hosts that do not
/// implement it ignore it, which is why the streamed chunks are still counted.
#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Deserialize)]
struct CompletionChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    /// A reasoning model streams its thinking here - under this name, or under
    /// `reasoning` on hosts that follow OpenRouter. It is not part of the
    /// reply, but it is most of what such a model generates in a turn.
    #[serde(default, alias = "reasoning")]
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    completion_tokens: Option<usize>,
}

#[derive(Clone)]
pub struct Chat {
    client: reqwest::Client,
    endpoint: Endpoint,
    /// The system message and the few-shot exchanges, built once and sent
    /// ahead of the conversation on every turn.
    preamble: Vec<Message>,
    history: std::sync::Arc<Mutex<Vec<Message>>>,
}

impl Chat {
    /// A chat against the endpoint the environment names. `programs` is the
    /// rendered list of programs the connected node can run; it is spliced into
    /// the system prompt so the model only suggests commands that exist.
    pub fn new(programs: &str) -> Result<Self> {
        Self::with_endpoint(Endpoint::from_environment()?, programs)
    }

    /// A chat against a caller-supplied endpoint.
    pub fn with_endpoint(endpoint: Endpoint, programs: &str) -> Result<Self> {
        let tls_no_verify = endpoint.transport == Transport::TlsNoVerify;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .tls_danger_accept_invalid_certs(tls_no_verify)
            .tls_danger_accept_invalid_hostnames(tls_no_verify)
            .build()
            .context("building the HTTP client for Ask Dusk")?;
        let examples: Vec<Example> = serde_json::from_str(include_str!("../prompts/examples.json"))
            .context("parsing the built-in few-shot examples")?;
        let mut preamble = vec![Message {
            role: "system",
            content: include_str!("../prompts/system.md").replace("{{PROGRAMS}}", programs),
        }];
        for example in examples {
            preamble.push(Message {
                role: "user",
                content: example.user,
            });
            preamble.push(Message {
                role: "assistant",
                content: example.assistant,
            });
        }

        Ok(Self {
            client,
            endpoint,
            preamble,
            history: std::sync::Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Run one chat turn. `on_token` fires once per streamed chunk with the
    /// running count of tokens generated so far, reasoning included.
    pub async fn chat(
        &self,
        message: &str,
        mut on_token: impl FnMut(usize) + Send + 'static,
    ) -> Result<LlmReply> {
        let mut history = self.history.lock().await;
        history.push(Message {
            role: "user",
            content: message.to_owned(),
        });

        let mut messages = self.preamble.clone();
        messages.extend(history.iter().rev().take(HISTORY_LIMIT).rev().cloned());

        let request = self
            .client
            .post(self.endpoint.url.clone())
            .json(&CompletionRequest {
                model: &self.endpoint.model,
                messages: &messages,
                stream: true,
                stream_options: StreamOptions {
                    include_usage: true,
                },
                temperature: TEMPERATURE,
                max_tokens: MAX_RESPONSE_TOKENS,
            });
        let request = match &self.endpoint.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        };

        let response = request
            .send()
            .await
            .with_context(|| format!("sending a chat request to {}", self.endpoint.url))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            history.pop();
            if matches!(
                status,
                reqwest::StatusCode::PAYMENT_REQUIRED | reqwest::StatusCode::TOO_MANY_REQUESTS
            ) {
                bail!(
                    "{} refused this request ({status}) - its budget or rate limit for this \
                     caller is spent. Wait a moment and ask again, or point {ENDPOINT_URL_VARIABLE} \
                     and {API_KEY_VARIABLE} at an account with more room.",
                    self.endpoint.url,
                );
            }
            bail!("{} answered {status}: {}", self.endpoint.url, body.trim());
        }

        let completion = match read_stream(response, &mut on_token).await {
            Ok(completion) => completion,
            Err(error) => {
                history.pop();
                return Err(error);
            }
        };

        // A reply the endpoint cut short is not the model failing to follow the
        // output contract, and saying so would send the reader after the wrong
        // thing. Name the ceiling that actually stopped it.
        if !completion.ended {
            history.pop();
            bail!(
                "{} closed the stream after {} characters without finishing the reply.",
                self.endpoint.url,
                completion.reply.chars().count(),
            );
        }

        if completion.finish_reason.as_deref() == Some("length") {
            history.pop();
            bail!(
                "{} stopped the reply after {MAX_RESPONSE_TOKENS} tokens, before it was finished.",
                self.endpoint.url,
            );
        }

        history.push(Message {
            role: "assistant",
            content: completion.reply.clone(),
        });
        parse_reply(completion.reply.trim())
    }
}

/// What the endpoint said, why it stopped saying it, and whether it got to the
/// end at all.
struct Completion {
    reply: String,
    finish_reason: Option<String>,
    /// The stream carried its `[DONE]` marker. Without it the reply is whatever
    /// arrived before the connection stopped, which is not the whole answer.
    ended: bool,
}

/// Accumulate the `content` deltas of an OpenAI-style `text/event-stream`
/// response, firing `on_token` with the running count of generated tokens -
/// reasoning included - as it grows.
///
/// The buffer is bytes, not text, and is decoded a whole line at a time. A
/// multi-byte character can land across two chunks of the response, and
/// decoding each chunk on its own turns the halves into replacement characters
/// — which corrupts that line's JSON, drops the delta it carried, and truncates
/// the reply. A line break cannot occur inside a UTF-8 character, so splitting
/// on it before decoding is always safe.
async fn read_stream<F: FnMut(usize)>(
    response: reqwest::Response,
    on_token: &mut F,
) -> Result<Completion> {
    let mut stream = response.bytes_stream();
    let mut pending: Vec<u8> = Vec::new();
    let mut reply = String::new();
    let mut finish_reason = None;
    let mut deltas = 0;
    let mut ended = false;

    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk.context("reading the response stream")?);

        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            let line = match std::str::from_utf8(&line) {
                Ok(line) => line.trim(),
                Err(error) => {
                    tracing::warn!(error = ?error, "dropping a stream line that is not UTF-8");
                    continue;
                }
            };

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                ended = true;
                continue;
            }
            let chunk: CompletionChunk = match serde_json::from_str(data) {
                Ok(chunk) => chunk,
                Err(error) => {
                    // Dropping one of these loses part of the reply, so it is
                    // reported rather than passed over quietly.
                    tracing::warn!(error = ?error, data, "dropping an unparsable stream chunk");
                    continue;
                }
            };

            if let Some(choice) = chunk.choices.first() {
                if let Some(content) = choice.delta.content.as_ref()
                    && !content.is_empty()
                {
                    reply.push_str(content);
                    deltas += 1;
                }
                if let Some(reasoning) = choice.delta.reasoning_content.as_ref()
                    && !reasoning.is_empty()
                {
                    deltas += 1;
                }
                if choice.finish_reason.is_some() {
                    finish_reason.clone_from(&choice.finish_reason);
                }
            }
            // The endpoint's own running total wins where it sends one; the
            // delta tally is the fallback for hosts that ignore stream_options.
            on_token(
                chunk
                    .usage
                    .and_then(|usage| usage.completion_tokens)
                    .unwrap_or(deltas),
            );
        }
    }

    // Anything left without a trailing newline is a line the endpoint never
    // finished sending. Dropping it silently is what turns a cut-short stream
    // into a reply that merely looks malformed.
    if !pending.is_empty() {
        tracing::warn!(
            trailing = pending.len(),
            "the response stream ended mid-line"
        );
    }

    Ok(Completion {
        reply,
        finish_reason,
        ended,
    })
}

/// Extract the first JSON object from the model's raw reply. Models
/// occasionally prefix with stray prose, so we skip to the first `{` and parse
/// one value off the stream (ignoring any trailing noise).
fn parse_reply(text: &str) -> Result<LlmReply> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("no `{{` in reply; raw reply: {text:?}"))?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<LlmReply>();
    match stream.next() {
        Some(Ok(reply)) => Ok(reply),
        Some(Err(error)) => Err(Error::new(error).context(format!(
            "model did not return a valid LlmReply; raw reply: {text:?}"
        ))),
        None => Err(anyhow!("no JSON object in reply; raw reply: {text:?}")),
    }
}
