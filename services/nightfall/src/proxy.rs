use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const SIGNATURE: [u8; 12] = [
    0x0d, 0x0a, 0x0d, 0x0a, 0x00, 0x0d, 0x0a, 0x51, 0x55, 0x49, 0x54, 0x0a,
];
const VERSION_PROXY: u8 = 0x21;
const VERSION_LOCAL: u8 = 0x20;
const TCP_OVER_IPV4: u8 = 0x11;
const TCP_OVER_IPV6: u8 = 0x21;
const UNSPECIFIED: u8 = 0x00;
pub const MAXIMUM_ADDRESS_BYTES: usize = 1024;

pub fn encode(source: SocketAddr, destination: SocketAddr) -> Vec<u8> {
    let (source, destination) = match (source, destination) {
        (SocketAddr::V4(source), SocketAddr::V6(destination)) => (
            SocketAddr::new(IpAddr::V6(source.ip().to_ipv6_mapped()), source.port()),
            SocketAddr::V6(destination),
        ),
        (SocketAddr::V6(source), SocketAddr::V4(destination)) => (
            SocketAddr::V6(source),
            SocketAddr::new(
                IpAddr::V6(destination.ip().to_ipv6_mapped()),
                destination.port(),
            ),
        ),
        pair => pair,
    };
    let mut header = SIGNATURE.to_vec();
    header.push(VERSION_PROXY);
    match (source, destination) {
        (SocketAddr::V4(source), SocketAddr::V4(destination)) => {
            header.push(TCP_OVER_IPV4);
            header.extend_from_slice(&12u16.to_be_bytes());
            header.extend_from_slice(&source.ip().octets());
            header.extend_from_slice(&destination.ip().octets());
            header.extend_from_slice(&source.port().to_be_bytes());
            header.extend_from_slice(&destination.port().to_be_bytes());
        }
        (SocketAddr::V6(source), SocketAddr::V6(destination)) => {
            header.push(TCP_OVER_IPV6);
            header.extend_from_slice(&36u16.to_be_bytes());
            header.extend_from_slice(&source.ip().octets());
            header.extend_from_slice(&destination.ip().octets());
            header.extend_from_slice(&source.port().to_be_bytes());
            header.extend_from_slice(&destination.port().to_be_bytes());
        }
        _ => unreachable!("both addresses were brought to one family"),
    }
    header
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    Proxied {
        source: SocketAddr,
        destination: SocketAddr,
    },
    Local,
}

#[derive(Debug)]
pub enum ProxyFailure {
    Signature,
    Version(u8),
    Family(u8),
    Length(usize),
    TimedOut,
    Io(std::io::Error),
}

impl std::fmt::Display for ProxyFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyFailure::Signature => write!(formatter, "no PROXY protocol v2 signature"),
            ProxyFailure::Version(byte) => {
                write!(formatter, "PROXY protocol version and command {byte:#04x}")
            }
            ProxyFailure::Family(byte) => {
                write!(formatter, "PROXY protocol address family {byte:#04x}")
            }
            ProxyFailure::Length(length) => {
                write!(formatter, "PROXY protocol address block of {length} bytes")
            }
            ProxyFailure::TimedOut => write!(formatter, "no PROXY protocol header in time"),
            ProxyFailure::Io(error) => {
                write!(formatter, "reading the PROXY header failed: {error}")
            }
        }
    }
}

pub fn decode_addresses(family: u8, block: &[u8]) -> Result<Header, ProxyFailure> {
    match family {
        TCP_OVER_IPV4 if block.len() >= 12 => {
            let source = Ipv4Addr::new(block[0], block[1], block[2], block[3]);
            let destination = Ipv4Addr::new(block[4], block[5], block[6], block[7]);
            Ok(Header::Proxied {
                source: SocketAddr::new(
                    IpAddr::V4(source),
                    u16::from_be_bytes([block[8], block[9]]),
                ),
                destination: SocketAddr::new(
                    IpAddr::V4(destination),
                    u16::from_be_bytes([block[10], block[11]]),
                ),
            })
        }
        TCP_OVER_IPV6 if block.len() >= 36 => {
            let mut source = [0u8; 16];
            let mut destination = [0u8; 16];
            source.copy_from_slice(&block[0..16]);
            destination.copy_from_slice(&block[16..32]);
            let canonical = |octets: [u8; 16]| Ipv6Addr::from(octets).to_canonical();
            Ok(Header::Proxied {
                source: SocketAddr::new(
                    canonical(source),
                    u16::from_be_bytes([block[32], block[33]]),
                ),
                destination: SocketAddr::new(
                    canonical(destination),
                    u16::from_be_bytes([block[34], block[35]]),
                ),
            })
        }
        TCP_OVER_IPV4 | TCP_OVER_IPV6 => Err(ProxyFailure::Length(block.len())),
        other => Err(ProxyFailure::Family(other)),
    }
}

pub async fn read_header(
    stream: &mut (impl AsyncRead + Unpin),
    timeout: Duration,
) -> Result<Header, ProxyFailure> {
    tokio::time::timeout(timeout, async {
        let mut fixed = [0u8; 16];
        stream
            .read_exact(&mut fixed)
            .await
            .map_err(ProxyFailure::Io)?;
        if fixed[..12] != SIGNATURE {
            return Err(ProxyFailure::Signature);
        }
        let length = u16::from_be_bytes([fixed[14], fixed[15]]) as usize;
        if length > MAXIMUM_ADDRESS_BYTES {
            return Err(ProxyFailure::Length(length));
        }
        let mut block = vec![0u8; length];
        stream
            .read_exact(&mut block)
            .await
            .map_err(ProxyFailure::Io)?;
        match fixed[12] {
            VERSION_LOCAL => Ok(Header::Local),
            VERSION_PROXY if fixed[13] == UNSPECIFIED => Ok(Header::Local),
            VERSION_PROXY => decode_addresses(fixed[13], &block),
            other => Err(ProxyFailure::Version(other)),
        }
    })
    .await
    .unwrap_or(Err(ProxyFailure::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_an_ipv4_header_byte_for_byte() {
        let header = encode(
            "192.0.2.10:51000".parse().unwrap(),
            "10.0.0.5:8445".parse().unwrap(),
        );
        let mut expected = SIGNATURE.to_vec();
        expected.extend_from_slice(&[0x21, 0x11, 0x00, 0x0c]);
        expected.extend_from_slice(&[192, 0, 2, 10, 10, 0, 0, 5]);
        expected.extend_from_slice(&51000u16.to_be_bytes());
        expected.extend_from_slice(&8445u16.to_be_bytes());
        assert_eq!(header, expected);
        assert_eq!(header.len(), 28);
    }

    #[test]
    fn encodes_ipv6_and_maps_a_mixed_pair_to_ipv6() {
        let header = encode(
            "[2001:db8::1]:40000".parse().unwrap(),
            "[2001:db8::2]:8445".parse().unwrap(),
        );
        assert_eq!(header.len(), 16 + 36);
        assert_eq!(&header[12..16], &[0x21, 0x21, 0x00, 0x24]);
        let mixed = encode(
            "192.0.2.10:51000".parse().unwrap(),
            "[2001:db8::2]:8445".parse().unwrap(),
        );
        assert_eq!(mixed[13], 0x21);
        assert_eq!(
            &mixed[16..32],
            &Ipv4Addr::new(192, 0, 2, 10).to_ipv6_mapped().octets()
        );
    }

    #[tokio::test]
    async fn reads_back_what_it_encodes_and_leaves_the_rest_of_the_stream() {
        for (source, destination) in [
            ("192.0.2.10:51000", "10.0.0.5:8445"),
            ("[2001:db8::1]:40000", "[2001:db8::2]:8445"),
        ] {
            let source: SocketAddr = source.parse().unwrap();
            let destination: SocketAddr = destination.parse().unwrap();
            let mut bytes = encode(source, destination);
            bytes.extend_from_slice(b"\x16\x03\x01rest");
            let mut reader = bytes.as_slice();
            let header = read_header(&mut reader, Duration::from_secs(1))
                .await
                .unwrap();
            assert_eq!(
                header,
                Header::Proxied {
                    source,
                    destination
                }
            );
            assert_eq!(reader, b"\x16\x03\x01rest");
        }
    }

    #[tokio::test]
    async fn skips_tlvs_and_accepts_a_local_command() {
        let mut bytes = SIGNATURE.to_vec();
        bytes.extend_from_slice(&[0x21, 0x11, 0x00, 0x10]);
        bytes.extend_from_slice(&[192, 0, 2, 10, 10, 0, 0, 5, 0, 80, 0, 81]);
        bytes.extend_from_slice(&[0x04, 0x00, 0x01, 0x00]);
        let mut reader = bytes.as_slice();
        let header = read_header(&mut reader, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(matches!(header, Header::Proxied { .. }));
        assert!(reader.is_empty());
        let mut local = SIGNATURE.to_vec();
        local.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]);
        let mut reader = local.as_slice();
        assert_eq!(
            read_header(&mut reader, Duration::from_secs(1))
                .await
                .unwrap(),
            Header::Local
        );
    }

    #[tokio::test]
    async fn refuses_a_stream_without_a_valid_header() {
        let mut reader: &[u8] = b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03abcdefghijkl";
        assert!(matches!(
            read_header(&mut reader, Duration::from_secs(1)).await,
            Err(ProxyFailure::Signature)
        ));
        let mut bytes = SIGNATURE.to_vec();
        bytes.extend_from_slice(&[0x21, 0x31, 0x00, 0x00]);
        let mut reader = bytes.as_slice();
        assert!(matches!(
            read_header(&mut reader, Duration::from_secs(1)).await,
            Err(ProxyFailure::Family(0x31))
        ));
        let mut short = SIGNATURE.to_vec();
        short.extend_from_slice(&[0x21, 0x11, 0x00, 0x04, 1, 2, 3, 4]);
        let mut reader = short.as_slice();
        assert!(matches!(
            read_header(&mut reader, Duration::from_secs(1)).await,
            Err(ProxyFailure::Length(4))
        ));
        let mut version = SIGNATURE.to_vec();
        version.extend_from_slice(&[0x11, 0x11, 0x00, 0x00]);
        let mut reader = version.as_slice();
        assert!(matches!(
            read_header(&mut reader, Duration::from_secs(1)).await,
            Err(ProxyFailure::Version(0x11))
        ));
    }
}
