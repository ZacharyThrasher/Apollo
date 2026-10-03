use std::fmt;

use zeroize::Zeroize;

/// A string that must never be printed, logged or persisted in clear. `Debug` and
/// `Display` are redacted; the contents are zeroed on drop (best effort).
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Deliberately named: every call site is a place a secret leaves its wrapper.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Clone for SecretString {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_formats_its_contents() {
        let s = SecretString::new("hunter2-canary");
        assert!(!format!("{s:?}").contains("canary"));
        assert!(!format!("{s}").contains("canary"));
        assert!(!format!("{:#?}", Some(&s)).contains("canary"));
        assert_eq!(s.expose(), "hunter2-canary");
    }
}
