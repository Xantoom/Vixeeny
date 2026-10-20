// SPDX-License-Identifier: GPL-3.0-or-later
//! SHA-256 and minisign checks: a file whose signature or sum is wrong is refused.

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
    #[error("cannot read the file: {0}")]
    Read(String),
}

/// Whether this build carries a real signing key.
pub fn has_key() -> bool {
    key(PUBLIC_KEY).is_ok()
}

fn key(public_key: &str) -> Result<minisign_verify::PublicKey, VerifyError> {
    let text = public_key.trim();
    if text.contains('\n') {
        minisign_verify::PublicKey::decode(text)
    } else {
        minisign_verify::PublicKey::from_base64(text)
    }
    .map_err(|_| VerifyError::NoKey)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The sum of `name` in a `SHA256SUMS` file (`<hex>  <name>` per line, `*` before the name for
/// binary mode).
pub fn listed_sum<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(sum.trim())
    })
}

/// The file a signature was made for: the `file:` field of its trusted comment (signed with the
/// file), without the folder it was signed in.
fn signed_name(signature: &minisign_verify::Signature) -> Option<&str> {
    let path = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("file:"))?;
    Some(path.rsplit(['\\', '/']).next().unwrap_or(path))
}

/// Checks the file `reader` gives, named `name`, against `signature` (the text of a `.minisig`
/// file) and `public_key` (the text of a `.pub` file, or the bare base64 key), and against
/// `sums` (a `SHA256SUMS` file) when there is one. The signature must have been made for a file
/// of this name: an older signed archive cannot pass for a newer version. Read once, in pieces.
pub fn check(
    mut reader: impl std::io::Read,
    name: &str,
    signature: &str,
    sums: Option<&str>,
    public_key: &str,
) -> Result<(), VerifyError> {
    let key = key(public_key)?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| VerifyError::Signature(e.to_string()))?;
    if signed_name(&signature) != Some(name) {
        return Err(VerifyError::Signature(format!(
            "made for another file than {name}"
        )));
    }
    let wanted = sums
        .map(|sums| listed_sum(sums, name).ok_or(VerifyError::NotListed))
        .transpose()?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|e| VerifyError::Signature(e.to_string()))?;
    let mut sum = Sha256::new();
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| VerifyError::Read(e.to_string()))?;
        if n == 0 {
            break;
        }
        verifier.update(&chunk[..n]);
        sum.update(&chunk[..n]);
    }
    if wanted.is_some_and(|wanted| !wanted.eq_ignore_ascii_case(&hex(&sum.finalize()))) {
        return Err(VerifyError::Checksum);
    }
    verifier
        .finalize()
        .map_err(|e| VerifyError::Signature(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    // Made with rsign2 for a file `Vixeeny-1.2.3-windows-x64.zip` holding `test`.
    const KEY: &str = "RWQDEdvIFG2/xcFcP0F/kgAuNjNDd0Soa5exPBdT85E2tu7atBLGtM55";
    const SIG: &str = "untrusted comment: signature from rsign secret key
RUQDEdvIFG2/xQpVWXAw3lqcmLeWFaZlgwYfG6gaUW2/1iPMEC2NpIqelHSbsqiW3Z2CgksxfbnM+IZpIQRc5qpoXwBgw+5p5AI=
trusted comment: timestamp:1792508596\tfile:Vixeeny-1.2.3-windows-x64.zip\tprehashed
soBvvaiVI9xsDNkzPMKTpYZDzJvz9R8zHbDwKmHxi+xxKBomLYpy53BUnmOnqiMEjHLD80o0IcHmStrv9W0HDQ==";
    const NAME: &str = "Vixeeny-1.2.3-windows-x64.zip";

    fn check_bytes(
        bytes: &[u8],
        name: &str,
        sums: Option<&str>,
        key: &str,
    ) -> Result<(), VerifyError> {
        check(bytes, name, SIG, sums, key)
    }

    #[test]
    fn a_valid_signature_passes_and_a_changed_file_does_not() {
        assert_eq!(check_bytes(b"test", NAME, None, KEY), Ok(()));
        assert!(matches!(
            check_bytes(b"tampered", NAME, None, KEY),
            Err(VerifyError::Signature(_))
        ));
    }

    #[test]
    fn a_signature_of_another_version_is_refused() {
        assert!(matches!(
            check_bytes(b"test", "Vixeeny-9.0.0-windows-x64.zip", None, KEY),
            Err(VerifyError::Signature(_))
        ));
    }

    #[test]
    fn the_folder_it_was_signed_in_does_not_matter() {
        let sig = SIG.replace("file:", r"file:D:\a\dist\");
        let signature = minisign_verify::Signature::decode(&sig).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(signed_name(&signature), Some(NAME));
    }

    #[test]
    fn the_placeholder_key_accepts_nothing() {
        assert_eq!(
            check_bytes(b"test", NAME, None, "unconfigured"),
            Err(VerifyError::NoKey)
        );
    }

    #[test]
    fn sums_are_looked_up_by_file_name_when_there_are_some() {
        let good = format!(
            "{}  {NAME}\n{} *b.zip\n",
            sha256_hex(b"test"),
            sha256_hex(b"b")
        );
        assert_eq!(check_bytes(b"test", NAME, Some(&good), KEY), Ok(()));
        let wrong = format!("{} *{NAME}\n", sha256_hex(b"x"));
        assert_eq!(
            check_bytes(b"test", NAME, Some(&wrong), KEY),
            Err(VerifyError::Checksum)
        );
        assert_eq!(
            check_bytes(b"test", NAME, Some("00  other.zip"), KEY),
            Err(VerifyError::NotListed)
        );
    }
}
