use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::Instant;

pub const RECORD_HEADER_BYTES: usize = 5;
pub const MAXIMUM_RECORD_BYTES: usize = 16 * 1024;
pub const MAXIMUM_PEEK_BYTES: usize = MAXIMUM_RECORD_BYTES + RECORD_HEADER_BYTES;
pub const UNRECOGNIZED_NAME_ALERT: [u8; 7] = [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x70];

const HANDSHAKE_RECORD: u8 = 0x16;
const CLIENT_HELLO: u8 = 0x01;
const SERVER_NAME_EXTENSION: u16 = 0x0000;
const HOST_NAME: u8 = 0x00;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Peeked {
    NeedMore(usize),
    NotTls,
    ClientHello { server_name: Option<String> },
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(length)?;
        let taken = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|taken| taken[0])
    }

    fn short(&mut self) -> Option<u16> {
        self.take(2)
            .map(|taken| u16::from_be_bytes([taken[0], taken[1]]))
    }

    fn vector8(&mut self) -> Option<&'a [u8]> {
        let length = self.byte()? as usize;
        self.take(length)
    }

    fn vector16(&mut self) -> Option<&'a [u8]> {
        let length = self.short()? as usize;
        self.take(length)
    }
}

fn server_name_of(extension: &[u8]) -> Option<String> {
    let mut list = Cursor {
        bytes: extension,
        position: 0,
    };
    let names = list.vector16()?;
    let mut names = Cursor {
        bytes: names,
        position: 0,
    };
    while names.position < names.bytes.len() {
        let kind = names.byte()?;
        let name = names.vector16()?;
        if kind == HOST_NAME {
            if name.is_empty() || !name.iter().all(|byte| byte.is_ascii_graphic()) {
                return None;
            }
            return Some(String::from_utf8_lossy(name).to_ascii_lowercase());
        }
    }
    None
}

fn hello_server_name(hello: &[u8]) -> Option<Option<String>> {
    let mut cursor = Cursor {
        bytes: hello,
        position: 0,
    };
    cursor.take(2)?;
    cursor.take(32)?;
    cursor.vector8()?;
    cursor.vector16()?;
    cursor.vector8()?;
    if cursor.position == hello.len() {
        return Some(None);
    }
    let extensions = cursor.vector16()?;
    let mut extensions = Cursor {
        bytes: extensions,
        position: 0,
    };
    while extensions.position < extensions.bytes.len() {
        let kind = extensions.short()?;
        let data = extensions.vector16()?;
        if kind == SERVER_NAME_EXTENSION {
            return Some(server_name_of(data));
        }
    }
    Some(None)
}

pub fn parse(bytes: &[u8]) -> Peeked {
    if bytes.is_empty() {
        return Peeked::NeedMore(RECORD_HEADER_BYTES);
    }
    if bytes[0] != HANDSHAKE_RECORD {
        return Peeked::NotTls;
    }
    if bytes.len() < RECORD_HEADER_BYTES {
        return Peeked::NeedMore(RECORD_HEADER_BYTES);
    }
    if bytes[1] != 0x03 {
        return Peeked::NotTls;
    }
    let record_length = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
    if record_length == 0 || record_length > MAXIMUM_RECORD_BYTES {
        return Peeked::NotTls;
    }
    let total = RECORD_HEADER_BYTES + record_length;
    if bytes.len() < total {
        return Peeked::NeedMore(total);
    }
    let record = &bytes[RECORD_HEADER_BYTES..total];
    if record.len() < 4 || record[0] != CLIENT_HELLO {
        return Peeked::NotTls;
    }
    let hello_length = u32::from_be_bytes([0, record[1], record[2], record[3]]) as usize;
    let available = &record[4..];
    let hello = &available[..hello_length.min(available.len())];
    match hello_server_name(hello) {
        Some(server_name) => Peeked::ClientHello { server_name },
        None if hello_length > available.len() => Peeked::ClientHello { server_name: None },
        None => Peeked::NotTls,
    }
}

#[derive(Debug)]
pub enum PeekFailure {
    TimedOut,
    Closed,
    NotTls,
    Io(std::io::Error),
}

impl std::fmt::Display for PeekFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeekFailure::TimedOut => write!(formatter, "no complete ClientHello record in time"),
            PeekFailure::Closed => write!(formatter, "the peer closed before a ClientHello"),
            PeekFailure::NotTls => write!(formatter, "the first record is not a TLS ClientHello"),
            PeekFailure::Io(error) => write!(formatter, "peeking the ClientHello failed: {error}"),
        }
    }
}

pub async fn peek_server_name(
    stream: &TcpStream,
    timeout: Duration,
) -> Result<Option<String>, PeekFailure> {
    let deadline = Instant::now() + timeout;
    let mut buffer = vec![0u8; MAXIMUM_PEEK_BYTES];
    let mut seen = 0usize;
    let mut pause = Duration::from_millis(1);
    loop {
        let peeked = match tokio::time::timeout_at(deadline, stream.peek(&mut buffer)).await {
            Err(_) => return Err(PeekFailure::TimedOut),
            Ok(Err(error)) => return Err(PeekFailure::Io(error)),
            Ok(Ok(0)) => return Err(PeekFailure::Closed),
            Ok(Ok(count)) => count,
        };
        match parse(&buffer[..peeked]) {
            Peeked::ClientHello { server_name } => return Ok(server_name),
            Peeked::NotTls => return Err(PeekFailure::NotTls),
            Peeked::NeedMore(_) => {}
        }
        if peeked > seen {
            seen = peeked;
            pause = Duration::from_millis(1);
        } else {
            pause = (pause * 2).min(Duration::from_millis(50));
        }
        if Instant::now() + pause >= deadline {
            return Err(PeekFailure::TimedOut);
        }
        tokio::time::sleep(pause).await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    pub(crate) fn client_hello(server_name: Option<&str>, padding: usize) -> Vec<u8> {
        let mut extensions = Vec::new();
        extensions.extend_from_slice(&0x000au16.to_be_bytes());
        extensions.extend_from_slice(&4u16.to_be_bytes());
        extensions.extend_from_slice(&[0x00, 0x02, 0x00, 0x1d]);
        if let Some(name) = server_name {
            let mut list = vec![HOST_NAME];
            list.extend_from_slice(&(name.len() as u16).to_be_bytes());
            list.extend_from_slice(name.as_bytes());
            let mut data = (list.len() as u16).to_be_bytes().to_vec();
            data.extend_from_slice(&list);
            extensions.extend_from_slice(&SERVER_NAME_EXTENSION.to_be_bytes());
            extensions.extend_from_slice(&(data.len() as u16).to_be_bytes());
            extensions.extend_from_slice(&data);
        }
        extensions.extend_from_slice(&0x0015u16.to_be_bytes());
        extensions.extend_from_slice(&(padding as u16).to_be_bytes());
        extensions.extend(std::iter::repeat_n(0u8, padding));
        let mut hello = vec![0x03, 0x03];
        hello.extend_from_slice(&[7u8; 32]);
        hello.push(32);
        hello.extend_from_slice(&[9u8; 32]);
        hello.extend_from_slice(&4u16.to_be_bytes());
        hello.extend_from_slice(&[0x13, 0x01, 0x13, 0x02]);
        hello.extend_from_slice(&[1, 0]);
        hello.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        hello.extend_from_slice(&extensions);
        let mut handshake = vec![CLIENT_HELLO];
        handshake.extend_from_slice(&(hello.len() as u32).to_be_bytes()[1..]);
        handshake.extend_from_slice(&hello);
        let mut record = vec![HANDSHAKE_RECORD, 0x03, 0x01];
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);
        record
    }

    #[test]
    fn finds_the_server_name_in_a_client_hello() {
        let hello = client_hello(Some("Fleet.Dusk.Example"), 10);
        assert_eq!(
            parse(&hello),
            Peeked::ClientHello {
                server_name: Some("fleet.dusk.example".to_string())
            }
        );
        assert_eq!(
            parse(&client_hello(None, 0)),
            Peeked::ClientHello { server_name: None }
        );
    }

    #[test]
    fn asks_for_the_rest_of_a_record_split_across_segments() {
        let hello = client_hello(Some("0123456789abcdef.fleet.dusk.example"), 900);
        assert_eq!(parse(&hello[..0]), Peeked::NeedMore(5));
        assert_eq!(parse(&hello[..3]), Peeked::NeedMore(5));
        assert_eq!(parse(&hello[..100]), Peeked::NeedMore(hello.len()));
        assert_eq!(
            parse(&hello[..hello.len() - 1]),
            Peeked::NeedMore(hello.len())
        );
        assert!(matches!(
            parse(&hello),
            Peeked::ClientHello {
                server_name: Some(_)
            }
        ));
    }

    #[test]
    fn refuses_what_is_not_a_client_hello() {
        assert_eq!(parse(b"GET / HTTP/1.1\r\n"), Peeked::NotTls);
        assert_eq!(parse(&[0x16, 0x03, 0x01, 0x00, 0x00]), Peeked::NotTls);
        assert_eq!(parse(&[0x16, 0x03, 0x01, 0x40, 0x01]), Peeked::NotTls);
        let mut server_hello = client_hello(Some("a.example"), 0);
        server_hello[5] = 0x02;
        assert_eq!(parse(&server_hello), Peeked::NotTls);
        let mut broken = client_hello(Some("a.example"), 0);
        let length = broken.len();
        broken[length - 32] = 0xff;
        broken[length - 31] = 0xff;
        assert_eq!(parse(&broken), Peeked::NotTls);
    }

    #[test]
    fn refuses_server_names_that_are_not_printable_ascii() {
        let hello = client_hello(Some("bad name.example"), 0);
        assert_eq!(parse(&hello), Peeked::ClientHello { server_name: None });
    }

    #[test]
    fn reads_the_server_name_of_a_hello_longer_than_its_first_record() {
        let mut hello = client_hello(Some("fleet.dusk.example"), 64);
        let handshake_length = (hello.len() - 9 + 5000) as u32;
        hello[6..9].copy_from_slice(&handshake_length.to_be_bytes()[1..]);
        assert_eq!(
            parse(&hello),
            Peeked::ClientHello {
                server_name: Some("fleet.dusk.example".to_string())
            }
        );
    }

    #[tokio::test]
    async fn peeks_a_client_hello_sent_in_pieces_without_consuming_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hello = client_hello(Some("0123456789abcdef.fleet.dusk.example"), 1200);
        let sent = hello.clone();
        let writer = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.set_nodelay(true).unwrap();
            for piece in sent.chunks(300) {
                stream.write_all(piece).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            stream
        });
        let (stream, _) = listener.accept().await.unwrap();
        let name = peek_server_name(&stream, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("0123456789abcdef.fleet.dusk.example"));
        let mut received = vec![0u8; hello.len()];
        let mut stream = stream;
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut received)
            .await
            .unwrap();
        assert_eq!(received, hello);
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn gives_up_on_a_silent_peer_at_the_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let started = std::time::Instant::now();
        let failure = peek_server_name(&stream, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(matches!(failure, PeekFailure::TimedOut));
        assert!(started.elapsed() >= Duration::from_millis(150));
        drop(client);
    }
}
