//! Offset conversion at the boundary. `kairn-core` addresses text in UTF-8 byte
//! offsets; TextKit / `NSTextStorage` address it in UTF-16 code units. Rather
//! than have the editor reinvent (and mis-handle) the mapping for every tap and
//! selection, the bridge owns one tested pair. Both floor to a character
//! boundary and clamp past the end, mirroring `kairn-core`'s own clamping so a
//! stray offset is never a crash.

/// Convert a UTF-16 code-unit offset (TextKit) to a UTF-8 byte offset
/// (kairn-core). An offset landing inside a surrogate pair floors to that
/// character's start; an offset past the end clamps to the text length.
#[uniffi::export]
pub fn utf16_to_byte(text: String, utf16_offset: u64) -> u64 {
    let target = utf16_offset as usize;
    let mut u = 0usize;
    let mut b = 0usize;
    for ch in text.chars() {
        if u >= target {
            break;
        }
        if u + ch.len_utf16() > target {
            break; // inside a surrogate pair: floor to this char's start
        }
        u += ch.len_utf16();
        b += ch.len_utf8();
    }
    b as u64
}

/// Convert a UTF-8 byte offset (kairn-core) to a UTF-16 code-unit offset
/// (TextKit). An offset landing inside a multi-byte character floors to that
/// character's start; an offset past the end clamps to the text length.
#[uniffi::export]
pub fn byte_to_utf16(text: String, byte_offset: u64) -> u64 {
    let target = byte_offset as usize;
    let mut u = 0usize;
    for (idx, ch) in text.char_indices() {
        if idx + ch.len_utf8() > target {
            break;
        }
        u += ch.len_utf16();
    }
    u as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    // "a😀b": 'a' 1 byte/1 unit, '😀' 4 bytes/2 units, 'b' 1 byte/1 unit.
    const S: &str = "a😀b";

    #[test]
    fn round_trips_at_boundaries() {
        for (bytes, units) in [(0u64, 0u64), (1, 1), (5, 3), (6, 4)] {
            assert_eq!(byte_to_utf16(S.into(), bytes), units);
            assert_eq!(utf16_to_byte(S.into(), units), bytes);
        }
    }

    #[test]
    fn utf16_inside_surrogate_pair_floors() {
        // Unit offset 2 is between the emoji's two code units -> its byte start.
        assert_eq!(utf16_to_byte(S.into(), 2), 1);
    }

    #[test]
    fn byte_inside_multibyte_char_floors() {
        // Byte offset 3 is inside the emoji -> its UTF-16 start.
        assert_eq!(byte_to_utf16(S.into(), 3), 1);
    }

    #[test]
    fn past_end_clamps() {
        assert_eq!(utf16_to_byte(S.into(), 99), 6);
        assert_eq!(byte_to_utf16(S.into(), 99), 4);
    }
}
