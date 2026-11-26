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
}

/// A relocation section's entries (`SHT_REL` or `SHT_RELA`).
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
        (bytes.len() % form.entry() == 0).then_some(Self {
            repr: Repr::File { bytes, form },
        })
    }

    /// How many entries.
    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::File { bytes, form } => bytes.len() / form.entry(),
            Repr::Values(v) => v.len(),
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
            Repr::Values(v) => v.get(i).copied(),
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
                Repr::Values(v) => IterRepr::Values(v.iter()),
            },
        }
    }

    /// The entries as values.
    pub fn to_vec(&self) -> Vec<Relocation> {
        match &self.repr {
            Repr::File { .. } => self.iter().collect(),
            Repr::Values(v) => v.clone(),
        }
    }

    /// The entries, to change as values (decoded if they weren't).
    pub fn values_mut(&mut self) -> &mut Vec<Relocation> {
        if let Repr::File { .. } = self.repr {
            self.repr = Repr::Values(self.to_vec());
        }
        match &mut self.repr {
            Repr::Values(v) => v,
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
        }
    }

    /// The entries' bytes, if they are held encoded as `form`.
    pub(crate) fn bytes_as(&self, form: Form) -> Option<&[u8]> {
        match &self.repr {
            Repr::File { bytes, form: f } if *f == form => Some(bytes),
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
