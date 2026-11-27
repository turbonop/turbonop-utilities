//! The globals and publics: the symbol record stream (publics first, then
//! globals), each set's hash table (4096 buckets by `hashStringV1` of the
//! name, each bucket sorted as the reference implementation's search
//! expects: shorter names first, then case-insensitively), and the
//! publics' address map. After LLVM's `GSIStreamBuilder` (Apache-2.0 WITH
//! LLVM-exception), which follows the reference implementation's `gsi.cpp`.

use turbonop_codeview::SymbolRecord;
use turbonop_pdb::hash::hash_v1;

const BUCKETS: usize = 4096;

/// A public symbol (`S_PUB32`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Public {
    /// Its (decorated) name.
    pub name: Vec<u8>,
    /// `CV_PUBSYMFLAGS`: 1 code, 2 function.
    pub flags: u32,
    /// Its section (1-based).
    pub segment: u16,
    /// Its offset in the section.
    pub offset: u32,
}

/// `S_PUB32`'s record for `p`, 4-byte aligned.
fn public_record(p: &Public) -> Vec<u8> {
    let len = (2 + 2 + 4 + 4 + 2 + p.name.len() + 1).next_multiple_of(4);
    let mut r = Vec::with_capacity(len);
    r.extend_from_slice(
        &u16::try_from(len - 2)
            .expect("a name under 64 KB")
            .to_le_bytes(),
    );
    r.extend_from_slice(&0x110Eu16.to_le_bytes());
    r.extend_from_slice(&p.flags.to_le_bytes());
    r.extend_from_slice(&p.offset.to_le_bytes());
    r.extend_from_slice(&p.segment.to_le_bytes());
    r.extend_from_slice(&p.name);
    r.resize(len, 0);
    r
}

/// `caseInsensitiveComparePchPchCchCch`: by length, then (for ASCII)
/// case-insensitively, else by bytes.
fn gsi_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| {
        if a.is_ascii() && b.is_ascii() {
            let low = |s: &[u8]| -> Vec<u8> { s.to_ascii_lowercase() };
            low(a).cmp(&low(b))
        } else {
            a.cmp(b)
        }
    })
}

/// A hash table over `names` (the records' names), whose records start at
/// `offsets` in the symbol record stream.
fn hash_table(names: &[&[u8]], offsets: &[u32]) -> Vec<u8> {
    let buckets: Vec<u32> = turbonop_par::map(names, 4096, |n| hash_v1(n) % crate::n32(BUCKETS));
    let mut starts = vec![0u32; BUCKETS + 1];
    for &b in &buckets {
        starts[b as usize + 1] += 1;
    }
    for i in 0..BUCKETS {
        starts[i + 1] += starts[i];
    }
    let mut cursor = starts.clone();
    let mut slots = vec![0u32; names.len()];
    for (i, &b) in buckets.iter().enumerate() {
        slots[cursor[b as usize] as usize] = u32::try_from(i).expect("fewer than 4 billion");
        cursor[b as usize] += 1;
    }
    // Each bucket sorted (and two of one name by their offsets).
    let mut ranges: Vec<&mut [u32]> = Vec::with_capacity(BUCKETS);
    let mut rest: &mut [u32] = &mut slots;
    for b in 0..BUCKETS {
        let (mine, tail) = rest.split_at_mut((starts[b + 1] - starts[b]) as usize);
        ranges.push(mine);
        rest = tail;
    }
    turbonop_par::for_each_chunk_mut(&mut ranges, 64, |part| {
        for r in part.iter_mut() {
            r.sort_by(|&x, &y| {
                gsi_cmp(names[x as usize], names[y as usize])
                    .then(offsets[x as usize].cmp(&offsets[y as usize]))
            });
        }
    });
    drop(ranges);
    let mut out = Vec::with_capacity(16 + names.len() * 8 + 600);
    let words = (BUCKETS + 32) / 32;
    let present: Vec<usize> = (0..BUCKETS)
        .filter(|&b| starts[b + 1] > starts[b])
        .collect();
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&(0xEFFE_0000u32 + 19_990_810).to_le_bytes());
    out.extend_from_slice(&u32::try_from(names.len() * 8).expect("small").to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(words * 4 + present.len() * 4)
            .expect("small")
            .to_le_bytes(),
    );
    for &s in &slots {
        // The record's offset plus one, and a reference count of one.
        out.extend_from_slice(&(offsets[s as usize] + 1).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
    }
    let mut bitmap = vec![0u32; words];
    for &b in &present {
        bitmap[b / 32] |= 1 << (b % 32);
    }
    for w in bitmap {
        out.extend_from_slice(&w.to_le_bytes());
    }
    for &b in &present {
        // As if each record were 12 bytes (`HROffsetCalc`).
        out.extend_from_slice(&(starts[b] * 12).to_le_bytes());
    }
    out
}

/// The name of a whole symbol record (length and kind first).
fn record_name(record: &[u8]) -> &[u8] {
    if record.len() < 4 {
        return b"";
    }
    let kind = u16::from_le_bytes([record[2], record[3]]);
    let r = SymbolRecord {
        offset: 0,
        kind,
        data: &record[4..],
    };
    r.name().unwrap_or_default()
}

/// The symbol record stream, the globals stream, and the publics stream.
pub struct Built {
    /// The records: publics, then globals.
    pub records: Vec<u8>,
    /// The globals' hash table.
    pub globals: Vec<u8>,
    /// The publics' header, hash table, and address map.
    pub publics: Vec<u8>,
}

/// Builds the streams for `publics` and `globals` (whole records, each
/// 4-byte aligned; duplicate `S_UDT` and `S_CONSTANT` records are dropped
/// here, the first kept).
pub fn build(mut publics: Vec<Public>, globals: Vec<Vec<u8>>) -> Built {
    // Publics by name (each name's records in a stable order).
    turbonop_par::sort_by(&mut publics, |a, b| {
        a.name
            .cmp(&b.name)
            .then(a.segment.cmp(&b.segment))
            .then(a.offset.cmp(&b.offset))
    });
    let pub_records: Vec<Vec<u8>> = turbonop_par::map(&publics, 4096, public_record);
    let mut records = Vec::new();
    let mut pub_offsets = Vec::with_capacity(publics.len());
    for r in &pub_records {
        pub_offsets.push(u32::try_from(records.len()).expect("under 4 GB"));
        records.extend_from_slice(r);
    }
    let mut seen = turbonop_hash::Set::default();
    let mut glob_offsets = Vec::with_capacity(globals.len());
    let mut kept: Vec<&[u8]> = Vec::with_capacity(globals.len());
    for g in &globals {
        let kind = u16::from_le_bytes([g[2], g[3]]);
        // Typedefs, constants, and global data (a selectany datum has a
        // record in every object) once each.
        if matches!(kind, 0x1108 | 0x1107 | 0x110D | 0x1113) && !seen.insert(g.as_slice()) {
            continue;
        }
        glob_offsets.push(u32::try_from(records.len()).expect("under 4 GB"));
        records.extend_from_slice(g);
        kept.push(g);
    }
    let glob_names: Vec<&[u8]> = kept.iter().map(|r| record_name(r)).collect();
    let pub_names: Vec<&[u8]> = publics.iter().map(|p| p.name.as_slice()).collect();
    let (globals_table, pub_table) = turbonop_par::join(
        || hash_table(&glob_names, &glob_offsets),
        || hash_table(&pub_names, &pub_offsets),
    );
    // The address map: the publics by (section, offset, name).
    let mut by_address: Vec<u32> = (0..u32::try_from(publics.len()).expect("small")).collect();
    turbonop_par::sort_by(&mut by_address, |&a, &b| {
        let (a, b) = (&publics[a as usize], &publics[b as usize]);
        (a.segment, a.offset, &a.name).cmp(&(b.segment, b.offset, &b.name))
    });
    let mut out = Vec::with_capacity(28 + pub_table.len() + 4 * publics.len());
    out.extend_from_slice(&u32::try_from(pub_table.len()).expect("small").to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(publics.len() * 4)
            .expect("small")
            .to_le_bytes(),
    );
    out.extend_from_slice(&[0u8; 20]); // thunks (incremental linking): none
    out.extend_from_slice(&pub_table);
    for i in by_address {
        out.extend_from_slice(&pub_offsets[i as usize].to_le_bytes());
    }
    Built {
        records,
        globals: globals_table,
        publics: out,
    }
}
