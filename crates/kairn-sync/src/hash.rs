//! Content addressing (spec §5). BLAKE3 over the raw blob bytes; the server
//! treats files as opaque, so the same function hashes markdown, images, and
//! (later) ciphertext identically.

use crate::types::ContentHash;

/// Hash a blob's bytes to a lowercase-hex BLAKE3 digest.
pub fn hash_bytes(bytes: &[u8]) -> ContentHash {
    ContentHash(blake3::hash(bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_hash_identically() {
        assert_eq!(hash_bytes(b"# note\n"), hash_bytes(b"# note\n"));
    }

    #[test]
    fn different_bytes_differ() {
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"b"));
    }

    #[test]
    fn hash_is_lowercase_hex_of_expected_width() {
        let h = hash_bytes(b"anything").0;
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
