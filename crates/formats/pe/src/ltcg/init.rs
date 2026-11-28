//! `.cil$in`: the variables' initial values. Each variable's initializer
//! is led by tag 0x00 and its id and ends with 0x07; between, its items
//! in order, each led by a tag; 0x0F ends the section. See
//! docs/ltcg-il.md.

use alloc::vec::Vec;
use turbonop_core::Result;

use super::Reader;

/// One of an initializer's items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitItem<'data> {
    /// An integer (tag 0x01): `kind` 1 signed, 2 unsigned, 3 a pointer, 4
    /// (not known), its size in bytes, and its value's bytes: one byte for
    /// a small value (sign-extended for a signed kind), else `size` bytes,
    /// little-endian.
    Integer {
        kind: u8,
        size: u32,
        raw: &'data [u8],
    },
    /// A floating-point value (tag 0x01, kind 5): its bytes.
    Float { size: u32, bytes: &'data [u8] },
    /// A symbol's address plus `addend` (tag 0x02), `size` bytes.
    Address { target: u32, addend: u32, size: u32 },
    /// A symbol's address relative to the image's base (tag 0x12: RTTI's
    /// 32-bit offsets), plus `addend`, `size` bytes.
    ImageRelative { target: u32, addend: u32, size: u32 },
    /// Bytes as they are (tag 0x03: strings, arrays of bytes).
    Bytes(&'data [u8]),
    /// So many zero bytes (tag 0x08: the rest of an aggregate, padding).
    Zeros(u32),
}

impl InitItem<'_> {
    /// How many bytes it fills.
    pub fn size(&self) -> u64 {
        match *self {
            Self::Integer { size, .. }
            | Self::Float { size, .. }
            | Self::Address { size, .. }
            | Self::ImageRelative { size, .. }
            | Self::Zeros(size) => size.into(),
            Self::Bytes(b) => b.len() as u64,
        }
    }

    /// An integer's value, as `size` bytes would hold it (two's
    /// complement), if it fits 64 bits.
    pub fn integer(&self) -> Option<u64> {
        let Self::Integer { kind, size, raw } = *self else {
            return None;
        };
        if size > 8 || raw.len() > 8 || raw.is_empty() {
            return None;
        }
        let negative = raw.len() == 1 && kind == 1 && size > 1 && raw[0] & 0x80 != 0;
        let mut b = if negative { [0xFF; 8] } else { [0; 8] };
        b[..raw.len()].copy_from_slice(raw);
        let v = u64::from_le_bytes(b);
        Some(if size == 8 {
            v
        } else {
            v & ((1u64 << (size * 8)) - 1)
        })
    }
}

/// A variable's initializer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initializer<'data> {
    /// Where it starts and ends (past its end tag) in `.cil$in`.
    pub offset: usize,
    pub end: usize,
    /// The variable's id (a `.cil$gl` record's, a local static's, or one
    /// with no record: a variable nothing refers to).
    pub id: u32,
    pub items: Vec<InitItem<'data>>,
}

impl Initializer<'_> {
    /// How many bytes its items fill (image-relative offsets at their 4
    /// bytes, where the variable's size in the IL counts pointers).
    pub fn size(&self) -> u64 {
        self.items.iter().map(InitItem::size).sum()
    }
}

/// `.cil$in`, read.
#[derive(Clone, Debug)]
pub struct Initializers<'data> {
    pub list: Vec<Initializer<'data>>,
    /// Whether it was walked to its end tag at its end; else where the
    /// first item not understood starts.
    pub walked: core::result::Result<(), usize>,
}

impl Default for Initializers<'_> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            walked: Ok(()),
        }
    }
}

impl<'data> Initializers<'data> {
    pub fn parse(d: &'data [u8]) -> Result<Self> {
        let mut out = Self::default();
        if d.is_empty() {
            return Ok(out);
        }
        let mut r = Reader::new(d, 0);
        loop {
            let at = r.p;
            match r.u8() {
                Some(0x0F) if r.p == d.len() => return Ok(out),
                Some(0x00) => match initializer(&mut r, at) {
                    Some(i) => out.list.push(i),
                    None => {
                        out.walked = Err(at);
                        return Ok(out);
                    }
                },
                _ => {
                    out.walked = Err(at);
                    return Ok(out);
                }
            }
        }
    }

    /// The initializer of the variable whose id is `id`.
    pub fn of(&self, id: u32) -> Option<&Initializer<'data>> {
        self.list.iter().find(|i| i.id == id)
    }
}

/// An initializer, from past its tag (at `offset`).
fn initializer<'d>(r: &mut Reader<'d>, offset: usize) -> Option<Initializer<'d>> {
    let id = r.id()?;
    // (Zero where seen.)
    let _unknown = r.c32()?;
    let mut items = Vec::new();
    loop {
        let item = match r.u8()? {
            0x07 => {
                return Some(Initializer {
                    offset,
                    end: r.p,
                    id,
                    items,
                });
            }
            0x01 => {
                let kind = r.u8()?;
                let size = r.c32()?;
                if kind == 5 {
                    InitItem::Float {
                        size,
                        bytes: r.take(size as usize)?,
                    }
                } else {
                    // (A byte-sized value is its byte, whatever it is.)
                    let first = *r.d.get(r.p)?;
                    let raw = if size == 1 || first != 0x80 {
                        r.take(1)?
                    } else {
                        r.p += 1;
                        r.take(size as usize)?
                    };
                    InitItem::Integer { kind, size, raw }
                }
            }
            t @ (0x02 | 0x12) => {
                let target = r.id()?;
                let addend = r.c32()?;
                let size = r.c32()?;
                if t == 0x02 {
                    InitItem::Address {
                        target,
                        addend,
                        size,
                    }
                } else {
                    InitItem::ImageRelative {
                        target,
                        addend,
                        size,
                    }
                }
            }
            0x03 => {
                let n = r.c16()?;
                InitItem::Bytes(r.take(n.into())?)
            }
            0x08 => InitItem::Zeros(r.c32()?),
            _ => return None,
        };
        items.push(item);
    }
}
