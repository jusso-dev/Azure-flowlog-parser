use crate::model::ReportScope;

/// IPv4 longest-prefix-match table mapping address ranges to scopes.
#[derive(Debug, Default, Clone)]
pub struct Ipv4Lookup {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    network: u32,
    mask: u32,
    length: u8,
    scope: ReportScope,
}

impl Ipv4Lookup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a CIDR ("10.0.0.0/16") or a single address (treated as /32). Returns false for non-IPv4 input.
    pub fn try_add(&mut self, cidr: &str, scope: ReportScope) -> bool {
        let (address, length) = match cidr.split_once('/') {
            Some((address, length)) => match length.parse::<u8>() {
                Ok(length) if length <= 32 => (address, length),
                _ => return false,
            },
            None => (cidr, 32),
        };
        let Some(network) = parse_ipv4(address) else {
            return false;
        };
        let mask = prefix_mask(length);
        self.entries.push(Entry { network: network & mask, mask, length, scope });
        true
    }

    /// Most specific scope containing the address. On equal prefix length the entry added first wins.
    pub fn find(&self, address: u32) -> Option<&ReportScope> {
        let mut best: Option<&Entry> = None;
        for entry in &self.entries {
            let contains = address & entry.mask == entry.network;
            if contains && best.is_none_or(|current| entry.length > current.length) {
                best = Some(entry);
            }
        }
        best.map(|entry| &entry.scope)
    }
}

fn prefix_mask(length: u8) -> u32 {
    if length == 0 { 0 } else { u32::MAX << (32 - u32::from(length)) }
}

/// Parses dotted-quad IPv4 into a big-endian u32. None for anything else (including IPv6).
pub fn parse_ipv4(text: &str) -> Option<u32> {
    let mut value = 0u32;
    let mut octets = 0;
    for part in text.split('.') {
        if octets == 4 || part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let octet: u32 = part.parse().ok().filter(|octet| *octet <= 255)?;
        value = (value << 8) | octet;
        octets += 1;
    }
    (octets == 4).then_some(value)
}

/// Private, shared and link-local ranges: 10/8, 172.16/12, 192.168/16, 100.64/10, 169.254/16.
pub fn is_private(address: u32) -> bool {
    let [a, b, _, _] = address.to_be_bytes();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::scope_kind;

    fn scope(id: &str) -> ReportScope {
        ReportScope { id: id.to_string(), kind: scope_kind::STORAGE.to_string(), ..Default::default() }
    }

    fn address(text: &str) -> u32 {
        parse_ipv4(text).unwrap()
    }

    #[test]
    fn longest_prefix_wins_regardless_of_insertion_order() {
        let mut lookup = Ipv4Lookup::new();
        assert!(lookup.try_add("10.0.0.0/8", scope("wide")));
        assert!(lookup.try_add("10.10.0.0/16", scope("subnet")));
        assert!(lookup.try_add("10.10.1.4/32", scope("nic")));

        assert_eq!(lookup.find(address("10.10.1.4")).unwrap().id, "nic");
        assert_eq!(lookup.find(address("10.10.1.5")).unwrap().id, "subnet");
        assert_eq!(lookup.find(address("10.200.0.1")).unwrap().id, "wide");
        assert!(lookup.find(address("11.0.0.1")).is_none());
    }

    #[test]
    fn equal_length_prefixes_keep_first_added() {
        let mut lookup = Ipv4Lookup::new();
        lookup.try_add("10.1.0.0/24", scope("first"));
        lookup.try_add("10.1.0.0/24", scope("second"));
        assert_eq!(lookup.find(address("10.1.0.9")).unwrap().id, "first");
    }

    #[test]
    fn prefix_host_bits_are_masked_and_zero_length_matches_everything() {
        let mut lookup = Ipv4Lookup::new();
        lookup.try_add("10.1.2.3/16", scope("masked"));
        lookup.try_add("0.0.0.0/0", scope("default"));
        assert_eq!(lookup.find(address("10.1.200.1")).unwrap().id, "masked");
        assert_eq!(lookup.find(address("192.0.2.1")).unwrap().id, "default");
    }

    #[test]
    fn try_add_rejects_ipv6_and_bad_lengths() {
        let mut lookup = Ipv4Lookup::new();
        assert!(!lookup.try_add("2001:db8::/32", scope("v6")));
        assert!(!lookup.try_add("10.0.0.0/33", scope("long")));
        assert!(!lookup.try_add("10.0.0.0/x", scope("text")));
        assert!(!lookup.try_add("10.0.0/16", scope("short")));
        assert!(lookup.find(address("10.0.0.1")).is_none());
    }

    #[test]
    fn parse_ipv4_accepts_only_dotted_quads() {
        assert_eq!(parse_ipv4("1.2.3.4"), Some(0x0102_0304));
        assert_eq!(parse_ipv4("255.255.255.255"), Some(u32::MAX));
        for bad in ["", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1.2.3.-4", "1..3.4", "::1", "10.0.0.1/32", "a.b.c.d"] {
            assert_eq!(parse_ipv4(bad), None, "{bad}");
        }
    }

    #[test]
    fn private_ranges_match_spec() {
        for private in [
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.5.5",
            "100.64.0.1",
            "100.127.255.255",
            "169.254.1.1",
        ] {
            assert!(is_private(address(private)), "{private}");
        }
        for public in [
            "9.255.255.255",
            "172.15.255.255",
            "172.32.0.0",
            "192.169.0.1",
            "100.63.255.255",
            "100.128.0.0",
            "169.253.0.1",
            "13.1.2.3",
        ] {
            assert!(!is_private(address(public)), "{public}");
        }
    }
}
