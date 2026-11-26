//! The link-edit data an object's commands locate, decoded: data-in-code
//! entries (`LC_DATA_IN_CODE`) and linker optimization hints
//! (`LC_LINKER_OPTIMIZATION_HINT`). In an object both hold addresses (a
//! section's address plus an offset in it), as LLVM writes them.

use alloc::vec::Vec;

use crate::Endian;

/// A data-in-code entry (`data_in_code_entry`): a run of data in code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataInCode {
    /// Its start: an address in an object (a file offset in an image).
    pub offset: u32,
    /// Its length in bytes.
    pub length: u16,
    /// `DICE_KIND_DATA` (1), `DICE_KIND_JUMP_TABLE8` (2),
    /// `DICE_KIND_JUMP_TABLE16` (3), `DICE_KIND_JUMP_TABLE32` (4),
    /// `DICE_KIND_ABS_JUMP_TABLE32` (5).
    pub kind: u16,
}

/// The entries of `LC_DATA_IN_CODE`'s data (`None` if it isn't a whole
/// number of entries).
pub fn decode_data_in_code(data: &[u8], endian: Endian) -> Option<Vec<DataInCode>> {
    if !data.len().is_multiple_of(8) {
        return None;
    }
    Some(
        data.as_chunks::<8>()
            .0
            .iter()
            .map(|e| {
                let u32_at = |b: [u8; 4]| match endian {
                    Endian::Little => u32::from_le_bytes(b),
                    Endian::Big => u32::from_be_bytes(b),
                };
                let u16_at = |b: [u8; 2]| match endian {
                    Endian::Little => u16::from_le_bytes(b),
                    Endian::Big => u16::from_be_bytes(b),
                };
                DataInCode {
                    offset: u32_at([e[0], e[1], e[2], e[3]]),
                    length: u16_at([e[4], e[5]]),
                    kind: u16_at([e[6], e[7]]),
                }
            })
            .collect(),
    )
}

/// `LC_DATA_IN_CODE`'s data for `entries`.
pub fn encode_data_in_code(entries: &[DataInCode], endian: Endian) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * 8);
    for e in entries {
        match endian {
            Endian::Little => {
                out.extend_from_slice(&e.offset.to_le_bytes());
                out.extend_from_slice(&e.length.to_le_bytes());
                out.extend_from_slice(&e.kind.to_le_bytes());
            }
            Endian::Big => {
                out.extend_from_slice(&e.offset.to_be_bytes());
                out.extend_from_slice(&e.length.to_be_bytes());
                out.extend_from_slice(&e.kind.to_be_bytes());
            }
        }
    }
    out
}

/// A linker optimization hint: its kind (`LOH_ARM64_ADRP_ADRP` (1) to
/// `LOH_ARM64_ADRP_LDR_GOT` (8)) and the addresses of the instructions it
/// names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub kind: u64,
    pub args: Vec<u64>,
}

fn uleb(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *data.get(*at)?;
        *at += 1;
        if shift >= 64 {
            return None;
        }
        v |= u64::from(b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
}

fn put_uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// The hints of `LC_LINKER_OPTIMIZATION_HINT`'s data: each its kind, its
/// count, and its addresses, as ULEB128s, then zeros to the pointer size
/// (`None` if it doesn't decode).
pub fn decode_hints(data: &[u8]) -> Option<Vec<Hint>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        if data[at..].iter().all(|&b| b == 0) {
            break;
        }
        let kind = uleb(data, &mut at)?;
        let n = uleb(data, &mut at)?;
        if n > (data.len() - at) as u64 {
            return None;
        }
        let mut args = Vec::with_capacity(n as usize);
        for _ in 0..n {
            args.push(uleb(data, &mut at)?);
        }
        out.push(Hint { kind, args });
    }
    Some(out)
}

/// `LC_LINKER_OPTIMIZATION_HINT`'s data for `hints`, padded with zeros to
/// 8 bytes (`wide`) or 4, as LLVM pads it.
pub fn encode_hints(hints: &[Hint], wide: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for h in hints {
        put_uleb(&mut out, h.kind);
        put_uleb(&mut out, h.args.len() as u64);
        for &a in &h.args {
            put_uleb(&mut out, a);
        }
    }
    let align = if wide { 8 } else { 4 };
    out.resize(out.len().div_ceil(align) * align, 0);
    out
}
