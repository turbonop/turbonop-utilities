//! `.eh_frame` as records, for moving functions' FDEs between objects.
//!
//! An object's `.eh_frame` is a sequence of CIEs and FDEs, each a 4-byte
//! length (or `0xFFFFFFFF` and an 8-byte one), then for a CIE the id 0 and
//! for an FDE the distance back from that field to its CIE. An FDE's
//! `pc_begin` is the field after it, which a relocation names (its
//! function's section, through the section's symbol); its augmentation
//! data may hold the LSDA's address (another relocation), and a CIE's the
//! personality routine's (another). Only the records' bounds, their kind,
//! the CIE an FDE names, and the relocations inside each are read: the
//! bytes are copied as they are, so the encodings needn't be.

/// One record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Where it starts in the section.
    pub start: usize,
    /// Its length, the length field included.
    pub len: usize,
    /// For an FDE, the start of its CIE (`None`: a CIE).
    pub cie: Option<usize>,
    /// The offset of the CIE pointer field (and so of `pc_begin`'s, which
    /// follows it) within the record.
    pub id_at: usize,
}

impl Record {
    /// Where `pc_begin` is, in the section.
    pub fn pc_begin(&self) -> usize {
        self.start + self.id_at + 4
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// The records of `data` (little- or big-endian), up to a terminator or
/// the end; `None` if one doesn't parse.
pub fn records(data: &[u8], big: bool) -> Option<Vec<Record>> {
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let len = u32_at(at)?;
        if len == 0 {
            // A terminator: whatever follows isn't read by linkers.
            break;
        }
        let (body, id_at) = if len == 0xFFFF_FFFF {
            let lo = u64::from(u32_at(at + 4)?);
            let hi = u64::from(u32_at(at + 8)?);
            let l = if big {
                (lo << 32) | hi
            } else {
                (hi << 32) | lo
            };
            (usize::try_from(l).ok()?, 12)
        } else {
            (len as usize, 4)
        };
        let end = at.checked_add(id_at)?.checked_add(body)?;
        if end > data.len() || body < 4 {
            return None;
        }
        let id = u32_at(at + id_at)?;
        let cie = if id == 0 {
            None
        } else {
            let field = at + id_at;
            Some(field.checked_sub(id as usize)?)
        };
        out.push(Record {
            start: at,
            len: end - at,
            cie,
            id_at,
        });
        at = end;
    }
    // Every FDE names a CIE among the records.
    for r in &out {
        if let Some(c) = r.cie
            && !out.iter().any(|x| x.start == c && x.cie.is_none())
        {
            return None;
        }
    }
    Some(out)
}

/// Builds a section of `pieces` (each a record's bytes, in order: CIEs
/// before the FDEs that name them), each FDE's CIE pointer rewritten to the
/// CIE given by its index in `pieces`. Returns the bytes and each piece's
/// new start.
pub fn build(pieces: &[(&[u8], Option<usize>, usize)], big: bool) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::new();
    let mut starts = Vec::with_capacity(pieces.len());
    for &(bytes, cie, id_at) in pieces {
        let start = out.len();
        starts.push(start);
        out.extend_from_slice(bytes);
        if let Some(c) = cie {
            let field = start + id_at;
            let distance = (field - starts[c]) as u32;
            let b = if big {
                distance.to_be_bytes()
            } else {
                distance.to_le_bytes()
            };
            out[field..field + 4].copy_from_slice(&b);
        }
    }
    (out, starts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cie() -> Vec<u8> {
        // length 12: id 0, version 1, "zR", align 1, -8, ra 16, aug 1, enc.
        let mut v = 12u32.to_le_bytes().to_vec();
        v.extend_from_slice(&[0, 0, 0, 0, 1, b'z', b'R', 0, 1, 0x78, 16, 1]);
        v
    }

    fn fde(back: u32) -> Vec<u8> {
        let mut v = 16u32.to_le_bytes().to_vec();
        v.extend_from_slice(&back.to_le_bytes());
        v.extend_from_slice(&[0; 12]);
        v
    }

    #[test]
    fn records_and_rebuild() {
        let mut data = cie();
        let f1 = data.len();
        data.extend(fde((f1 + 4) as u32));
        let f2 = data.len();
        data.extend(fde((f2 + 4) as u32));
        let r = records(&data, false).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].cie, None);
        assert_eq!(r[1].cie, Some(0));
        assert_eq!(r[2].cie, Some(0));
        assert_eq!(r[2].pc_begin(), f2 + 8);
        // Drop the first FDE: the second's CIE pointer shrinks.
        let pieces = [
            (&data[r[0].start..r[0].end()], None, 4),
            (&data[r[2].start..r[2].end()], Some(0), 4),
        ];
        let (out, starts) = build(&pieces, false);
        assert_eq!(starts, [0, 16]);
        let back = records(&out, false).unwrap();
        assert_eq!(back[1].cie, Some(0));
        assert_eq!(u32::from_le_bytes(out[20..24].try_into().unwrap()), 20);
        // A pointer to no CIE doesn't parse.
        let mut bad = data.clone();
        bad[f1 + 4] = 3;
        assert!(records(&bad, false).is_none());
    }
}
