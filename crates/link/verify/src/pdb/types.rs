//! Structural hashes of a PDB's type (TPI) and id (IPI) records: each
//! record's kind and bytes, with every index field replaced by the hash of
//! the record it names (or, for a built-in type, its index). Two records
//! hash alike exactly when they are the same type, whatever their
//! indices.
//!
//! Records name earlier records, except through forward references that a
//! later definition resolves by name, so the hashes are computed in
//! levels: a record's level is one more than the highest of the records it
//! names, and each level's records are hashed in parallel. A record that
//! names itself or a later record (none should) is hashed with that index
//! as it is, and counted.

use turbonop_codeview::type_refs;
use turbonop_codeview::types::TypeRecord;
use turbonop_pdb::TypeStream;

use crate::hash::H128;

/// `LF_UDT_MOD_SRC_LINE`: a UDT's source file, line, and module.
pub(crate) const LF_UDT_MOD_SRC_LINE: u16 = 0x1607;

/// The first index of a stream's records (lower ones are built-in types).
pub(crate) const FIRST: u32 = 0x1000;

/// A stream's records' hashes, by index from [`FIRST`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Hashes {
    pub hashes: Vec<u128>,
    /// Records whose index fields couldn't be found (an unknown kind).
    pub unknown: Vec<u16>,
    /// Records that name themselves or a later record.
    pub forward: u64,
}

impl Hashes {
    /// The hash of index `ti`: a built-in type's index, or a record's
    /// hash (an index past the end hashes as itself, marked).
    pub fn of(&self, ti: u32) -> u128 {
        if ti < FIRST {
            return u128::from(ti);
        }
        self.hashes
            .get((ti - FIRST) as usize)
            .copied()
            .unwrap_or(u128::from(ti) | (1 << 127))
    }
}

/// The records of `stream`, in index order.
pub(crate) fn records<'s>(stream: &'s TypeStream<'_>) -> Vec<TypeRecord<'s>> {
    stream.iter().map(|(_, r)| r).collect()
}

/// Hashes `records`, a stream whose index fields name records of its own
/// (or, marked as ids, of the stream `ids_of`'s hashes are for: an IPI
/// record's types are TPI records). For the TPI, `other` is `None`; for
/// the IPI, it is the TPI's hashes.
///
/// `names` is the PDB's string table: an IPI's `LF_UDT_MOD_SRC_LINE`
/// names its file by an offset in it (an object's `LF_UDT_SRC_LINE` names
/// an `LF_STRING_ID` instead).
pub(crate) fn hash(
    records: &[TypeRecord<'_>],
    other: Option<(&Hashes, &[TypeRecord<'_>])>,
    names: Option<&turbonop_pdb::StringTable<'_>>,
) -> Hashes {
    let other_hashes = other.map(|o| o.0);
    // Each record's index fields, in parallel.
    let refs: Vec<Result<Vec<type_refs::Ref>, u16>> = turbonop_par::map(records, 4096, |r| {
        let mut v = Vec::new();
        type_refs::refs(r.kind, r.data, &mut v)
            .map(|()| v)
            .map_err(|_| r.kind)
    });
    // Whether a field names this stream's records.
    let own = |r: &type_refs::Ref| match other {
        None => true,
        Some(_) => r.id,
    };
    let value = |data: &[u8], r: &type_refs::Ref| -> u32 {
        let o = r.offset as usize;
        data.get(o..o + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // Levels.
    let n = records.len();
    let mut level = vec![0u32; n];
    let mut forward = 0u64;
    for i in 0..n {
        let Ok(list) = &refs[i] else { continue };
        let mut l = 0;
        for r in list.iter().filter(|r| own(r)) {
            let v = value(records[i].data, r);
            if v < FIRST {
                continue;
            }
            let k = (v - FIRST) as usize;
            if k < i {
                l = l.max(level[k] + 1);
            } else {
                forward += 1;
            }
        }
        level[i] = l;
    }
    let top = level.iter().copied().max().unwrap_or(0) as usize;
    let mut by_level: Vec<Vec<u32>> = vec![Vec::new(); top + 1];
    for (i, &l) in level.iter().enumerate() {
        by_level[l as usize].push(i as u32);
    }
    let mut out = Hashes {
        hashes: vec![0; n],
        unknown: refs
            .iter()
            .filter_map(|r| r.as_ref().err().copied())
            .collect(),
        forward,
    };
    for indices in &by_level {
        let done = &out;
        let hashed = turbonop_par::map(indices, 2048, |&i| {
            let i = i as usize;
            let r = &records[i];
            let mut h = H128::new();
            h.u64(u64::from(r.kind));
            match &refs[i] {
                Err(_) => h.bytes(r.data),
                Ok(list) => {
                    let mut from = 0;
                    for f in list {
                        let o = f.offset as usize;
                        if o < from || o + 4 > r.data.len() {
                            continue;
                        }
                        h.bytes(&r.data[from..o]);
                        let v = value(r.data, f);
                        if own(f) {
                            if v >= FIRST && (v - FIRST) as usize >= i {
                                h.u64(u64::from(v) | (1 << 63));
                            } else {
                                h.u128(done.of(v));
                            }
                        } else if r.kind == LF_UDT_MOD_SRC_LINE {
                            // The UDT by its name: link.exe gives the line
                            // to one of the records of that name, and which
                            // one differs between links.
                            let name = other
                                .and_then(|o| o.1.get(v.checked_sub(FIRST)? as usize))
                                .and_then(TypeRecord::name);
                            h.bytes(name.unwrap_or(b"\xFF"));
                        } else {
                            h.u128(other_hashes.map_or(u128::from(v), |t| t.of(v)));
                        }
                        from = o + 4;
                    }
                    // LF_UDT_MOD_SRC_LINE: after the type, the file is a
                    // string-table offset, which stands for its string;
                    // then the line; then the number of the module the
                    // linker saw the type in first, an index into the
                    // module list, whose order differs between links (and
                    // which folding may change), so it is left out.
                    if r.kind == LF_UDT_MOD_SRC_LINE && from == 4 && r.data.len() >= 14 {
                        let file = u32::from_le_bytes([r.data[4], r.data[5], r.data[6], r.data[7]]);
                        h.bytes(names.and_then(|n| n.get(file)).unwrap_or(b"\xFF"));
                        h.bytes(&r.data[8..12]);
                        h.bytes(&r.data[14..]);
                    } else {
                        h.bytes(&r.data[from..]);
                    }
                }
            }
            h.finish()
        });
        for (&i, h) in indices.iter().zip(hashed) {
            out.hashes[i as usize] = h;
        }
    }
    out
}
