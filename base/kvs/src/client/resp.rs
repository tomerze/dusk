use super::{ScannedKeys, key_display, key_parse};
use crate::kvs_capnp::kvs_portal;
use crate::{Value, kvs};
use dusk_capnp::capnp_rpc;
use dusk_program::stream::Stream;
use std::borrow::ToOwned as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::{format, string::String, string::ToString as _, vec, vec::Vec};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::task::AbortHandle;

const MAX_ARGUMENTS: usize = 1024 * 1024;
const MAX_COMMAND_LENGTH: u64 = 512 * 1024 * 1024;
const MAX_LINE_LENGTH: u64 = 64 * 1024;

pub(super) type Connections = Rc<RefCell<Vec<AbortHandle>>>;
pub(super) type Names = Rc<RefCell<HashMap<u64, String>>>;

pub(super) async fn connection(
    stream: TcpStream,
    portal: anyhow::Result<kvs_portal::Client>,
    flags: u8,
    names: &Names,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let served: anyhow::Result<()> = async {
        let portal = portal?;
        let mut reader = BufReader::new(reader);
        let mut nil = RESP2_NIL;
        loop {
            let Some(command) = read_command(&mut reader)
                .await
                .map_err(|error| error.context("Protocol error"))?
            else {
                return Ok(());
            };
            let (reply, quit) = execute(&portal, flags, names, &mut nil, command).await?;
            writer.write_all(&reply).await?;
            if quit {
                return Ok(());
            }
        }
    }
    .await;
    let Err(failure) = served else {
        return Ok(());
    };
    if let Err(write_error) = writer.write_all(&error(&format!("{failure:#}"))).await {
        tracing::debug!(%write_error, "couldn't send the kvs resp connection the error that closed it");
    }
    Err(failure)
}

async fn read_command<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<Vec<Vec<u8>>>> {
    let Some(header) = read_line(reader).await? else {
        return Ok(None);
    };
    let Some(count) = header.strip_prefix(b"*") else {
        anyhow::bail!("expected an array");
    };
    let count: usize = std::str::from_utf8(count)?.parse()?;
    anyhow::ensure!(
        count <= MAX_ARGUMENTS,
        "an array of {count} elements is more than the {MAX_ARGUMENTS} allowed"
    );
    let mut remaining = MAX_COMMAND_LENGTH;
    let mut arguments = Vec::new();
    for _ in 0..count {
        let Some(header) = read_line(reader).await? else {
            anyhow::bail!("the connection closed in the middle of a command");
        };
        let Some(length) = header.strip_prefix(b"$") else {
            anyhow::bail!("expected a bulk string");
        };
        let length: u64 = std::str::from_utf8(length)?.parse()?;
        anyhow::ensure!(
            length <= remaining,
            "a command is longer than the {MAX_COMMAND_LENGTH} bytes allowed"
        );
        remaining -= length;
        let mut argument = Vec::new();
        reader.take(length + 2).read_to_end(&mut argument).await?;
        anyhow::ensure!(
            argument.len() as u64 == length + 2,
            "the connection closed in the middle of a command"
        );
        anyhow::ensure!(
            argument.ends_with(b"\r\n"),
            "a bulk string is not terminated by CRLF"
        );
        argument.truncate(length as usize);
        arguments.push(argument);
    }
    Ok(Some(arguments))
}

async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> anyhow::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    if reader
        .take(MAX_LINE_LENGTH)
        .read_until(b'\n', &mut line)
        .await?
        == 0
    {
        return Ok(None);
    }
    if !line.ends_with(b"\r\n") {
        anyhow::bail!("a line of {} bytes is not terminated by CRLF", line.len());
    }
    line.truncate(line.len() - 2);
    Ok(Some(line))
}

async fn execute(
    portal: &kvs_portal::Client,
    flags: u8,
    names: &Names,
    nil: &mut &'static [u8],
    command: Vec<Vec<u8>>,
) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut arguments = command.iter();
    let Some(name) = arguments.next() else {
        return Ok((error("empty command"), false));
    };
    let name = String::from_utf8_lossy(name).to_ascii_uppercase();
    let Ok(arguments) = arguments
        .map(|argument| std::str::from_utf8(argument))
        .collect::<Result<Vec<&str>, _>>()
    else {
        return Ok((error("arguments must be UTF-8"), false));
    };
    let reply = match (name.as_str(), arguments.as_slice()) {
        ("PING", []) => b"+PONG\r\n".to_vec(),
        ("PING", [message]) => bulk(message.as_bytes()),
        ("QUIT", []) => return Ok((b"+OK\r\n".to_vec(), true)),
        ("HELLO", arguments) => hello(arguments, nil),
        ("INFO", _) => bulk(
            format!(
                "# Server\r\nserver_name:dusk\r\ndusk_version:{}\r\nredis_mode:standalone\r\n\r\n\
                 # Persistence\r\nloading:0\r\n\r\n# Replication\r\nrole:master\r\n",
                crate::VERSION
            )
            .as_bytes(),
        ),
        ("COMMAND", _) => b"*0\r\n".to_vec(),
        ("CLIENT", _) => b"+OK\r\n".to_vec(),
        ("GET", [key]) => match get(portal, names, key).await? {
            Some(value) => string(&value, nil).unwrap_or_else(wrong_type),
            None => nil.to_vec(),
        },
        ("SET", [key, value]) => match set(portal, flags, names, key, value).await? {
            Some(refusal) => refusal,
            None => b"+OK\r\n".to_vec(),
        },
        ("SET", [_, _, option, ..]) => error(&format!("SET option '{option}' is not supported")),
        ("MGET", keys) if !keys.is_empty() => {
            let mut replies = Vec::with_capacity(keys.len());
            for key in keys {
                replies.push(match get(portal, names, key).await? {
                    Some(value) => string(&value, nil).unwrap_or_else(|| nil.to_vec()),
                    None => nil.to_vec(),
                });
            }
            array(replies)
        }
        ("MSET", pairs) if !pairs.is_empty() && pairs.len() % 2 == 0 => 'mset: {
            for pair in pairs.chunks_exact(2) {
                if let Some(refusal) = set(portal, flags, names, pair[0], pair[1]).await? {
                    break 'mset refusal;
                }
            }
            b"+OK\r\n".to_vec()
        }
        ("DEL", keys) if !keys.is_empty() => 'delete: {
            let mut deleted = 0;
            for key in keys {
                let mut request = portal.delete_request();
                let id = key_parse(key);
                request.get().set_key(id);
                request.get().set_forbidden_unstick(true);
                match request.send().promise.await {
                    Ok(response) => {
                        if response.get()?.get_deleted() {
                            names.borrow_mut().remove(&id);
                            deleted += 1;
                        }
                    }
                    Err(failure) => break 'delete refused(failure)?,
                }
            }
            integer(deleted)
        }
        ("EXISTS", keys) if !keys.is_empty() => {
            let mut present = 0;
            for key in keys {
                let mut request = portal.exists_request();
                request.get().set_key(key_parse(key));
                if request.send().promise.await?.get()?.get_exists() {
                    remember(names, key);
                    present += 1;
                }
            }
            integer(present)
        }
        ("TYPE", [key]) => {
            let kind = match get(portal, names, key).await? {
                Some(Value::List(_)) => "list",
                Some(Value::Record(_)) => "hash",
                Some(Value::Null) | None => "none",
                Some(_) => "string",
            };
            format!("+{kind}\r\n").into_bytes()
        }
        ("KEYS", [pattern]) => {
            let mut keys: Vec<String> = ids(portal)
                .await?
                .into_iter()
                .map(|id| display(names, id))
                .filter(|key| glob_matches(pattern.as_bytes(), key.as_bytes()))
                .collect();
            keys.sort();
            array(keys.iter().map(|key| bulk(key.as_bytes())).collect())
        }
        ("SCAN", [cursor, options @ ..]) => {
            let mut pattern = "*";
            let mut count = 10;
            let mut options = options.iter();
            while let Some(option) = options.next() {
                match (option.to_ascii_uppercase().as_str(), options.next()) {
                    ("MATCH", Some(value)) => pattern = value,
                    ("COUNT", Some(value)) => match value.parse() {
                        Ok(parsed) if parsed > 0 => count = parsed,
                        _ => {
                            return Ok((error("value is not an integer or out of range"), false));
                        }
                    },
                    _ => return Ok((error("syntax error"), false)),
                }
            }
            let Ok(cursor) = cursor.parse::<u64>() else {
                return Ok((error("invalid cursor"), false));
            };
            let ids = ids(portal).await?;
            let from = ids.partition_point(|id| *id < cursor);
            let next = ids.get(from.saturating_add(count)).copied().unwrap_or(0);
            let keys = ids[from..]
                .iter()
                .take(count)
                .map(|id| display(names, *id))
                .filter(|key| glob_matches(pattern.as_bytes(), key.as_bytes()))
                .map(|key| bulk(key.as_bytes()))
                .collect();
            array(vec![bulk(next.to_string().as_bytes()), array(keys)])
        }
        (
            "PING" | "GET" | "SET" | "MGET" | "MSET" | "DEL" | "EXISTS" | "TYPE" | "KEYS" | "SCAN"
            | "QUIT",
            _,
        ) => error(&format!(
            "wrong number of arguments for '{}' command",
            name.to_lowercase()
        )),
        _ => error(&format!("unknown command '{}'", name.to_lowercase())),
    };
    Ok((reply, false))
}

async fn get(
    portal: &kvs_portal::Client,
    names: &Names,
    key: &str,
) -> anyhow::Result<Option<Value>> {
    let id = key_parse(key);
    let mut request = portal.get_request();
    request.get().set_key(id);
    match request.send().promise.await {
        Ok(response) => {
            remember(names, key);
            Ok(Some(Value::from_reader(response.get()?.get_value()?)?))
        }
        Err(failure) if failure.kind == capnp::ErrorKind::Failed => Ok(None),
        Err(failure) => Err(failure.into()),
    }
}

async fn set(
    portal: &kvs_portal::Client,
    flags: u8,
    names: &Names,
    key: &str,
    value: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let id = key_parse(key);
    let mut request = portal.set_request();
    request.get().set_key(id);
    request.get().set_flags(flags);
    request.get().set_forbidden_unstick(true);
    Value::String(value.to_owned()).write_to_builder(request.get().init_value())?;
    match request.send().promise.await {
        Ok(_) => {
            remember(names, key);
            Ok(None)
        }
        Err(failure) => Ok(Some(refused(failure)?)),
    }
}

fn hello(arguments: &[&str], nil: &mut &'static [u8]) -> Vec<u8> {
    let protocol = match arguments.first().map(|version| version.parse::<u64>()) {
        None if *nil == RESP3_NIL => 3,
        None => 2,
        Some(Ok(version @ (2 | 3))) => version,
        Some(_) => return b"-NOPROTO unsupported protocol version\r\n".to_vec(),
    };
    let mut options = arguments.iter().skip(1);
    while let Some(option) = options.next() {
        if !option.eq_ignore_ascii_case("SETNAME") || options.next().is_none() {
            return error(&format!("HELLO option '{option}' is not supported"));
        }
    }
    let fields = vec![
        bulk(b"server"),
        bulk(b"dusk"),
        bulk(b"version"),
        bulk(crate::VERSION.as_bytes()),
        bulk(b"proto"),
        integer(protocol),
        bulk(b"mode"),
        bulk(b"standalone"),
        bulk(b"role"),
        bulk(b"master"),
        bulk(b"modules"),
        array(Vec::new()),
    ];
    if protocol == 2 {
        *nil = RESP2_NIL;
        return array(fields);
    }
    *nil = RESP3_NIL;
    let mut reply = format!("%{}\r\n", fields.len() / 2).into_bytes();
    reply.extend(fields.into_iter().flatten());
    reply
}

fn refused(failure: capnp::Error) -> anyhow::Result<Vec<u8>> {
    if failure.kind != capnp::ErrorKind::Failed {
        return Err(failure.into());
    }
    let message = failure.extra.strip_prefix("remote exception: ");
    Ok(error(message.unwrap_or(&failure.extra)))
}

fn remember(names: &Names, key: &str) {
    let id = kvs::key_id(key);
    if key_parse(key) == id {
        names.borrow_mut().insert(id, key.to_owned());
    }
}

fn display(names: &Names, id: u64) -> String {
    match names.borrow().get(&id) {
        Some(name) => name.clone(),
        None => key_display(id),
    }
}

async fn ids(portal: &kvs_portal::Client) -> anyhow::Result<Vec<u64>> {
    let keys = Rc::new(RefCell::new(Vec::new()));
    let mut request = portal.scan_request();
    request
        .get()
        .set_output(capnp_rpc::new_client(Stream::new(ScannedKeys {
            keys: keys.clone(),
        })));
    request.send().promise.await?;
    let mut ids = keys.take();
    ids.sort_unstable();
    Ok(ids)
}

fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    let (mut pattern_index, mut text_index) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while text_index < text.len() {
        match pattern.get(pattern_index) {
            Some(b'*') => {
                star = Some((pattern_index, text_index));
                pattern_index += 1;
            }
            Some(b'?') => {
                pattern_index += 1;
                text_index += 1;
            }
            Some(expected) if *expected == text[text_index] => {
                pattern_index += 1;
                text_index += 1;
            }
            _ => match star {
                Some((star_index, matched_up_to)) => {
                    pattern_index = star_index + 1;
                    text_index = matched_up_to + 1;
                    star = Some((star_index, text_index));
                }
                None => return false,
            },
        }
    }
    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}

const RESP2_NIL: &[u8] = b"$-1\r\n";
const RESP3_NIL: &[u8] = b"_\r\n";

fn string(value: &Value, nil: &[u8]) -> Option<Vec<u8>> {
    match value {
        Value::Null => Some(nil.to_vec()),
        Value::Uint(number) => Some(bulk(number.to_string().as_bytes())),
        Value::Bool(flag) => Some(bulk(if *flag { b"1" } else { b"0" })),
        Value::String(text) | Value::Text(text) => Some(bulk(text.as_bytes())),
        Value::Bytes(bytes) => Some(bulk(bytes)),
        Value::List(_) | Value::Record(_) => None,
    }
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn bulk(bytes: &[u8]) -> Vec<u8> {
    let mut reply = format!("${}\r\n", bytes.len()).into_bytes();
    reply.extend_from_slice(bytes);
    reply.extend_from_slice(b"\r\n");
    reply
}

fn integer(number: u64) -> Vec<u8> {
    format!(":{number}\r\n").into_bytes()
}

fn array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut reply = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        reply.extend_from_slice(&item);
    }
    reply
}

fn error(message: &str) -> Vec<u8> {
    let message: String = message
        .chars()
        .map(|character| match character {
            '\r' | '\n' => ' ',
            character => character,
        })
        .collect();
    format!("-ERR {message}\r\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glob_matches_like_redis() {
        assert!(glob_matches(b"session:*", b"session:abc"));
        assert!(glob_matches(b"h?llo", b"hello"));
        assert!(!glob_matches(b"h?llo", b"hllo"));
        assert!(glob_matches(b"*", b""));
        assert!(!glob_matches(b"a*b", b"acbd"));
        assert!(glob_matches(&[b'*'; 4096], b"anything at all"));
    }

    #[test]
    fn a_command_is_read_as_the_bytes_arrive() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut reader: &[u8] = b"*2\r\n$3\r\nGET\r\n$5\r\na\r\nbc\r\n";
            let command = read_command(&mut reader).await.unwrap().unwrap();
            assert_eq!(command, vec![b"GET".to_vec(), b"a\r\nbc".to_vec()]);

            let mut huge: &[u8] = b"*2\r\n$536870912\r\nx";
            let refused = read_command(&mut huge).await.unwrap_err();
            assert!(
                refused.to_string().contains("closed in the middle"),
                "{refused}"
            );
        });
    }
}
