//! An archive's symbol index: Microsoft's two linker members (or a GNU
//! or BSD table), read as (name, member header offset) pairs.

use turbonop_archive::{Archive, MemberKind};

/// The archive's index.
#[derive(Clone, Debug, Default)]
pub struct Index<'a> {
    /// The first table (Microsoft's first linker member: in member order),
    /// as (name, member header offset).
    pub first: Vec<(&'a [u8], u64)>,
    /// Microsoft's second linker member (sorted by name), if there is one.
    pub second: Option<Vec<(&'a [u8], u64)>>,
    /// Whether there is an `/<ECSYMBOLS>/` table (ARM64EC).
    pub ec: bool,
    /// Whether a table could not be read in full.
    pub damaged: bool,
}

impl<'a> Index<'a> {
    /// The pairs to check: the second linker member's, or the first's.
    pub fn entries(&self) -> &[(&'a [u8], u64)] {
        self.second.as_deref().unwrap_or(&self.first)
    }
}

fn be32(b: &[u8; 4]) -> u64 {
    u64::from(u32::from_be_bytes(*b))
}

fn le32(b: &[u8]) -> usize {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

/// The NUL-terminated names at the start of `data`, `count` of them.
fn names(mut data: &[u8], count: usize) -> Option<Vec<&[u8]>> {
    let mut out = Vec::with_capacity(count.min(1 << 22));
    for _ in 0..count {
        let end = data.iter().position(|&b| b == 0)?;
        out.push(&data[..end]);
        data = &data[end + 1..];
    }
    Some(out)
}

/// GNU's table and Microsoft's first linker member: a big-endian count,
/// that many big-endian offsets, then the names.
fn first(data: &[u8]) -> Option<Vec<(&[u8], u64)>> {
    let count = usize::try_from(be32(data.first_chunk::<4>()?)).ok()?;
    let offsets = data.get(4..4 + count.checked_mul(4)?)?;
    let names = names(&data[4 + count * 4..], count)?;
    Some(
        names
            .into_iter()
            .zip(offsets.as_chunks::<4>().0.iter().map(be32))
            .collect(),
    )
}

/// Microsoft's second linker member: a member count, the members'
/// offsets, a symbol count, each symbol's 1-based member number (16
/// bits), then the names.
fn second(data: &[u8]) -> Option<Vec<(&[u8], u64)>> {
    let members = le32(data.get(..4)?);
    let offsets_end = 4usize.checked_add(members.checked_mul(4)?)?;
    let offsets = data.get(4..offsets_end)?;
    let symbols = le32(data.get(offsets_end..offsets_end + 4)?);
    let numbers_start = offsets_end + 4;
    let numbers = data.get(numbers_start..numbers_start.checked_add(symbols.checked_mul(2)?)?)?;
    let names = names(&data[numbers_start + symbols * 2..], symbols)?;
    let mut out = Vec::with_capacity(symbols);
    for (name, n) in names.into_iter().zip(numbers.as_chunks::<2>().0) {
        let number = usize::from(u16::from_le_bytes([n[0], n[1]]));
        let at = number.checked_sub(1)?.checked_mul(4)?;
        let offset = offsets.get(at..at + 4).map(|b| le32(b) as u64)?;
        out.push((name, offset));
    }
    Some(out)
}

/// Reads `archive`'s index.
pub fn read<'a>(archive: &Archive<'a>) -> Index<'a> {
    let mut index = Index::default();
    let mut tables = 0;
    for m in archive.members().take_while(|m| m.kind != MemberKind::File) {
        match m.kind {
            MemberKind::SymbolTable if &*m.name == b"/" => {
                tables += 1;
                let read = if tables == 1 {
                    first(m.data).map(|t| index.first = t)
                } else {
                    second(m.data).map(|t| index.second = Some(t))
                };
                index.damaged |= read.is_none();
            }
            MemberKind::EcSymbolTable => index.ec = true,
            _ => {}
        }
    }
    index
}
