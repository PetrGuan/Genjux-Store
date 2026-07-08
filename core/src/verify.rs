//! Checksum verification (issue #10).
//!
//! Per `.copilot-workflow/PLAN.md` section 4/5: if a release publishes a
//! checksum (a manifest like `checksums.txt`/`SHA256SUMS`, or a per-file
//! `<name>.sha256`), we compare against it. Otherwise we compute the
//! SHA-256 ourselves and hand it back to the caller to *display*, per the
//! trust model — a self-computed hash with nothing to compare against is
//! not an assertion that the file is safe, just a fact the user can look
//! up elsewhere if they want to.

use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch: expected {expected}, computed {computed}")]
    Mismatch { expected: String, computed: String },
}

/// Computes the SHA-256 hex digest of a file already on disk.
pub async fn sha256_file(path: &Path) -> Result<String, VerifyError> {
    let bytes = tokio::fs::read(path).await?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Verifies a downloaded file against an expected SHA-256 hex digest
/// (case-insensitive, surrounding whitespace ignored). By default this
/// should gate install progression on failure (see PLAN.md section 4).
pub async fn verify_sha256(path: &Path, expected_hex: &str) -> Result<(), VerifyError> {
    let computed = sha256_file(path).await?;
    if computed.eq_ignore_ascii_case(expected_hex.trim()) {
        Ok(())
    } else {
        Err(VerifyError::Mismatch {
            expected: expected_hex.trim().to_string(),
            computed,
        })
    }
}

/// Parses a multi-file checksum manifest (the `<hex-digest>  <filename>`
/// or `<hex-digest> *<filename>` format produced by `sha256sum`/
/// `shasum -a 256`, which is what most GitHub releases publish as
/// `checksums.txt` / `SHA256SUMS`) and looks up the digest for one
/// specific filename. Blank lines and `#`-comments are ignored.
pub fn parse_checksum_manifest(contents: &str, filename: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let hex = parts.next()?;
        let rest = parts.next()?.trim_start().trim_start_matches('*');
        if rest == filename {
            return Some(hex.to_string());
        }
    }
    None
}

/// Parses a single-file checksum file (the common `<asset-name>.sha256`
/// convention). These files vary in the wild: some contain just the bare
/// hex digest, others use the same `<hex>  <filename>` format as a full
/// manifest. Tries the manifest format first, then falls back to "the
/// first token on the first line, if it looks like a SHA-256 hex digest".
pub fn parse_single_checksum_file(contents: &str, filename: &str) -> Option<String> {
    if let Some(found) = parse_checksum_manifest(contents, filename) {
        return Some(found);
    }
    let first_token = contents.trim().lines().next()?.split_whitespace().next()?;
    is_sha256_hex(first_token).then(|| first_token.to_string())
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sha256_file_matches_known_digest_of_empty_string() {
        // Well-known test vector: SHA-256 of the empty byte string.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let hash = sha256_file(tmp.path()).await.unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn verify_sha256_succeeds_on_match_case_insensitively() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, b"hello world").unwrap();

        // Known SHA-256("hello world"), uppercased to also exercise the
        // case-insensitive comparison.
        let expected = "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";
        verify_sha256(tmp.path(), expected).await.unwrap();
    }

    #[tokio::test]
    async fn verify_sha256_fails_typed_on_mismatch() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, b"hello world").unwrap();

        let err = verify_sha256(tmp.path(), "0".repeat(64).as_str())
            .await
            .unwrap_err();
        assert!(matches!(err, VerifyError::Mismatch { .. }));
    }

    #[test]
    fn parses_manifest_with_plain_and_binary_marker_entries() {
        let manifest = "\
abc123  myapp-linux.tar.gz
def456 *myapp-macos.dmg
# a comment line, and a blank line below

789xyz  myapp-windows.exe
";
        assert_eq!(
            parse_checksum_manifest(manifest, "myapp-linux.tar.gz"),
            Some("abc123".to_string())
        );
        assert_eq!(
            parse_checksum_manifest(manifest, "myapp-macos.dmg"),
            Some("def456".to_string())
        );
        assert_eq!(
            parse_checksum_manifest(manifest, "myapp-windows.exe"),
            Some("789xyz".to_string())
        );
        assert_eq!(parse_checksum_manifest(manifest, "unknown-file.zip"), None);
    }

    #[test]
    fn parses_single_checksum_file_in_bare_digest_form() {
        let contents = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\n";
        assert_eq!(
            parse_single_checksum_file(contents, "myapp.dmg"),
            Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".to_string())
        );
    }

    #[test]
    fn parses_single_checksum_file_in_manifest_form() {
        let contents =
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9  myapp.dmg\n";
        assert_eq!(
            parse_single_checksum_file(contents, "myapp.dmg"),
            Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".to_string())
        );
    }

    #[test]
    fn garbage_checksum_file_returns_none_rather_than_a_wrong_digest() {
        assert_eq!(
            parse_single_checksum_file("not a checksum file at all", "myapp.dmg"),
            None
        );
    }
}
