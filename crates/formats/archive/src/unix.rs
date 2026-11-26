//! Writing archives in the GNU and BSD dialects (`.a`), byte for byte as
//! GNU `ar` and `llvm-ar` write them, regular and thin.
//!
//! **GNU** (System V): `!<arch>\n` (`!<thin>\n` for a thin archive), then
//!
//! 1. the **symbol table** `/`: a big-endian count, each symbol's member
//!    header offset (big-endian, 32-bit), then the NUL-terminated names, in
//!    the index's order (each member's symbols in its symbol table's order,
//!    duplicates kept: see [`crate::symbols::elf_symbols`]), padded to an
//!    even size with NUL inside its size. When an offset doesn't fit in 32
//!    bits, it is `/SYM64/`, with 64-bit count and offsets (see
//!    [`Archiver`] for when, and how each pads it);
//! 2. the **long names** `//`, if a name doesn't fit its header: each name
//!    followed by `/\n`, the table padded to an even size with `\n` inside
//!    its size. Its date, owner, group, and mode are blank;
//! 3. the members, each a 60-byte header and its data, then `\n` if the
//!    data's length is odd. A name that fits is stored as `name/`, padded
//!    with spaces; a longer one as `/<offset>` into the long names.
//!
//! A **thin** archive holds no data: each member's header names its file,
//! always through the long names (a path, relative to the archive's
//! directory), its size is the file's, and the next header follows it
//! directly. GNU `ar` can also put a regular archive's members into a thin
//! archive: such a member's name is `/<offset>:<origin>`, the archive's
//! path in the long names and the member's header offset in it.
//!
//! **BSD**: names are `#1/<length>`, the name stored at the start of the
//! member's data (within its size), and the symbol table is `__.SYMDEF` (or
//! `__.SYMDEF_64`): the byte count of its entries, each a name offset and a
//! member header offset, then the string table's byte count and the
//! strings. `llvm-ar --format=bsd` and `--format=darwin` write it; this GNU
//! `ar` (2.46, built for x86-64 ELF) writes BSD archives only for targets
//! it isn't built for, so only `llvm-ar`'s are written here.
//!
//! The members' own header fields (date, owner, group, and mode) are the
//! caller's ([`Fields`]): `ar`'s deterministic mode (`D`, the default of
//! both tools) writes `0`, `0`, `0`, and `644`; without it (`U`), GNU `ar`
//! writes the file's time, owner, group, and whole mode (`100644`), and
//! `llvm-ar` its permissions alone (`644`).
//!
//! As for the Microsoft dialect ([`crate::write::Layout`]), [`Layout`]
//! plans everything before anything is written, so members can be copied
//! in parallel.

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::write::{Fields, IndexSymbol};
use crate::{Archive, MAGIC, Member, MemberKind, THIN_MAGIC};

pub mod symbols;

const HEADER_LEN: usize = 60;

/// Whose conventions to follow.
///
/// Where GNU `ar` (binutils 2.46) and `llvm-ar` (LLVM 24) differ, in the
/// GNU dialect (measured, and read in `bfd/archive.c` and LLVM's
/// `ArchiveWriter.cpp`):
///
/// | | GNU `ar` | `llvm-ar` |
/// |---|---|---|
/// | a name in the long names | longer than 15 bytes | 16 bytes or more, or holding `/` |
/// | a name two members share | in the long names twice (but a thin archive's same file twice in a row, once) | once (a thin archive's, each time) |
/// | a symbol table without symbols | a count of 0 (written if any member is an object) | a count of 0 and three NULs (written if any member is an object) |
/// | `/SYM64/` | when the header offset of a member with symbols passes 32 bits; padded to 8 bytes | when the last member's header offset reaches 2^32 (`SYM64_THRESHOLD` sets another); padded to 2 bytes |
///
/// What makes a member an object differs too (the formats each tool
/// reads); that is for the caller to say ([`Entry::object`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Archiver {
    /// GNU `ar` (and `ranlib`).
    GnuAr,
    /// `llvm-ar` (and `llvm-ranlib`).
    LlvmAr,
}

/// The dialect to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// GNU (System V): `/`, `/SYM64/`, `//`.
    Gnu,
    /// BSD (`llvm-ar --format=bsd`): `#1/` names, `__.SYMDEF`.
    Bsd,
    /// Darwin (`--format=darwin`): BSD, with each member's data padded to
    /// 8 bytes with `\n` (inside its size), and `__.SYMDEF_64` past 32 bits.
    Darwin,
}

/// How to write an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    /// Whose conventions.
    pub archiver: Archiver,
    /// Which dialect.
    pub dialect: Dialect,
    /// A thin archive (GNU dialect only).
    pub thin: bool,
    /// The symbol table's date field: 0 in deterministic mode (`D`), the
    /// time otherwise.
    pub table_date: u64,
    /// For `llvm-ar`: the offset at which a 64-bit symbol table is used,
    /// 2^32 unless its `SYM64_THRESHOLD` says otherwise.
    pub sym64_threshold: u64,
    /// For `llvm-ar`: whether a symbol table without symbols holds three
    /// NULs (it does when a member is an object; a Darwin archive's table,
    /// written whatever the members, is empty otherwise).
    pub filler: bool,
}

impl Options {
    /// `archiver`'s deterministic defaults, in `dialect`.
    pub fn new(archiver: Archiver, dialect: Dialect) -> Self {
        Self {
            archiver,
            dialect,
            thin: false,
            table_date: 0,
            sym64_threshold: 1 << 32,
            filler: true,
        }
    }
}

/// One member to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The name, as stored (for a thin archive, the path).
    pub name: &'a [u8],
    /// For a GNU thin archive's member of another archive (`name`): its
    /// header's offset there.
    pub origin: Option<u64>,
    /// The date, owner, group, and mode fields.
    pub fields: Fields,
    /// The data's size (the file's, for a thin archive).
    pub size: u64,
}

/// Why an archive can't be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A member's size (with its BSD name) doesn't fit its header's ten
    /// digits.
    MemberTooLarge {
        /// The member's position.
        member: usize,
    },
    /// A member's name holds a byte its header can't (a NUL; for GNU, a
    /// `\n`, which ends a long name).
    BadName {
        /// The member's position.
        member: usize,
    },
    /// A symbol's name holds a NUL.
    BadSymbol {
        /// Its position in the index.
        symbol: usize,
    },
    /// A symbol names a member that isn't there.
    NoSuchMember {
        /// Its position in the index.
        symbol: usize,
    },
    /// An offset doesn't fit the table (a BSD symbol table past 32 bits,
    /// or a header's name field).
    TooLarge,
    /// A thin archive in the BSD dialect, or an origin in a regular one
    /// (neither tool writes them).
    NotWritable,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::MemberTooLarge { member } => {
                write!(f, "member {member} is too large for its header")
            }
            WriteError::BadName { member } => {
                write!(f, "member {member}'s name can't be stored")
            }
            WriteError::BadSymbol { symbol } => write!(f, "symbol {symbol}'s name holds a NUL"),
            WriteError::NoSuchMember { symbol } => {
                write!(f, "symbol {symbol} names a member that isn't there")
            }
            WriteError::TooLarge => f.write_str("an offset doesn't fit the archive's tables"),
            WriteError::NotWritable => f.write_str("neither tool writes such an archive"),
        }
    }
}

impl core::error::Error for WriteError {}

/// Appends `n` in decimal.
fn decimal(out: &mut Vec<u8>, n: u64) {
    let mut buf = [0u8; 20];
    let mut at = buf.len();
    let mut n = n;
    loop {
        at -= 1;
        buf[at] = b'0' + u8::try_from(n % 10).unwrap_or(0);
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[at..]);
}

/// `text` padded with spaces to `width` bytes (cut to it if longer).
fn padded(out: &mut Vec<u8>, text: &[u8], width: usize) {
    let n = text.len().min(width);
    out.extend_from_slice(&text[..n]);
    out.resize(out.len() + (width - n), b' ');
}

/// A header: the name field's text, the fields, and the size.
fn header(out: &mut Vec<u8>, name: &[u8], fields: &Fields, size: u64) {
    padded(out, name, 16);
    padded(out, fields.date(), 12);
    padded(out, fields.uid(), 6);
    padded(out, fields.gid(), 6);
    padded(out, fields.mode(), 8);
    let mut s = Vec::with_capacity(10);
    decimal(&mut s, size);
    padded(out, &s, 10);
    out.extend_from_slice(b"`\n");
}

/// The symbol table's fields: its date, owner 0, group 0, and mode 0.
fn table_fields(date: u64) -> Fields {
    let mut d = Vec::new();
    decimal(&mut d, date);
    Fields::new(&d[..d.len().min(12)], b"0", b"0", b"0").unwrap_or(Fields::BLANK)
}

/// How a member's name is stored.
#[derive(Clone, Copy, Debug)]
enum Name {
    /// `name/` in the header.
    Short,
    /// `/<offset>` into the long names (with `:<origin>`).
    Long(u64),
    /// `#1/<length>`: the name and this many NULs after it.
    Bsd,
}

/// Where everything in a GNU or BSD archive goes, worked out before any of
/// it is written: the bytes before the first member (the signature, the
/// symbol table, and the long names), and each member's header (with a
/// BSD name) and offset.
#[derive(Clone, Debug)]
pub struct Layout {
    head: Vec<u8>,
    /// All the members' headers, one after another.
    headers: Vec<u8>,
    /// Each member: where its header starts in `headers`, its header's
    /// offset in the archive, its data's size in the archive (0 when
    /// thin), and the padding after the data.
    members: Vec<(usize, u64, u64, u8)>,
    size: u64,
    sym64: bool,
}

impl Layout {
    /// Lays out an archive of `members`, in this order, with a symbol table
    /// listing `symbols` in this order (`None`: no symbol table).
    pub fn new(
        members: &[Entry<'_>],
        symbols: Option<&[IndexSymbol<'_>]>,
        options: &Options,
    ) -> Result<Self, WriteError> {
        let bsd = options.dialect != Dialect::Gnu;
        let darwin = options.dialect == Dialect::Darwin;
        let llvm = options.archiver == Archiver::LlvmAr;
        if bsd && (options.thin || options.archiver == Archiver::GnuAr) {
            return Err(WriteError::NotWritable);
        }
        let mut names_len = 0usize;
        if let Some(symbols) = symbols {
            for (i, s) in symbols.iter().enumerate() {
                if s.member as usize >= members.len() {
                    return Err(WriteError::NoSuchMember { symbol: i });
                }
                if s.name.contains(&0) {
                    return Err(WriteError::BadSymbol { symbol: i });
                }
                names_len += s.name.len() + 1;
            }
        }
        // llvm-ar's three NULs, for a table without symbols.
        let filler = llvm && options.filler && symbols.is_some_and(<[_]>::is_empty);
        if filler {
            names_len = 3;
        }

        // The long names, and how each member's name is stored.
        let mut long: Vec<u8> = Vec::new();
        let mut seen: alloc::collections::BTreeMap<&[u8], u64> =
            alloc::collections::BTreeMap::new();
        let mut stored = Vec::with_capacity(members.len());
        let mut previous: Option<(&[u8], u64)> = None;
        for (i, m) in members.iter().enumerate() {
            if m.name.contains(&0) || (!bsd && m.name.contains(&b'\n')) {
                return Err(WriteError::BadName { member: i });
            }
            if m.origin.is_some() && !(options.thin && !llvm) {
                return Err(WriteError::NotWritable);
            }
            if bsd {
                stored.push(Name::Bsd);
                continue;
            }
            let is_long = options.thin
                || if llvm {
                    m.name.len() >= 16 || m.name.contains(&b'/')
                } else {
                    m.name.len() > 15
                };
            if !is_long {
                stored.push(Name::Short);
                continue;
            }
            let reuse = if options.thin {
                // GNU: the same path as the member before it, once.
                previous.filter(|&(n, _)| !llvm && n == m.name).map(|p| p.1)
            } else if llvm {
                seen.get(m.name).copied()
            } else {
                None
            };
            let at = reuse.unwrap_or_else(|| {
                let at = long.len() as u64;
                long.extend_from_slice(m.name);
                long.extend_from_slice(b"/\n");
                if llvm && !options.thin {
                    seen.insert(m.name, at);
                }
                at
            });
            previous = Some((m.name, at));
            stored.push(Name::Long(at));
        }
        let long_member = if long.is_empty() {
            0
        } else {
            HEADER_LEN as u64 + long.len() as u64 + (long.len() as u64 & 1)
        };

        // Each member's header (but a BSD name's padding, which depends on
        // where it lands) and span, from the first member.
        let span = |m: &Entry<'_>, header: u64| -> u64 {
            if options.thin {
                header
            } else {
                let data = m.size + if darwin { m.size.wrapping_neg() & 7 } else { 0 };
                header + data + (data & 1)
            }
        };

        let n = symbols.map_or(0, <[_]>::len) as u64;
        let names_len = names_len as u64;
        // The symbol table member's size (header included) in each form.
        let bsd_name = |wide: bool| -> (&'static [u8], usize) {
            if wide {
                (b"__.SYMDEF_64", 0)
            } else {
                (b"__.SYMDEF", 3)
            }
        };
        let table_size = |wide: bool| -> u64 {
            if symbols.is_none() {
                return 0;
            }
            if bsd && !(wide && options.dialect == Dialect::Bsd) {
                let w = if wide { 8 } else { 4 };
                let body = w + n * 2 * w + w + names_len;
                let (name, pad) = bsd_name(wide);
                HEADER_LEN as u64 + (name.len() + pad) as u64 + body + (body.wrapping_neg() & 7)
            } else {
                let w = if wide { 8 } else { 4 };
                let body = w + n * w + names_len;
                let pad = if wide && !llvm {
                    body.wrapping_neg() & 7
                } else {
                    body & 1
                };
                HEADER_LEN as u64 + body + pad
            }
        };
        // Each member's header offset, with the table of `wide` entries,
        // and the header size of each (a BSD name with its padding).
        let place = |wide: bool| -> (Vec<u64>, Vec<usize>, u64) {
            let first = MAGIC.len() as u64 + table_size(wide) + long_member;
            let mut at = first;
            let mut offsets = Vec::with_capacity(members.len());
            let mut pads = Vec::with_capacity(members.len());
            for (m, s) in members.iter().zip(&stored) {
                offsets.push(at);
                let mut h = HEADER_LEN as u64;
                if let Name::Bsd = s {
                    // llvm-ar aligns the data to 8 bytes from the first
                    // member (the archive's start too, but past 4 GiB).
                    let end = (at - first) + HEADER_LEN as u64 + m.name.len() as u64;
                    let pad = (end.wrapping_neg() & 7) as usize;
                    pads.push(pad);
                    h += (m.name.len() + pad) as u64;
                } else {
                    pads.push(0);
                }
                at += span(m, h);
            }
            (offsets, pads, at)
        };
        let (mut offsets, mut pads, mut end) = place(false);
        let wide = symbols.is_some()
            && match options.archiver {
                Archiver::GnuAr => symbols.is_some_and(|s| {
                    s.iter()
                        .any(|s| offsets[s.member as usize] > u64::from(u32::MAX))
                }),
                Archiver::LlvmAr => offsets
                    .last()
                    .is_some_and(|&o| o >= options.sym64_threshold),
            };
        if wide {
            (offsets, pads, end) = place(true);
        }
        // (llvm-ar writes a GNU /SYM64/ for a BSD archive past 4 GiB, its
        // members' headers BSD's still.)
        let bsd_table = bsd && !(wide && options.dialect == Dialect::Bsd);
        if bsd_table && !wide && offsets.last().is_some_and(|&o| o > u64::from(u32::MAX)) {
            return Err(WriteError::TooLarge);
        }

        // The head: the signature, the symbol table, the long names.
        let mut head = Vec::with_capacity(
            usize::try_from(MAGIC.len() as u64 + table_size(wide) + long_member).unwrap_or(0),
        );
        head.extend_from_slice(if options.thin { THIN_MAGIC } else { MAGIC });
        if let Some(symbols) = symbols {
            let fields = table_fields(options.table_date);
            let w = if wide { 8 } else { 4 };
            let start = head.len();
            let put = |out: &mut Vec<u8>, v: u64, be: bool| {
                let b = v.to_be_bytes();
                let mut b = if w == 8 { b.to_vec() } else { b[4..].to_vec() };
                if !be {
                    b.reverse();
                }
                out.extend_from_slice(&b);
            };
            if bsd_table {
                let (name, pad) = bsd_name(wide);
                let body = w + n * 2 * w + w + names_len;
                let body_pad = body.wrapping_neg() & 7;
                let mut h = Vec::from(&b"#1/"[..]);
                decimal(&mut h, (name.len() + pad) as u64);
                header(
                    &mut head,
                    &h,
                    &fields,
                    (name.len() + pad) as u64 + body + body_pad,
                );
                head.extend_from_slice(name);
                head.resize(head.len() + pad, 0);
                put(&mut head, n * 2 * w, false);
                let mut strx = 0u64;
                for s in symbols {
                    put(&mut head, strx, false);
                    put(&mut head, offsets[s.member as usize], false);
                    strx += s.name.len() as u64 + 1;
                }
                put(&mut head, names_len, false);
                for s in symbols {
                    head.extend_from_slice(&s.name);
                    head.push(0);
                }
                if filler {
                    head.extend_from_slice(&[0, 0, 0]);
                }
                head.resize(head.len() + body_pad as usize, 0);
            } else {
                let body = w + n * w + names_len;
                let pad = if wide && !llvm {
                    body.wrapping_neg() & 7
                } else {
                    body & 1
                };
                header(
                    &mut head,
                    if wide { b"/SYM64/" } else { b"/" },
                    &fields,
                    body + pad,
                );
                put(&mut head, n, true);
                for s in symbols {
                    put(&mut head, offsets[s.member as usize], true);
                }
                for s in symbols {
                    head.extend_from_slice(&s.name);
                    head.push(0);
                }
                if filler {
                    head.extend_from_slice(&[0, 0, 0]);
                }
                head.resize(head.len() + usize::try_from(pad).unwrap_or(0), 0);
            }
            debug_assert_eq!((head.len() - start) as u64, table_size(wide));
        }
        if !long.is_empty() {
            let size = long.len() as u64 + (long.len() as u64 & 1);
            header(&mut head, b"//", &Fields::BLANK, size);
            head.extend_from_slice(&long);
            if long.len() % 2 == 1 {
                head.push(b'\n');
            }
        }

        // Each member's header.
        let mut headers = Vec::with_capacity(members.len() * HEADER_LEN);
        let mut laid = Vec::with_capacity(members.len());
        for (i, ((m, s), (&offset, &pad))) in members
            .iter()
            .zip(&stored)
            .zip(offsets.iter().zip(&pads))
            .enumerate()
        {
            let start = headers.len();
            let member_pad = if darwin { m.size.wrapping_neg() & 7 } else { 0 };
            let mut size = m.size + member_pad;
            let mut field = Vec::with_capacity(16);
            match *s {
                Name::Short => {
                    field.extend_from_slice(m.name);
                    field.push(b'/');
                }
                Name::Long(at) => {
                    field.push(b'/');
                    decimal(&mut field, at);
                    if let Some(origin) = m.origin {
                        field.push(b':');
                        decimal(&mut field, origin);
                    }
                    if field.len() > 16 {
                        return Err(WriteError::TooLarge);
                    }
                }
                Name::Bsd => {
                    field.extend_from_slice(b"#1/");
                    decimal(&mut field, (m.name.len() + pad) as u64);
                    size += (m.name.len() + pad) as u64;
                }
            }
            if size > 9_999_999_999 {
                return Err(WriteError::MemberTooLarge { member: i });
            }
            header(&mut headers, &field, &m.fields, size);
            if let Name::Bsd = s {
                headers.extend_from_slice(m.name);
                headers.resize(headers.len() + pad, 0);
            }
            let data = if options.thin { 0 } else { m.size };
            let after = if options.thin {
                0
            } else {
                member_pad + ((m.size + member_pad) & 1)
            };
            // (The padding is at most 7 bytes.)
            laid.push((start, offset, data, u8::try_from(after).unwrap_or(0)));
        }
        debug_assert!(laid.is_empty() || laid[0].1 == head.len() as u64);
        Ok(Self {
            head,
            headers,
            members: laid,
            size: end,
            sym64: wide,
        })
    }

    /// The archive's size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Whether the symbol table is the 64-bit one (`/SYM64/` or
    /// `__.SYMDEF_64`).
    pub fn is_sym64(&self) -> bool {
        self.sym64
    }

    /// The bytes before the first member.
    pub fn head(&self) -> &[u8] {
        &self.head
    }

    /// How many members there are.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The offset of member `i`'s header.
    pub fn offset(&self, i: usize) -> u64 {
        self.members[i].1
    }

    /// Member `i`'s header (with its BSD name and the name's padding).
    pub fn header(&self, i: usize) -> &[u8] {
        let start = self.members[i].0;
        let end = self.members.get(i + 1).map_or(self.headers.len(), |m| m.0);
        &self.headers[start..end]
    }

    /// The size of member `i`'s data in the archive (0 when thin).
    pub fn data_size(&self, i: usize) -> u64 {
        self.members[i].2
    }

    /// The bytes member `i` takes: its header, its data, and its padding.
    pub fn span(&self, i: usize) -> u64 {
        let (_, _, data, pad) = self.members[i];
        self.header(i).len() as u64 + data + u64::from(pad)
    }

    /// The padding after member `i`'s data: `\n`s (the Darwin dialect's
    /// up to 8-byte alignment, or one for an odd size).
    fn padding(&self, i: usize) -> usize {
        usize::from(self.members[i].3)
    }

    /// Writes member `i` (`data`, of [`data_size`](Self::data_size)) into
    /// `out`, which is its [`span`](Self::span) long.
    ///
    /// # Panics
    ///
    /// If `data` or `out` is not the size laid out.
    pub fn write_member(&self, i: usize, data: &[u8], out: &mut [u8]) {
        assert_eq!(data.len() as u64, self.data_size(i), "member {i}'s data");
        assert_eq!(out.len() as u64, self.span(i), "member {i}'s span");
        let h = self.header(i);
        out[..h.len()].copy_from_slice(h);
        out[h.len()..h.len() + data.len()].copy_from_slice(data);
        out[h.len() + data.len()..].fill(b'\n');
    }

    /// Splits an output of the archive's [`size`](Self::size) into the head
    /// and each member's span.
    ///
    /// # Panics
    ///
    /// If `out` is not the archive's size.
    pub fn split_mut<'o>(&self, out: &'o mut [u8]) -> (&'o mut [u8], Vec<&'o mut [u8]>) {
        assert_eq!(out.len() as u64, self.size, "the archive's size");
        let (head, mut rest) = out.split_at_mut(self.head.len());
        let mut spans = Vec::with_capacity(self.members.len());
        for i in 0..self.members.len() {
            // (Every span fits: the whole archive does.)
            let (span, after) = rest.split_at_mut(usize::try_from(self.span(i)).unwrap_or(0));
            spans.push(span);
            rest = after;
        }
        (head, spans)
    }

    /// Writes into `out` all but the members' data, and returns the copies
    /// left to make, each at most `piece` bytes, as
    /// [`crate::write::Layout::prepare_mut`] does. `data(i)` is member
    /// `i`'s data (empty when thin).
    ///
    /// # Panics
    ///
    /// If `out` is not the archive's size, or `data(i)` not member `i`'s
    /// [`data_size`](Self::data_size), or `piece` is 0.
    pub fn prepare_mut<'o, 'd>(
        &self,
        out: &'o mut [u8],
        mut data: impl FnMut(usize) -> &'d [u8],
        piece: usize,
    ) -> Vec<(&'o mut [u8], &'d [u8])> {
        let (head, spans) = self.split_mut(out);
        head.copy_from_slice(&self.head);
        let mut copies = Vec::new();
        for (i, span) in spans.into_iter().enumerate() {
            let d = data(i);
            assert_eq!(d.len() as u64, self.data_size(i), "member {i}'s data");
            let h = self.header(i);
            let (header, rest) = span.split_at_mut(h.len());
            header.copy_from_slice(h);
            let (body, pad) = rest.split_at_mut(d.len());
            debug_assert_eq!(pad.len(), self.padding(i));
            pad.fill(b'\n');
            copies.extend(body.chunks_mut(piece).zip(d.chunks(piece)));
        }
        copies
    }

    /// Writes the whole archive into `out` (of its [`size`](Self::size)).
    pub fn write_into<'d>(&self, out: &mut [u8], mut data: impl FnMut(usize) -> &'d [u8]) {
        let (head, spans) = self.split_mut(out);
        head.copy_from_slice(&self.head);
        for (i, span) in spans.into_iter().enumerate() {
            self.write_member(i, data(i), span);
        }
    }
}

/// One member to write, with its data (empty in a thin archive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewMember<'a> {
    /// The member's header.
    pub entry: Entry<'a>,
    /// Its data.
    pub data: &'a [u8],
}

/// Writes an archive of `members`, in this order, with `symbols`.
pub fn write(
    members: &[NewMember<'_>],
    symbols: Option<&[IndexSymbol<'_>]>,
    options: &Options,
) -> Result<Vec<u8>, WriteError> {
    let entries: Vec<Entry<'_>> = members.iter().map(|m| m.entry).collect();
    let layout = Layout::new(&entries, symbols, options)?;
    let size = usize::try_from(layout.size()).map_err(|_| WriteError::TooLarge)?;
    let mut out = vec![0u8; size];
    let thin = options.thin;
    layout.write_into(&mut out, |i| if thin { &[] } else { members[i].data });
    Ok(out)
}

/// A GNU or BSD archive read back into what [`write`] takes: its members
/// (with their header fields), its symbol table, and the options it looks
/// written with. Writing it again with the [`Archiver`] that wrote it
/// gives it back byte for byte (see the `roundtrip_unix` example).
#[derive(Clone, Debug)]
pub struct Contents<'a> {
    /// The members, in archive order (data empty when thin).
    pub members: Vec<NewMember<'a>>,
    /// The symbol table's symbols in its order, if it has one.
    pub symbols: Option<Vec<IndexSymbol<'a>>>,
    /// Its dialect, thinness, the symbol table's date, and (as found) GNU
    /// `ar`'s conventions; [`Contents::write`] takes the archiver.
    pub options: Options,
    /// Whether the symbol table is the 64-bit one.
    pub sym64: bool,
}

/// Why an archive can't be read back for writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// A Microsoft archive (two `/` tables), or not GNU or BSD.
    NotUnix,
    /// A table after the first file, or tables in an order neither tool
    /// writes.
    TableAmongFiles {
        /// Its header's offset.
        offset: u64,
    },
    /// The symbol table doesn't parse, or names a member that isn't there.
    BadTable,
    /// A member whose header fields don't fit [`Fields`].
    BadMember {
        /// Its header's offset.
        offset: u64,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotUnix => f.write_str("not a GNU or BSD archive"),
            ReadError::TableAmongFiles { offset } => {
                write!(f, "a table among the files, at {offset:#x}")
            }
            ReadError::BadTable => f.write_str("the symbol table is unreadable"),
            ReadError::BadMember { offset } => write!(f, "member at {offset:#x} unreadable"),
        }
    }
}

impl core::error::Error for ReadError {}

impl<'a> Contents<'a> {
    /// Reads `archive` back.
    pub fn read(archive: &Archive<'a>) -> Result<Self, ReadError> {
        let all: Vec<Member<'a>> = archive.members().collect();
        let tables = all
            .iter()
            .take_while(|m| m.kind != MemberKind::File)
            .count();
        let (tables, files) = all.split_at(tables);
        if let Some(m) = files.iter().find(|m| m.kind != MemberKind::File) {
            return Err(ReadError::TableAmongFiles {
                offset: m.header_offset,
            });
        }
        let symtabs: Vec<&Member<'a>> = tables
            .iter()
            .filter(|m| m.kind == MemberKind::SymbolTable)
            .collect();
        if symtabs.len() > 1 || tables.iter().any(|m| m.kind == MemberKind::EcSymbolTable) {
            return Err(ReadError::NotUnix);
        }
        let symtab = symtabs.first().copied();
        let bsd = symtab.is_some_and(|t| t.name.starts_with(b"__.SYMDEF"))
            || files.iter().any(|m| {
                usize::try_from(m.header_offset)
                    .ok()
                    .and_then(|o| archive.data().get(o..o + 3))
                    == Some(b"#1/")
            });
        let darwin = bsd && files.iter().all(|m| m.data.len() % 8 == 0);
        let dialect = match (bsd, darwin) {
            (false, _) => Dialect::Gnu,
            (true, false) => Dialect::Bsd,
            (true, true) => Dialect::Darwin,
        };
        let sym64 = symtab.is_some_and(|t| &*t.name == b"/SYM64/" || t.name.ends_with(b"_64"));
        let table_date = match symtab {
            Some(t) => core::str::from_utf8(t.date)
                .ok()
                .and_then(|d| d.parse().ok())
                .ok_or(ReadError::BadTable)?,
            None => 0,
        };
        let symbols = match symtab {
            None => None,
            Some(_) => {
                let offsets: Vec<u64> = files.iter().map(|m| m.header_offset).collect();
                let listed = archive.symbols();
                let mut out = Vec::with_capacity(listed.len());
                for s in listed {
                    let member = offsets
                        .binary_search(&s.member)
                        .map_err(|_| ReadError::BadTable)?;
                    out.push(IndexSymbol {
                        name: Cow::Borrowed(s.name),
                        member: u32::try_from(member).map_err(|_| ReadError::BadTable)?,
                    });
                }
                Some(out)
            }
        };
        let mut members = Vec::with_capacity(files.len());
        for m in files {
            let bad = ReadError::BadMember {
                offset: m.header_offset,
            };
            let Cow::Borrowed(name) = m.name else {
                return Err(bad);
            };
            let size = if archive.is_thin() {
                m.size
            } else {
                m.data.len() as u64
            };
            members.push(NewMember {
                entry: Entry {
                    name,
                    origin: m.origin,
                    fields: Fields::of(m).ok_or(bad)?,
                    size,
                },
                data: m.data,
            });
        }
        let mut options = Options::new(Archiver::GnuAr, dialect);
        if let (Some(t), Some(symbols)) = (symtab, &symbols)
            && bsd
            && symbols.is_empty()
        {
            // The string table's size, after the empty ranlib array.
            options.filler = t.data.get(4..8).is_some_and(|b| b != [0, 0, 0, 0]);
        }
        options.thin = archive.is_thin();
        options.table_date = table_date;
        Ok(Self {
            members,
            symbols,
            options,
            sym64,
        })
    }

    /// The archive written again by `archiver`.
    pub fn write(&self, archiver: Archiver) -> Result<Vec<u8>, WriteError> {
        let mut options = self.options;
        options.archiver = archiver;
        write(&self.members, self.symbols.as_deref(), &options)
    }
}

#[cfg(test)]
mod tests;
