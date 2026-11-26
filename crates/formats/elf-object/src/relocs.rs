//! A relocation section's entries, kept as the file holds them until an
//! edit needs them as values.
//!
//! Most relocation sections are read and written back unchanged (an
//! object's `.rela.debug_*` sections are a quarter of its bytes), or only
//! renumbered: decoding every entry into a [`Relocation`] and encoding it
//! again cost a copy of each, and a heap allocation the size of the
//! section. [`Relocations`] reads entries from the file's bytes on demand,
//! renumbers their symbols in a copy of those bytes, and removes entries
//! by copying the others; only [`Relocations::values_mut`] decodes them.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::{Class, Endian, Relocation};

/// How a relocation section's entries are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form {
    pub class: Class,
    pub endian: Endian,
    /// `SHT_RELA` (an addend in each entry), else `SHT_REL`.
    pub rela: bool,
}

impl Form {
    /// The size of a word.
    fn word(self) -> usize {
        match self.class {
            Class::Elf32 => 4,
            Class::Elf64 => 8,
        }
    }

    /// The size of an entry.
    pub fn entry(self) -> usize {
        self.word() * if self.rela { 3 } else { 2 }
    }

    fn read(self, b: &[u8]) -> u64 {
        match (self.class, self.endian) {
            (Class::Elf32, Endian::Little) => {
                u64::from(u32::from_le_bytes(b[..4].try_into().expect("a word")))
            }
            (Class::Elf32, Endian::Big) => {
                u64::from(u32::from_be_bytes(b[..4].try_into().expect("a word")))
            }
            (Class::Elf64, Endian::Little) => {
                u64::from_le_bytes(b[..8].try_into().expect("a word"))
            }
            (Class::Elf64, Endian::Big) => u64::from_be_bytes(b[..8].try_into().expect("a word")),
        }
    }

    fn put(self, b: &mut [u8], v: u64) {
        match (self.class, self.endian) {
            (Class::Elf32, Endian::Little) => b[..4].copy_from_slice(&(v as u32).to_le_bytes()),
            (Class::Elf32, Endian::Big) => b[..4].copy_from_slice(&(v as u32).to_be_bytes()),
            (Class::Elf64, Endian::Little) => b[..8].copy_from_slice(&v.to_le_bytes()),
            (Class::Elf64, Endian::Big) => b[..8].copy_from_slice(&v.to_be_bytes()),
        }
    }

    /// The entry `b` holds (one entry's bytes).
    fn decode(self, b: &[u8]) -> Relocation {
        let w = self.word();
        let offset = self.read(b);
        let info = self.read(&b[w..]);
        let addend = if self.rela {
            let a = self.read(&b[2 * w..]);
            match self.class {
                Class::Elf32 => i64::from(a as u32 as i32),
                Class::Elf64 => a as i64,
            }
        } else {
            0
        };
        Relocation {
            offset,
            info,
            addend,
        }
    }
}

#[derive(Clone, Debug)]
enum Repr<'a> {
    /// Encoded entries (a whole number of them).
    File {
        bytes: Cow<'a, [u8]>,
        form: Form,
    },
    Values(Vec<Relocation>),
    /// A CREL section's entries, decoded as read (an entry's size varies),
    /// with the bytes read while nothing has changed them, and whether the
    /// section has addends.
    Crel {
        bytes: Option<Cow<'a, [u8]>>,
        values: Vec<Relocation>,
        addends: bool,
    },
}

/// A relocation section's entries (`SHT_REL`, `SHT_RELA`, or `SHT_CREL`).
#[derive(Clone, Debug)]
pub struct Relocations<'a> {
    repr: Repr<'a>,
}

impl Default for Relocations<'_> {
    fn default() -> Self {
        Self {
            repr: Repr::Values(Vec::new()),
        }
    }
}

impl From<Vec<Relocation>> for Relocations<'_> {
    fn from(v: Vec<Relocation>) -> Self {
        Self {
            repr: Repr::Values(v),
        }
    }
}

impl PartialEq for Relocations<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Eq for Relocations<'_> {}

impl<'a> Relocations<'a> {
    /// The entries `bytes` holds, encoded as `form` (`None` unless a whole
    /// number of entries).
    pub fn encoded(bytes: Cow<'a, [u8]>, form: Form) -> Option<Self> {
        bytes.len().is_multiple_of(form.entry()).then_some(Self {
            repr: Repr::File { bytes, form },
        })
    }

    /// A CREL section's entries (`values`, decoded from `bytes`), and
    /// whether it has addends: written back as `bytes` until changed.
    pub fn crel(bytes: Cow<'a, [u8]>, values: Vec<Relocation>, addends: bool) -> Self {
        Self {
            repr: Repr::Crel {
                bytes: Some(bytes),
                values,
                addends,
            },
        }
    }

    /// How many entries.
    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::File { bytes, form } => bytes.len() / form.entry(),
            Repr::Values(v) | Repr::Crel { values: v, .. } => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entry `i`.
    pub fn get(&self, i: usize) -> Option<Relocation> {
        match &self.repr {
            Repr::File { bytes, form } => {
                let n = form.entry();
                bytes.get(i * n..i * n + n).map(|b| form.decode(b))
            }
            Repr::Values(v) | Repr::Crel { values: v, .. } => v.get(i).copied(),
        }
    }

    /// The entries, in order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: match &self.repr {
                Repr::File { bytes, form } => IterRepr::File {
                    chunks: bytes.chunks_exact(form.entry()),
                    form: *form,
                },
                Repr::Values(v) | Repr::Crel { values: v, .. } => IterRepr::Values(v.iter()),
            },
        }
    }

    /// The entries as values.
    pub fn to_vec(&self) -> Vec<Relocation> {
        match &self.repr {
            Repr::File { .. } => self.iter().collect(),
            Repr::Values(v) | Repr::Crel { values: v, .. } => v.clone(),
        }
    }

    /// The entries, to change as values (decoded if they weren't).
    pub fn values_mut(&mut self) -> &mut Vec<Relocation> {
        if let Repr::File { .. } = self.repr {
            self.repr = Repr::Values(self.to_vec());
        }
        match &mut self.repr {
            Repr::Values(v) => v,
            Repr::Crel { bytes, values, .. } => {
                *bytes = None;
                values
            }
            Repr::File { .. } => unreachable!("decoded above"),
        }
    }

    /// Keeps the entries `f` says, in order.
    pub fn retain(&mut self, mut f: impl FnMut(&Relocation) -> bool) {
        match &mut self.repr {
            Repr::File { bytes, form } => {
                let n = form.entry();
                let mut out = Vec::with_capacity(bytes.len());
                for b in bytes.chunks_exact(n) {
                    if f(&form.decode(b)) {
                        out.extend_from_slice(b);
                    }
                }
                *bytes = Cow::Owned(out);
            }
            Repr::Values(v) => v.retain(f),
            Repr::Crel { bytes, values, .. } => {
                let before = values.len();
                values.retain(f);
                if values.len() != before {
                    *bytes = None;
                }
            }
        }
    }

    /// Replaces each entry's `r_info` by `f` of it.
    pub fn map_info(&mut self, mut f: impl FnMut(u64) -> u64) {
        match &mut self.repr {
            Repr::File { bytes, form } => {
                let (n, w) = (form.entry(), form.word());
                for b in bytes.to_mut().chunks_exact_mut(n) {
                    let info = form.read(&b[w..]);
                    let new = f(info);
                    if new != info {
                        form.put(&mut b[w..], new);
                    }
                }
            }
            Repr::Values(v) => {
                for r in v {
                    r.info = f(r.info);
                }
            }
            Repr::Crel { bytes, values, .. } => {
                for r in values {
                    let new = f(r.info);
                    if new != r.info {
                        r.info = new;
                        *bytes = None;
                    }
                }
            }
        }
    }

    /// The entries' bytes, if they are held encoded as `form`.
    pub(crate) fn bytes_as(&self, form: Form) -> Option<&[u8]> {
        match &self.repr {
            Repr::File { bytes, form: f } if *f == form => Some(bytes),
            _ => None,
        }
    }

    /// A CREL section's bytes as read, if nothing has changed its entries.
    pub(crate) fn crel_bytes(&self) -> Option<&[u8]> {
        match &self.repr {
            Repr::Crel { bytes: Some(b), .. } => Some(b),
            _ => None,
        }
    }

    /// Whether a CREL section read has addends (`None`: not one read).
    pub fn crel_addends(&self) -> Option<bool> {
        match &self.repr {
            Repr::Crel { addends, .. } => Some(*addends),
            _ => None,
        }
    }
}

impl<'r> IntoIterator for &'r Relocations<'_> {
    type Item = Relocation;
    type IntoIter = Iter<'r>;
    fn into_iter(self) -> Iter<'r> {
        self.iter()
    }
}

/// The entries of [`Relocations`], as values.
#[derive(Clone, Debug)]
pub struct Iter<'r> {
    inner: IterRepr<'r>,
}

#[derive(Clone, Debug)]
enum IterRepr<'r> {
    File {
        chunks: core::slice::ChunksExact<'r, u8>,
        form: Form,
    },
    Values(core::slice::Iter<'r, Relocation>),
}

impl Iterator for Iter<'_> {
    type Item = Relocation;

    fn next(&mut self) -> Option<Relocation> {
        match &mut self.inner {
            IterRepr::File { chunks, form } => chunks.next().map(|b| form.decode(b)),
            IterRepr::Values(v) => v.next().copied(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = match &self.inner {
            IterRepr::File { chunks, .. } => chunks.len(),
            IterRepr::Values(v) => v.len(),
        };
        (n, Some(n))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Relocation> {
        match &mut self.inner {
            IterRepr::File { chunks, form } => chunks.next_back().map(|b| form.decode(b)),
            IterRepr::Values(v) => v.next_back().copied(),
        }
    }
}

impl ExactSizeIterator for Iter<'_> {}
/// One CREL entry's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrelEntry {
    pub offset: u64,
    pub symbol: u32,
    pub kind: u32,
    pub addend: i64,
}

/// `CREL_HDR_ADDEND`: the entries have addends.
const CREL_HDR_ADDEND: u64 = 4;

fn read_uleb(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= u64::from(x & 0x7F) << shift;
        }
        shift += 7;
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
}

fn read_sleb(b: &[u8], at: &mut usize) -> Option<i64> {
    let mut v = 0i64;
    let mut shift = 0u32;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= i64::from(x & 0x7F) << shift;
        }
        shift += 7;
        if x & 0x80 == 0 {
            if shift < 64 && x & 0x40 != 0 {
                v |= -1i64 << shift;
            }
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

fn put_sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        if done {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// The entries of a CREL section (`SHT_CREL`, LLVM's compact relocations),
/// as LLVM's `decodeCrel` reads them, and whether they have addends;
/// `None` if they don't decode or bytes follow them. Offsets and addends
/// wrap at the class's word.
pub fn decode_crel(b: &[u8], class: Class) -> Option<(Vec<CrelEntry>, bool)> {
    let mask = match class {
        Class::Elf32 => u64::from(u32::MAX),
        Class::Elf64 => u64::MAX,
    };
    let mut at = 0;
    let hdr = read_uleb(b, &mut at)?;
    let count = usize::try_from(hdr / 8).ok()?;
    let addends = hdr & CREL_HDR_ADDEND != 0;
    let flag_bits = if addends { 3 } else { 2 };
    let shift = hdr % CREL_HDR_ADDEND;
    // (Each entry takes a byte at least.)
    if count > b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    let (mut offset, mut addend) = (0u64, 0u64);
    let (mut symbol, mut kind) = (0u32, 0u32);
    for _ in 0..count {
        let first = *b.get(at)?;
        at += 1;
        offset = offset.wrapping_add(u64::from(first >> flag_bits));
        if first >= 0x80 {
            let more = read_uleb(b, &mut at)?;
            offset = offset
                .wrapping_add(more.wrapping_shl(7 - flag_bits))
                .wrapping_sub(0x80 >> flag_bits);
        }
        if first & 1 != 0 {
            symbol = symbol.wrapping_add(read_sleb(b, &mut at)? as u32);
        }
        if first & 2 != 0 {
            kind = kind.wrapping_add(read_sleb(b, &mut at)? as u32);
        }
        if addends && first & 4 != 0 {
            addend = addend.wrapping_add(read_sleb(b, &mut at)? as u64);
        }
        offset &= mask;
        addend &= mask;
        let a = match class {
            Class::Elf32 => i64::from(addend as u32 as i32),
            Class::Elf64 => addend as i64,
        };
        out.push(CrelEntry {
            offset: offset.wrapping_shl(shift as u32) & mask,
            symbol,
            kind,
            addend: a,
        });
    }
    (at == b.len()).then_some((out, addends))
}

/// `entries` encoded as LLVM's `encodeCrel` writes them (with addends, as
/// LLVM's assembler always has, or without: then each entry's addend must
/// be 0).
pub fn encode_crel(entries: &[CrelEntry], class: Class, addends: bool) -> Vec<u8> {
    let mask = match class {
        Class::Elf32 => u64::from(u32::MAX),
        Class::Elf64 => u64::MAX,
    };
    let flag_bits: u32 = if addends { 3 } else { 2 };
    let mut offset_mask = 8u64;
    for e in entries {
        offset_mask |= e.offset;
    }
    let shift = u64::from(offset_mask.trailing_zeros());
    let mut out = Vec::with_capacity(entries.len() * 2 + 4);
    put_uleb(
        &mut out,
        entries.len() as u64 * 8 + if addends { CREL_HDR_ADDEND } else { 0 } + shift,
    );
    let (mut offset, mut addend) = (0u64, 0u64);
    let (mut symbol, mut kind) = (0u32, 0u32);
    for e in entries {
        let delta = (e.offset.wrapping_sub(offset) & mask) >> shift;
        offset = e.offset;
        let a = (e.addend as u64) & mask;
        let flags = u8::from(symbol != e.symbol)
            | if kind == e.kind { 0 } else { 2 }
            | if addends && addend != a { 4 } else { 0 };
        let b = ((delta << flag_bits) as u8) | flags;
        if delta < (0x80 >> flag_bits) {
            out.push(b);
        } else {
            out.push(b | 0x80);
            put_uleb(&mut out, delta >> (7 - flag_bits));
        }
        if flags & 1 != 0 {
            put_sleb(&mut out, i64::from(e.symbol.wrapping_sub(symbol) as i32));
            symbol = e.symbol;
        }
        if flags & 2 != 0 {
            put_sleb(&mut out, i64::from(e.kind.wrapping_sub(kind) as i32));
            kind = e.kind;
        }
        if flags & 4 != 0 {
            let d = a.wrapping_sub(addend) & mask;
            put_sleb(
                &mut out,
                match class {
                    Class::Elf32 => i64::from(d as u32 as i32),
                    Class::Elf64 => d as i64,
                },
            );
            addend = a;
        }
    }
    out
}
