//! Release versions: dotted numbers (`0.1.17`), with an optional leading `v`.

use std::cmp::Ordering;

/// A parsed version. Compared number by number; a missing trailing number counts as zero.
#[derive(Clone, Debug)]
pub struct Version(Vec<u64>);

impl Version {
    /// `0.1.17` or `v0.1.17`. Anything else (a suffix, an empty part) is not a version.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        if text.is_empty() {
            return None;
        }
        let parts = text
            .split('.')
            .map(|part| {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                part.parse::<u64>().ok()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self(parts))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.0.len().max(other.0.len());
        for i in 0..len {
            let (a, b) = (
                self.0.get(i).copied().unwrap_or(0),
                other.0.get(i).copied().unwrap_or(0),
            );
            match a.cmp(&b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u64::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_with_and_without_v() {
        assert_eq!(v("v0.1.17"), v("0.1.17"));
        assert_eq!(v("0.1.17").to_string(), "0.1.17");
    }

    #[test]
    fn rejects_non_versions() {
        for bad in ["", "v", "0..1", "0.1.x", "0.1.17-beta", "latest", "-1"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn orders_numerically_not_textually() {
        assert!(v("0.1.10") > v("0.1.9"));
        assert!(v("0.2.0") > v("0.1.99"));
        assert!(v("1.0") == v("1.0.0"));
        assert!(v("0.0.0") < v("0.1.1"));
    }
}
