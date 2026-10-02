// SPDX-License-Identifier: GPL-3.0-or-later
//! SHA-256 and minisign checks (plan 9.4): a file whose signature or sum is wrong is refused.

use sha2::{Digest, Sha256};

/// The public key the releases are signed with (`packaging/minisign.pub`). Until the maintainer
/// has generated the key pair it is a placeholder and no update is accepted.
pub const PUBLIC_KEY: &str = include_str!("../../../packaging/minisign.pub");

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("no signing key is configured in this build")]
    NoKey,
    #[error("the signature is invalid: {0}")]
    Signature(String),
    #[error("the checksum does not match")]
    Checksum,
    #[error("the file is not in the checksum list")]
    NotListed,
}

/// Whether this build carries a real signing key.
pub fn has_key() -> bool {
    check_signature(b"", "", PUBLIC_KEY) != Err(VerifyError::NoKey)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The sum of `name` in a `SHA256SUMS` file (`<hex>  <name>` per line, `*` before the name for
/// binary mode).
pub fn listed_sum<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(sum.trim())
    })
}

pub fn check_sum(bytes: &[u8], sums: &str, name: &str) -> Result<(), VerifyError> {
    let wanted = listed_sum(sums, name).ok_or(VerifyError::NotListed)?;
    if wanted.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        Ok(())
    } else {
        Err(VerifyError::Checksum)
    }
}

/// Checks `signature` (the text of a `.minisig` file) of `bytes` against `public_key` (the text
/// of a `.pub` file, or the bare base64 key).
pub fn check_signature(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), VerifyError> {
    let key_text = public_key.trim();
    let key = if key_text.contains('\n') {
        minisign_verify::PublicKey::decode(key_text)
    } else {
        minisign_verify::PublicKey::from_base64(key_text)
    }
    .map_err(|_| VerifyError::NoKey)?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| VerifyError::Signature(e.to_string()))?;
    key.verify(bytes, &signature, true)
        .map_err(|e| VerifyError::Signature(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The example of the minisign-verify documentation.
    const KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG: &str = "untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966\tfile:test
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

    #[test]
    fn a_valid_signature_passes_and_a_changed_file_does_not() {
        assert_eq!(check_signature(b"test", SIG, KEY), Ok(()));
        assert!(matches!(
            check_signature(b"tampered", SIG, KEY),
            Err(VerifyError::Signature(_))
        ));
    }

    #[test]
    fn the_placeholder_key_accepts_nothing() {
        assert_eq!(
            check_signature(b"test", SIG, "unconfigured"),
            Err(VerifyError::NoKey)
        );
    }

    #[test]
    fn sums_are_looked_up_by_file_name() {
        let sums = format!("{}  a.zip\n{} *b.zip\n", sha256_hex(b"a"), sha256_hex(b"b"));
        assert_eq!(check_sum(b"a", &sums, "a.zip"), Ok(()));
        assert_eq!(check_sum(b"b", &sums, "b.zip"), Ok(()));
        assert_eq!(check_sum(b"x", &sums, "a.zip"), Err(VerifyError::Checksum));
        assert_eq!(check_sum(b"a", &sums, "c.zip"), Err(VerifyError::NotListed));
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
