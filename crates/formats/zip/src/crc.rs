//! CRC-32 (ISO 3309, the polynomial 0xEDB88320 reflected): ZIP's, zlib's,
//! PNG's. Slicing by eight: eight table lookups per eight bytes.

const POLY: u32 = 0xEDB8_8320;

#[allow(clippy::cast_possible_truncation)]
const fn tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 8 {
            let prev = t[s - 1][i];
            t[s][i] = (prev >> 8) ^ t[0][(prev & 0xFF) as usize];
            s += 1;
        }
        i += 1;
    }
    t
}

static TABLES: [[u32; 256]; 8] = tables();

/// `crc` (a running CRC-32, 0 to start) extended over `data`.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let t = &TABLES;
    let mut c = !crc;
    let (chunks, rest) = data.as_chunks::<8>();
    for b in chunks {
        let lo = c ^ u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let hi = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        c = t[7][(lo & 0xFF) as usize]
            ^ t[6][((lo >> 8) & 0xFF) as usize]
            ^ t[5][((lo >> 16) & 0xFF) as usize]
            ^ t[4][(lo >> 24) as usize]
            ^ t[3][(hi & 0xFF) as usize]
            ^ t[2][((hi >> 8) & 0xFF) as usize]
            ^ t[1][((hi >> 16) & 0xFF) as usize]
            ^ t[0][(hi >> 24) as usize];
    }
    for &b in rest {
        c = (c >> 8) ^ t[0][((c ^ u32::from(b)) & 0xFF) as usize];
    }
    !c
}

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn sliced_matches_bytewise() {
        fn slow(data: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in data {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + i / 3) as u8).collect();
        for n in [0, 1, 7, 8, 9, 15, 16, 17, 999, 1000] {
            assert_eq!(crc32(&data[..n]), slow(&data[..n]), "{n}");
        }
        // Incrementally, split anywhere.
        let whole = crc32(&data);
        for at in [0, 3, 8, 500, 1000] {
            assert_eq!(crc32_update(crc32(&data[..at]), &data[at..]), whole);
        }
    }
}
