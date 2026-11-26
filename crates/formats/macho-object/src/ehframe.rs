//! The pointers in `__TEXT,__eh_frame`'s records (CIEs and FDEs), which an
//! object holds as their values (LLVM writes an FDE's function and LSDA
//! pointers as differences it resolves itself, PC-relative, with no
//! relocation: they move as the sections do; a CIE's personality pointer
//! has a relocation, to a GOT entry).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::Endian;
use crate::reloc::value_at;

/// What a pointer in a record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// A CIE's personality routine (`P`).
    Personality,
    /// An FDE's function: its start (`pc_begin`).
    PcBegin,
    /// An FDE's language-specific data area (`L`).
    Lsda,
}

/// A pointer in a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointer {
    pub kind: PointerKind,
    /// Its offset in the section.
    pub offset: usize,
    /// Its encoding (`DW_EH_PE_*`): the format (low 4 bits), how it is
    /// applied (`DW_EH_PE_pcrel`, 0x10, and the like), and
    /// `DW_EH_PE_indirect` (0x80).
    pub encoding: u8,
    /// Its size in bytes.
    pub size: u8,
    /// The offsets of the record it is in, and of the record's end.
    pub record: (usize, usize),
}

/// `DW_EH_PE_pcrel`: relative to the pointer's own address.
pub const DW_EH_PE_PCREL: u8 = 0x10;
/// `DW_EH_PE_omit`: no pointer.
pub const DW_EH_PE_OMIT: u8 = 0xFF;

/// The size of a pointer of `encoding` (`None` for the variable-length
/// ones).
fn size_of(encoding: u8, wide: bool) -> Option<u8> {
    match encoding & 0x0F {
        0x0 => Some(if wide { 8 } else { 4 }),
        0x2 | 0xA => Some(2),
        0x3 | 0xB => Some(4),
        0x4 | 0xC => Some(8),
        _ => None,
    }
}

fn uleb(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *data.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= u64::from(b & 0x7F) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
}

/// The pointers in the records of an `__eh_frame` section's bytes (`None`
/// if they don't decode: a 64-bit length, an augmentation it doesn't
/// know, a variable-length pointer encoding).
pub fn pointers(data: &[u8], wide: bool, endian: Endian) -> Option<Vec<Pointer>> {
    let u32_at = |at: usize| value_at(data, u32::try_from(at).ok()?, 2, endian);
    let mut out = Vec::new();
    // Each CIE's FDE pointer encoding, LSDA encoding, and whether it has
    // augmentation data.
    let mut cies: BTreeMap<usize, (u8, u8, bool)> = BTreeMap::new();
    let mut at = 0usize;
    while at + 4 <= data.len() {
        let length = u32_at(at)? as usize;
        if length == 0 {
            at += 4;
            continue;
        }
        if length == 0xFFFF_FFFF {
            return None;
        }
        let start = at + 4;
        let end = start.checked_add(length)?;
        if end > data.len() || length < 4 {
            return None;
        }
        let record = (at, end);
        let id = u32_at(start)? as usize;
        let mut p = start + 4;
        if id == 0 {
            let version = *data.get(p)?;
            p += 1;
            let aug_end = p + data.get(p..end)?.iter().position(|&b| b == 0)?;
            let aug = &data[p..aug_end];
            p = aug_end + 1;
            uleb(data, &mut p)?; // Code alignment.
            uleb(data, &mut p)?; // Data alignment (signed: only skipped).
            if version == 1 {
                p += 1;
            } else {
                uleb(data, &mut p)?;
            }
            let (mut fde, mut lsda, z) = (0u8, DW_EH_PE_OMIT, aug.first() == Some(&b'z'));
            if z {
                uleb(data, &mut p)?;
                for &c in &aug[1..] {
                    match c {
                        b'P' => {
                            let encoding = *data.get(p)?;
                            p += 1;
                            let size = size_of(encoding, wide)?;
                            out.push(Pointer {
                                kind: PointerKind::Personality,
                                offset: p,
                                encoding,
                                size,
                                record,
                            });
                            p += usize::from(size);
                        }
                        b'L' => {
                            lsda = *data.get(p)?;
                            p += 1;
                        }
                        b'R' => {
                            fde = *data.get(p)?;
                            p += 1;
                        }
                        b'S' | b'B' | b'G' => {}
                        _ => return None,
                    }
                }
            } else if !aug.is_empty() {
                return None;
            }
            cies.insert(at, (fde, lsda, z));
        } else {
            let cie = start.checked_sub(id)?;
            let &(fde, lsda, z) = cies.get(&cie)?;
            let size = size_of(fde, wide)?;
            out.push(Pointer {
                kind: PointerKind::PcBegin,
                offset: p,
                encoding: fde,
                size,
                record,
            });
            p += 2 * usize::from(size);
            if z {
                uleb(data, &mut p)?;
                if lsda != DW_EH_PE_OMIT {
                    let size = size_of(lsda, wide)?;
                    out.push(Pointer {
                        kind: PointerKind::Lsda,
                        offset: p,
                        encoding: lsda,
                        size,
                        record,
                    });
                }
            }
        }
        at = end;
    }
    Some(out)
}

/// Each FDE's CIE pointer in an `__eh_frame` section's bytes: the
/// offset of the field (which holds its distance back to the CIE) and of
/// the CIE (`None` if the records don't decode).
pub fn cie_links(data: &[u8], endian: Endian) -> Option<Vec<(usize, usize)>> {
    let u32_at = |at: usize| value_at(data, u32::try_from(at).ok()?, 2, endian);
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= data.len() {
        let length = u32_at(at)? as usize;
        if length == 0 {
            at += 4;
            continue;
        }
        if length == 0xFFFF_FFFF || length < 4 {
            return None;
        }
        let start = at + 4;
        let end = start.checked_add(length)?;
        if end > data.len() {
            return None;
        }
        let id = u32_at(start)? as usize;
        if id != 0 {
            out.push((start, start.checked_sub(id)?));
        }
        at = end;
    }
    Some(out)
}
