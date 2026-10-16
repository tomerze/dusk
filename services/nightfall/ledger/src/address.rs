use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ring::hmac;

use crate::entry::hex;

pub const KEYED_HASH_LABEL: &str = "dusk-remote-address-v1:";

pub fn network(address: IpAddr) -> String {
    match address.to_canonical() {
        IpAddr::V4(ip) => format!("{}/24", Ipv4Addr::from(u32::from(ip) & 0xffff_ff00)),
        IpAddr::V6(ip) => format!(
            "{}/48",
            Ipv6Addr::from(u128::from(ip) & !((1u128 << 80) - 1))
        ),
    }
}

pub fn keyed_hash(key: &[u8], address: IpAddr) -> String {
    let input = format!("{KEYED_HASH_LABEL}{}", address.to_canonical());
    hex(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), input.as_bytes()).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_v4_slash_24_and_a_v6_slash_48() {
        assert_eq!(network("203.0.113.24".parse().unwrap()), "203.0.113.0/24");
        assert_eq!(
            network("::ffff:203.0.113.24".parse().unwrap()),
            "203.0.113.0/24"
        );
        assert_eq!(
            network("2001:db8:85a3:8d3:1319:8a2e:370:7348".parse().unwrap()),
            "2001:db8:85a3::/48"
        );
    }

    #[test]
    fn hashes_the_canonical_address_under_its_label() {
        let key = [0x0b; 32];
        let mapped = keyed_hash(&key, "::ffff:203.0.113.24".parse().unwrap());
        assert_eq!(mapped, keyed_hash(&key, "203.0.113.24".parse().unwrap()));
        assert_eq!(
            mapped,
            "665b3fceda8816d568b2a986d9e083dbf9a96b81244d6f553425bfd9bde7f88b"
        );
        assert_ne!(mapped, keyed_hash(&key, "203.0.113.25".parse().unwrap()));
        assert_ne!(
            mapped,
            keyed_hash(&[0x0c; 32], "203.0.113.24".parse().unwrap())
        );
    }
}
