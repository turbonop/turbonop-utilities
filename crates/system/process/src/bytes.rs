//! Reading the structures platforms return, from their bytes: in the
//! host's byte order (the structures are the running system's), checked
//! against the buffer's end.

/// `N` bytes at `at`, if the buffer has them.
pub(crate) fn array<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    array(b, at).map(u16::from_ne_bytes)
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    array(b, at).map(u32::from_ne_bytes)
}

pub(crate) fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    array(b, at).map(i32::from_ne_bytes)
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    array(b, at).map(u64::from_ne_bytes)
}

pub(crate) fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    array(b, at).map(i64::from_ne_bytes)
}

/// A NUL-terminated string in a fixed-size field: its bytes up to the
/// first NUL (or all of them), as UTF-8 (lossily).
pub(crate) fn c_str(field: &[u8]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// UTF-16 code units (in the host's order) as a string, lossily.
pub(crate) fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_ne_bytes(c))
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_within_the_buffer() {
        let b = [1u8, 0, 0, 0, 2, 0, 0, 0];
        assert_eq!(u32_at(&b, 0), Some(u32::from_ne_bytes([1, 0, 0, 0])));
        assert_eq!(u32_at(&b, 4), Some(u32::from_ne_bytes([2, 0, 0, 0])));
        assert_eq!(u32_at(&b, 5), None);
        assert_eq!(u64_at(&b, 0), Some(u64::from_ne_bytes(b)));
        assert_eq!(u64_at(&b, 1), None);
        assert_eq!(u16_at(&b, usize::MAX), None);
        assert_eq!(i64_at(&b, 0), Some(i64::from_ne_bytes(b)));
        assert_eq!(i32_at(&b, 8), None);
    }

    #[test]
    fn strings() {
        assert_eq!(c_str(b"abc\0def"), "abc");
        assert_eq!(c_str(b"abcdef"), "abcdef");
        assert_eq!(c_str(b"\0"), "");
        let w: Vec<u8> = "héllo".encode_utf16().flat_map(u16::to_ne_bytes).collect();
        assert_eq!(utf16(&w), "héllo");
        // (An odd byte at the end is ignored.)
        let mut odd = w.clone();
        odd.push(0x41);
        assert_eq!(utf16(&odd), "héllo");
    }
}
