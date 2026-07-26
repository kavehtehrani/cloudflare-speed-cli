//! Shared loading and parsing of user-supplied TLS certificates.
//!
//! `--certificate` provides a custom root CA. Several independent clients in
//! this crate (the main reqwest client, the IPv4/IPv6 comparison client, the
//! external-IP fetcher, and the rustls-based TLS handshake measurement) need
//! to honor it. This module centralizes file validation, reading, and parsing
//! so each call site is a one-liner.

use anyhow::{Context, Result};
use base64::Engine;
use rustls::pki_types::CertificateDer;
use std::path::Path;

pub const VALID_EXTENSIONS: &[&str] = &["pem", "crt", "cer", "der"];

/// Read certificate bytes from disk and report whether the file is DER-encoded
/// (`true`) or PEM-encoded (`false`), based on file extension. Errors if the
/// extension is missing or not in `VALID_EXTENSIONS`.
fn read_cert_bytes(path: &Path) -> Result<(Vec<u8>, bool)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    let ext = ext.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Certificate file has no extension. Expected one of: {}",
            VALID_EXTENSIONS.join(", ")
        )
    })?;

    if !VALID_EXTENSIONS.contains(&ext) {
        return Err(anyhow::anyhow!(
            "Invalid certificate file extension '{}'. Expected one of: {}",
            ext,
            VALID_EXTENSIONS.join(", ")
        ));
    }

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read certificate from {}", path.display()))?;

    Ok((bytes, ext == "der"))
}

/// Load a custom root CA as a `reqwest::Certificate` for use with
/// `ClientBuilder::add_root_certificate`.
pub fn load_reqwest_certificate(path: &Path) -> Result<reqwest::Certificate> {
    let (bytes, is_der) = read_cert_bytes(path)?;
    if is_der {
        reqwest::Certificate::from_der(&bytes)
            .with_context(|| format!("failed to parse DER certificate from {}", path.display()))
    } else {
        reqwest::Certificate::from_pem(&bytes)
            .with_context(|| format!("failed to parse PEM certificate from {}", path.display()))
    }
}

/// Load a custom root CA as one or more `CertificateDer` values for use with
/// `rustls::RootCertStore::add`. PEM bundles containing multiple certificates
/// are split into individual entries.
pub fn load_rustls_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let (bytes, is_der) = read_cert_bytes(path)?;
    if is_der {
        return Ok(vec![CertificateDer::from(bytes)]);
    }

    let pem = std::str::from_utf8(&bytes)
        .with_context(|| format!("PEM certificate at {} is not valid UTF-8", path.display()))?;

    let ders = parse_pem_certificates(pem)
        .with_context(|| format!("failed to parse PEM certificate from {}", path.display()))?;

    Ok(ders.into_iter().map(CertificateDer::from).collect())
}

/// Extract the DER bytes of every `CERTIFICATE` block in a PEM document.
/// `rustls` 0.23 / `rustls-pki-types` 1.x do not ship a built-in PEM parser
/// (that lives in the optional `rustls-pemfile` crate), so this is a small
/// hand-rolled scanner over the well-defined PEM format.
fn parse_pem_certificates(input: &str) -> Result<Vec<Vec<u8>>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let mut certs = Vec::new();
    let mut lines = input.lines();

    loop {
        let mut found_begin = false;
        for line in lines.by_ref() {
            if line.trim() == BEGIN {
                found_begin = true;
                break;
            }
        }
        if !found_begin {
            break;
        }

        let mut body = String::new();
        let mut found_end = false;
        for line in lines.by_ref() {
            let trimmed = line.trim();
            if trimmed == END {
                found_end = true;
                break;
            }
            body.push_str(trimmed);
        }
        if !found_end {
            return Err(anyhow::anyhow!("PEM block missing '{}' marker", END));
        }

        let der = base64::engine::general_purpose::STANDARD
            .decode(body.as_bytes())
            .context("PEM body is not valid base64")?;
        certs.push(der);
    }

    if certs.is_empty() {
        return Err(anyhow::anyhow!("no '{}' blocks found in PEM input", BEGIN));
    }

    Ok(certs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cf-speed-cli-cert-test-{}", name));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        path
    }

    // Synthetic PEM whose body is just valid base64 ("Hello, world!").
    // The PEM scanner only validates structure + base64 decoding, not the
    // DER content, so this is sufficient to exercise the parser. Real cert
    // bytes are exercised end-to-end by the integration paths in cloudflare.rs.
    const TEST_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
SGVsbG8sIHdvcmxkIQ==\n\
-----END CERTIFICATE-----\n";

    #[test]
    fn rejects_missing_extension() {
        let path = write_temp("noext", b"x");
        let err = read_cert_bytes(&path).unwrap_err();
        assert!(err.to_string().contains("no extension"));
    }

    #[test]
    fn rejects_unknown_extension() {
        let path = write_temp("bogus.txt", b"x");
        let err = read_cert_bytes(&path).unwrap_err();
        assert!(err.to_string().contains("Invalid"));
    }

    #[test]
    fn parses_pem_block_count() {
        // Concatenate two copies of TEST_PEM and confirm the parser yields two DER blobs.
        let two = format!("{}{}", TEST_PEM, TEST_PEM);
        let blocks = parse_pem_certificates(&two).unwrap();
        assert_eq!(blocks.len(), 2);
        // Both blobs should decode to the same bytes.
        assert_eq!(blocks[0], blocks[1]);
        assert!(!blocks[0].is_empty());
    }

    #[test]
    fn rejects_pem_without_end_marker() {
        let truncated = "-----BEGIN CERTIFICATE-----\nAAAA\n";
        let err = parse_pem_certificates(truncated).unwrap_err();
        assert!(err.to_string().contains("END CERTIFICATE"));
    }

    #[test]
    fn rejects_pem_without_any_block() {
        let err = parse_pem_certificates("no markers here\n").unwrap_err();
        assert!(err.to_string().contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn load_rustls_certificates_pem_round_trip() {
        let path = write_temp("good.pem", TEST_PEM.as_bytes());
        let certs = load_rustls_certificates(&path).unwrap();
        assert_eq!(certs.len(), 1);
        assert!(!certs[0].as_ref().is_empty());
    }
}
