//! File checksums: MD5, SHA-1 and SHA-256 computed in one streaming pass.

use anyhow::{Context as _, Result};
use md5::{Digest as _, Md5};
use sha1::Sha1;
use sha2::Sha256;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHashes {
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
}

impl FileHashes {
    /// Which of the three hashes matches `expected` (case-insensitive), if any.
    pub fn matches(&self, expected: &str) -> Option<&'static str> {
        let expected = expected.trim().to_ascii_lowercase();
        if expected == self.md5 {
            Some("md5")
        } else if expected == self.sha1 {
            Some("sha1")
        } else if expected == self.sha256 {
            Some("sha256")
        } else {
            None
        }
    }
}

/// Hash a file streaming in 1 MB chunks; all three digests in a single read.
pub fn hash_file(path: &Path) -> Result<FileHashes> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        md5.update(&buf[..n]);
        sha1.update(&buf[..n]);
        sha256.update(&buf[..n]);
    }
    Ok(FileHashes {
        md5: hex(&md5.finalize()),
        sha1: hex(&sha1.finalize()),
        sha256: hex(&sha256.finalize()),
    })
}

pub fn hash_bytes(bytes: &[u8]) -> FileHashes {
    FileHashes {
        md5: hex(&Md5::digest(bytes)),
        sha1: hex(&Sha1::digest(bytes)),
        sha256: hex(&Sha256::digest(bytes)),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors_for_abc() {
        let h = hash_bytes(b"abc");
        assert_eq!(h.md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(h.sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            h.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn known_vectors_for_empty() {
        let h = hash_bytes(b"");
        assert_eq!(h.md5, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(h.sha1, "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            h.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn file_matches_bytes_and_expected_lookup() {
        let dir = std::env::temp_dir();
        let path = dir.join("konvrt-hash-test.bin");
        std::fs::write(&path, b"hello konvrt").unwrap();
        let from_file = hash_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let from_bytes = hash_bytes(b"hello konvrt");
        assert_eq!(from_file, from_bytes);
        assert_eq!(
            from_file.matches(&from_file.sha256.to_uppercase()),
            Some("sha256")
        );
        assert_eq!(from_file.matches(&from_file.md5), Some("md5"));
        assert_eq!(from_file.matches("deadbeef"), None);
    }
}
