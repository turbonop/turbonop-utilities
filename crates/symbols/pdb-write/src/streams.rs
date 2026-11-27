//! The small streams and tables: the `/names` string table (and the DBI's
//! EC names, which have its form), the PDB info stream with its named
//! stream map, and the TPI/IPI streams with their hash streams.
//!
//! Hash table sizes follow the reference implementation's (as LLVM's
//! writer does), so that a reader probing by hash finds every entry.

use turbonop_pdb::hash::{hash_record, hash_v1};

/// A string table (`/names`): strings by offset, the first (offset 0)
/// empty.
#[derive(Debug, Clone)]
pub struct StringTable {
    buffer: Vec<u8>,
    offsets: turbonop_hash::Map<Vec<u8>, u32>,
    order: Vec<u32>,
}

impl Default for StringTable {
    fn default() -> Self {
        StringTable {
            buffer: vec![0],
            offsets: turbonop_hash::Map::default(),
            order: Vec::new(),
        }
    }
}

impl StringTable {
    /// `s`'s offset (added if new).
    pub fn insert(&mut self, s: &[u8]) -> u32 {
        if s.is_empty() {
            return 0;
        }
        if let Some(&o) = self.offsets.get(s) {
            return o;
        }
        let o = u32::try_from(self.buffer.len()).expect("a string table under 4 GB");
        self.buffer.extend_from_slice(s);
        self.buffer.push(0);
        self.offsets.insert(s.to_vec(), o);
        self.order.push(o);
        o
    }

    fn bucket_count(n: u32) -> u32 {
        // The reference implementation grows by 3/2 + 1 when 3/4 full
        // (LLVM's table of the sizes it reaches).
        let mut strings = 0u32;
        let mut buckets = 1u32;
        for _ in 0..n {
            strings += 1;
            if buckets * 3 / 4 < strings {
                buckets = buckets * 3 / 2 + 1;
            }
        }
        buckets
    }

    /// The table's bytes: header, strings, hash buckets, count.
    pub fn serialize(&self) -> Vec<u8> {
        let n = u32::try_from(self.order.len()).expect("fewer than 4 billion strings");
        let buckets = Self::bucket_count(n);
        let mut table = vec![0u32; buckets as usize];
        for &o in &self.order {
            let end = self.buffer[o as usize..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(0);
            let s = &self.buffer[o as usize..o as usize + end];
            let h = hash_v1(s);
            for i in 0..buckets {
                let slot = ((h.wrapping_add(i)) % buckets) as usize;
                if table[slot] == 0 {
                    table[slot] = o;
                    break;
                }
            }
        }
        let mut out = Vec::with_capacity(12 + self.buffer.len() + 4 * table.len() + 8);
        out.extend_from_slice(&0xEFFE_EFFEu32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(self.buffer.len())
                .expect("small")
                .to_le_bytes(),
        );
        out.extend_from_slice(&self.buffer);
        out.extend_from_slice(&buckets.to_le_bytes());
        for b in table {
            out.extend_from_slice(&b.to_le_bytes());
        }
        out.extend_from_slice(&n.to_le_bytes());
        out
    }
}

/// The PDB info stream: version, signature, age, GUID, the named stream
/// map, and the feature codes (VC140).
pub fn info_stream(signature: u32, age: u32, guid: &[u8; 16], named: &[(&str, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20_000_404u32.to_le_bytes()); // VC70
    out.extend_from_slice(&signature.to_le_bytes());
    out.extend_from_slice(&age.to_le_bytes());
    out.extend_from_slice(guid);
    // The names, then a hash table of (name offset, stream) by the
    // names' 16-bit hashes, probing linearly; it grows as LLVM's (to
    // twice its maximum load, when that is reached).
    let mut names = Vec::new();
    let mut entries = Vec::new();
    for (n, s) in named {
        entries.push((u32::try_from(names.len()).expect("small"), *s, *n));
        names.extend_from_slice(n.as_bytes());
        names.push(0);
    }
    let max_load = |c: u32| c * 2 / 3 + 1;
    let mut capacity = 8u32;
    let mut size = 0u32;
    for _ in &entries {
        size += 1;
        if size >= max_load(capacity) {
            capacity = max_load(capacity) * 2;
        }
    }
    let mut buckets: Vec<Option<(u32, u32)>> = vec![None; capacity as usize];
    for &(off, stream, name) in &entries {
        // (The reference implementation's hash is 16 bits.)
        let h = hash_v1(name.as_bytes()) & 0xFFFF;
        let mut i = h % capacity;
        while buckets[i as usize].is_some() {
            i = (i + 1) % capacity;
        }
        buckets[i as usize] = Some((off, stream));
    }
    out.extend_from_slice(&u32::try_from(names.len()).expect("small").to_le_bytes());
    out.extend_from_slice(&names);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&capacity.to_le_bytes());
    // The present bits, then the (empty) deleted bits.
    let last = buckets
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |i| i + 1);
    let words = last.div_ceil(32);
    out.extend_from_slice(&u32::try_from(words).expect("small").to_le_bytes());
    for w in 0..words {
        let mut v = 0u32;
        for b in 0..32 {
            if buckets.get(w * 32 + b).is_some_and(Option::is_some) {
                v |= 1 << b;
            }
        }
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    for (off, stream) in buckets.iter().flatten() {
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&stream.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // niMac
    out.extend_from_slice(&20_140_508u32.to_le_bytes()); // VC140
    out
}

/// The type records' hash buckets (`NumHashBuckets`).
pub const TPI_BUCKETS: u32 = 0x3FFFF;

fn is_anonymous(name: &[u8]) -> bool {
    name == b"<unnamed-tag>"
        || name == b"__unnamed"
        || name.ends_with(b"::<unnamed-tag>")
        || name.ends_with(b"::__unnamed")
}

/// A class's, structure's, union's, or enum's properties, name, and unique
/// name (`record` whole); `None` for other records.
pub fn tag(record: &[u8]) -> Option<(u16, &[u8], &[u8])> {
    use turbonop_codeview::types::leaf;
    let kind = u16::from_le_bytes([*record.get(2)?, *record.get(3)?]);
    let body = &record[4..];
    let fixed = match kind {
        leaf::LF_CLASS | leaf::LF_STRUCTURE | leaf::LF_INTERFACE => 16,
        leaf::LF_UNION => 8,
        leaf::LF_ENUM => 12,
        _ => return None,
    };
    // Options are the second u16 for classes, unions, and enums.
    let props = u16::from_le_bytes([*body.get(2)?, *body.get(3)?]);
    let mut r = turbonop_core::Reader::new(body.get(fixed..)?);
    if kind != leaf::LF_ENUM {
        turbonop_codeview::types::read_numeric(&mut r).ok()?;
    }
    let rest = r.remaining();
    let end = rest.iter().position(|&b| b == 0)?;
    let name = &rest[..end];
    let rest = &rest[end + 1..];
    let unique = rest
        .iter()
        .position(|&b| b == 0)
        .map_or(&[][..], |e| &rest[..e]);
    Some((props, name, unique))
}

/// A type record's hash, as the TPI hash stream keys it: a UDT by its
/// name (or unique name), a source-line record by its UDT's index, every
/// other record by the CRC of its bytes. `record` is the whole record
/// (length and kind first).
pub fn type_hash(record: &[u8]) -> u32 {
    use turbonop_codeview::types::leaf;
    if record.len() < 4 {
        return hash_record(record);
    }
    let kind = u16::from_le_bytes([record[2], record[3]]);
    let body = &record[4..];
    if matches!(kind, leaf::LF_UDT_SRC_LINE | leaf::LF_UDT_MOD_SRC_LINE) {
        return hash_v1(body.get(..4).unwrap_or(body));
    }
    if let Some((props, name, unique)) = tag(record) {
        const FORWARD: u16 = 0x80;
        const SCOPED: u16 = 0x100;
        const UNIQUE: u16 = 0x200;
        let forward = props & FORWARD != 0;
        let scoped = props & SCOPED != 0;
        let has_unique = props & UNIQUE != 0;
        let anon = has_unique && is_anonymous(name);
        if !forward && !scoped && !anon {
            return hash_v1(name);
        }
        if !forward && has_unique && !anon {
            return hash_v1(unique);
        }
    }
    hash_record(record)
}

/// A TPI or IPI stream (header and records) and its hash stream, for
/// `records` (each whole, 4-byte aligned, the first index 0x1000), with
/// `hashes` each record's [`type_hash`], `hash_stream` its stream number.
pub fn tpi_streams(
    records: &[u8],
    offsets: &[u32],
    hashes: &[u32],
    hash_stream: u16,
) -> (Vec<u8>, Vec<u8>) {
    // The hash stream: each record's bucket, then an offset every 8 KB.
    let mut hash = Vec::with_capacity(hashes.len() * 4 + offsets.len() / 8);
    for &h in hashes {
        hash.extend_from_slice(&(h % TPI_BUCKETS).to_le_bytes());
    }
    let values = u32::try_from(hash.len()).expect("small");
    let mut index_offsets = Vec::new();
    let mut size = 0usize;
    for (i, &o) in offsets.iter().enumerate() {
        let next = offsets.get(i + 1).map_or(records.len(), |&n| n as usize);
        let len = next - o as usize;
        if (size + len) / 8192 > size / 8192 || i == 0 {
            index_offsets
                .extend_from_slice(&(0x1000 + u32::try_from(i).expect("small")).to_le_bytes());
            index_offsets.extend_from_slice(&o.to_le_bytes());
        }
        size += len;
    }
    hash.extend_from_slice(&index_offsets);
    let mut out = Vec::with_capacity(56 + records.len());
    let end = 0x1000 + u32::try_from(offsets.len()).expect("small");
    for v in [
        20_040_203u32,
        56,
        0x1000,
        end,
        u32::try_from(records.len()).expect("under 4 GB"),
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&hash_stream.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&TPI_BUCKETS.to_le_bytes());
    // The buffers' (offset, length): hash values, index offsets, hash
    // adjustments (none; written after the values, before the offsets).
    let index_len = u32::try_from(index_offsets.len()).expect("small");
    for v in [0, values, values, index_len, values, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(records);
    (out, hash)
}
