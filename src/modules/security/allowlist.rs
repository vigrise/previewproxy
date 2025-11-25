use ipnet::IpNet;
use std::net::IpAddr;

pub struct Allowlist {
  entries: Vec<String>,
  open: bool,
}

impl Allowlist {
  pub fn new(entries: Vec<String>) -> Self {
    let open = entries.is_empty();
    Self { entries, open }
  }

  pub fn is_allowed(&self, host: &str) -> bool {
    if self.open {
      return true;
    }
    self.entries.iter().any(|entry| matches_host(entry, host))
  }
}

fn matches_host(pattern: &str, host: &str) -> bool {
  if let Some(suffix) = pattern.strip_prefix("*.") {
    // Wildcard: host must be exactly one label + "." + suffix
    if let Some(rest) = host.strip_suffix(suffix) {
      let label = rest.strip_suffix('.').unwrap_or(rest);
      return !label.is_empty() && !label.contains('.');
    }
    false
  } else {
    pattern == host
  }
}

static PRIVATE_RANGES: &[&str] = &[
  "10.0.0.0/8",
  "172.16.0.0/12",
  "192.168.0.0/16",
  "127.0.0.0/8",
  "169.254.0.0/16",
  "0.0.0.0/8",
  "100.64.0.0/10",
  "198.18.0.0/15",
  "::1/128",
  "fe80::/10",
  "fc00::/7",
];

pub fn is_private_ip(ip: IpAddr) -> bool {
  PRIVATE_RANGES.iter().any(|range| {
    range
      .parse::<IpNet>()
      .map(|net| net.contains(&ip))
      .unwrap_or(false)
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn test_exact_match() {
    let list = Allowlist::new(vec!["example.com".to_string()]);
    assert!(list.is_allowed("example.com"));
    assert!(!list.is_allowed("www.example.com"));
    assert!(!list.is_allowed("other.com"));
  }
  #[test]
  fn test_wildcard_single_label() {
    let list = Allowlist::new(vec!["*.example.com".to_string()]);
    assert!(list.is_allowed("www.example.com"));
    assert!(!list.is_allowed("a.b.example.com"));
    assert!(!list.is_allowed("example.com"));
  }
  #[test]
  fn test_empty_allows_all() {
    let list = Allowlist::new(vec![]);
    assert!(list.is_allowed("anything.com"));
    assert!(list.is_allowed("10.0.0.1")); // open mode ignores private IP check in allowlist
  }
  #[test]
  fn test_private_ip_always_blocked() {
    use std::net::IpAddr;
    assert!(is_private_ip("10.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(is_private_ip("192.168.1.1".parse::<IpAddr>().unwrap()));
    assert!(is_private_ip("127.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(is_private_ip("169.254.1.1".parse::<IpAddr>().unwrap()));
    assert!(is_private_ip("172.16.0.1".parse::<IpAddr>().unwrap()));
    assert!(!is_private_ip("8.8.8.8".parse::<IpAddr>().unwrap()));
    assert!(!is_private_ip("1.1.1.1".parse::<IpAddr>().unwrap()));
  }
}
