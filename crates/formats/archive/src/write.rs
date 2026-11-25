//! Writing archives in Microsoft's dialect (`.lib`), byte for byte as
//! `lib.exe` and `llvm-lib` write them.
//!
//! An archive is `!<arch>\n`, then, in this order:
//!
//! 1. The **first linker member** (`/`): a big-endian symbol count, each
//!    symbol's member header offset (big-endian, 32-bit), then the
//!    NUL-terminated names. Its order is the writer's: `lib.exe` lists the
//!    members in archive order with each member's symbols sorted, and
//!    `llvm-lib` keeps each member's symbol-table order (see [`Tool`]). An
//!    import library from `lib /DEF` lists the members in the `.def`
//!    file's order instead.
//! 2. The **second linker member** (`/`): a little-endian member count, the
//!    members' header offsets, the symbol count, each symbol's 1-based
//!    member number (16-bit), then the names, sorted by their bytes
//!    (unsigned, a shorter name before any it begins).
//! 3. The **long names** (`//`), if a member's name doesn't fit its header:
//!    each NUL-terminated, in member order, each distinct name once.
//! 4. `/<ECSYMBOLS>/`, in ARM64EC and ARM64X archives: a little-endian
//!    count, 16-bit member numbers, and names, sorted as in the second
//!    linker member. It indexes the members' EC symbols, which the linker
//!    members don't.
//! 5. The members, each a 60-byte header (name, date, owner, group, mode,
//!    and size, as left-aligned text padded with spaces, then `` `\n``) and
//!    its data, then `\n` if the data's length is odd. A name of up to 15
//!    bytes (without `/`) is stored as `name/`; a longer one as `/offset`
//!    into the long names.
//!
//! Each symbol name appears once in the index, for the first member (in
//! the writer's order) that defines it, except that `lib.exe` lets a later
//! member supersede a common symbol. [`Index::build`] drops the rest, as
//! both tools do (`lib.exe` warns LNK4006 of two definitions).
//!
//! The header fields each tool writes (measured with MSVC 14.52's `lib.exe`
//! and LLVM 24's `llvm-lib`) are in [`Style`]. The tables' padding differs
//! too: `lib.exe` gives a table's exact size and pads it with `\n` after,
//! as for any member; `llvm-lib` pads the linker members and
//! `/<ECSYMBOLS>/` to an even size with NULs, and the long names with
//! `\n`, inside the size.
//!
//! **Limits.** Member offsets are 32-bit and member numbers 16-bit:
//!
//! - `lib.exe` refuses a library larger than 0xFFFFFFFF bytes (LNK1248:
//!   "image size (1130003AC) exceeds maximum allowable size (FFFFFFFF)")
//!   and one of more than 65,535 members (LNK1189: "library limit of 65535
//!   objects exceeded").
//! - `llvm-lib` writes the Microsoft dialect over 4 GiB as long as every
//!   member's header offset fits in 32 bits. `link.exe` rejects such a
//!   library (LNK1127, "library is corrupt", for 4.3 GiB of four
//!   members); `lld-link` links against it. When an offset doesn't fit, it
//!   writes a GNU `/SYM64/` table (big-endian 64-bit count and offsets,
//!   then names, in member order) in place of both linker members, which
//!   `link.exe` rejects (LNK4003) and `lld-link` accepts. With more than
//!   65,535 members it writes only the first linker member.
//!
//! [`SizeLimit`] chooses between the first two; the `/SYM64/` and
//! first-member-only forms are not written (they are the GNU dialect's).

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::{Archive, ArchiveSymbol, HEADER_LEN, MAGIC, Member, MemberKind};

/// The most members an index can number (its member numbers are 16-bit).
pub const MAX_MEMBERS: usize = 0xFFFF;

/// The longest name stored in the header itself (with its `/`).
pub const MAX_SHORT_NAME: usize = 15;

/// A member header's date, owner, group, and mode fields: the text each
/// holds, left-aligned and padded with spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fields {
    date: [u8; 12],
    uid: [u8; 6],
    gid: [u8; 6],
    mode: [u8; 8],
}

fn field<const N: usize>(text: &[u8]) -> Option<[u8; N]> {
    if text.len() > N {
        return None;
    }
    let mut f = [b' '; N];
    f[..text.len()].copy_from_slice(text);
    Some(f)
}

/// `n` in decimal (or, with `radix` 8, octal).
fn number(n: u64, radix: u64) -> ([u8; 20], usize) {
    let mut buf = [0u8; 20];
    let mut at = buf.len();
    let mut n = n;
    loop {
        at -= 1;
        // (A digit is less than the radix, at most 10.)
        buf[at] = b'0' + u8::try_from(n % radix).unwrap_or(0);
        n /= radix;
        if n == 0 {
            break;
        }
    }
    let len = buf.len() - at;
    buf.copy_within(at.., 0);
    (buf, len)
}

impl Fields {
    /// Every field blank, as `llvm-lib` writes the long names' header.
    pub const BLANK: Fields = Fields {
        date: [b' '; 12],
        uid: [b' '; 6],
        gid: [b' '; 6],
        mode: [b' '; 8],
    };

    /// The fields holding this text; `None` if one is too long for its
    /// field (12, 6, 6, and 8 bytes).
    pub fn new(date: &[u8], uid: &[u8], gid: &[u8], mode: &[u8]) -> Option<Self> {
        Some(Self {
            date: field(date)?,
            uid: field(uid)?,
            gid: field(gid)?,
            mode: field(mode)?,
        })
    }

    /// A date in seconds since 1970, blank owner and group, and a mode (in
    /// octal), as `lib.exe` writes them.
    pub fn dated(date: u64, mode: u32) -> Self {
        let (d, dl) = number(date, 10);
        let (m, ml) = number(u64::from(mode), 8);
        // (20 digits don't fit a date's 12; such a date is cut.)
        Self::new(&d[..dl.min(12)], b"", b"", &m[..ml.min(8)]).unwrap_or(Self::BLANK)
    }

    /// The fields a member read from an archive has (which the reader gives
    /// trimmed of their padding).
    pub fn of(member: &Member<'_>) -> Option<Self> {
        Self::new(member.date, member.uid, member.gid, member.mode)
    }

    /// The date field's text, without its padding.
    pub fn date(&self) -> &[u8] {
        crate::trim(&self.date)
    }

    /// The owner field's text.
    pub fn uid(&self) -> &[u8] {
        crate::trim(&self.uid)
    }

    /// The group field's text.
    pub fn gid(&self) -> &[u8] {
        crate::trim(&self.gid)
    }

    /// The mode field's text.
    pub fn mode(&self) -> &[u8] {
        crate::trim(&self.mode)
    }
}

/// Which tool's header fields and table padding to write.
///
/// What each writes (`lib.exe` of MSVC 14.52, `llvm-lib` of LLVM 24):
///
/// | Member | `lib.exe` | `lib.exe /Brepro` | `llvm-lib` |
/// |---|---|---|---|
/// | linker members, `/<ECSYMBOLS>/` | now, blank, blank, `0` | `-1`, blank, blank, `0` | `0`, `0`, `0`, `0` |
/// | long names (`//`) | now, blank, blank, `0` | `-1`, blank, blank, `0` | all blank |
/// | an object | its file's time, blank, blank, `100666` (`100444` if read-only) | `-1`, blank, blank, `100666` | `0`, `0`, `0`, `644` |
/// | an import object (from `/DEF`) | now, blank, blank, `0` | `-1`, blank, blank, `0` | `0`, `0`, `0`, `644` |
///
/// "Now" is the time `lib.exe` ran, in seconds since 1970.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// `lib.exe` without `/Brepro`, run at `time`.
    LibExe {
        /// When it ran, in seconds since 1970.
        time: u64,
    },
    /// `lib.exe /Brepro`.
    LibExeBrepro,
    /// `llvm-lib` (always deterministic).
    LlvmLib,
}

impl Style {
    /// The fields of the linker members and `/<ECSYMBOLS>/`.
    pub fn table_fields(self) -> Fields {
        match self {
            Style::LibExe { time } => Fields::dated(time, 0),
            Style::LibExeBrepro => Fields::new(b"-1", b"", b"", b"0").unwrap_or(Fields::BLANK),
            Style::LlvmLib => Fields::new(b"0", b"0", b"0", b"0").unwrap_or(Fields::BLANK),
        }
    }

    /// The fields of the long-names member.
    pub fn long_names_fields(self) -> Fields {
        match self {
            Style::LlvmLib => Fields::BLANK,
            _ => self.table_fields(),
        }
    }

    /// The fields of an object read from a file last written at `mtime`
    /// (seconds since 1970), read-only or not. (`lib.exe /Brepro`'s mode
    /// for a read-only file is likely `100444`, as without it: untested.)
    pub fn object_fields(self, mtime: u64, read_only: bool) -> Fields {
        let mode = if read_only { 0o100_444 } else { 0o100_666 };
        match self {
            Style::LibExe { .. } => Fields::dated(mtime, mode),
            Style::LibExeBrepro => {
                let (m, ml) = number(u64::from(mode), 8);
                Fields::new(b"-1", b"", b"", &m[..ml]).unwrap_or(Fields::BLANK)
            }
            Style::LlvmLib => Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK),
        }
    }

    /// The fields of an import object the tool made (from a `.def` file or
    /// `/EXPORT`), and of the import descriptor objects beside it.
    pub fn import_fields(self) -> Fields {
        match self {
            Style::LlvmLib => Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK),
            _ => self.table_fields(),
        }
    }

    /// The tool's conventions for the tables and the padding.
    pub fn conventions(self) -> Conventions {
        Conventions {
            table_fields: self.table_fields(),
            long_names_fields: self.long_names_fields(),
            tables_padded_inside: self == Style::LlvmLib,
            padding: b'\n',
        }
    }
}

/// How a writer writes what isn't a member's own: the tables' header
/// fields, and the padding.
///
/// [`Style`] gives `lib.exe`'s and `llvm-lib`'s. Microsoft's own import
/// libraries in the Windows SDK (10.0.26100) show a third: every date
/// `4294967295` (-1 as an unsigned 32-bit number), and NUL for padding,
/// both after the tables and after odd-sized members (likely an older
/// `lib.exe` with `/Brepro`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Conventions {
    /// The fields of the linker members and `/<ECSYMBOLS>/`.
    pub table_fields: Fields,
    /// The fields of the long-names member.
    pub long_names_fields: Fields,
    /// Whether the tables' padding is inside their size (`llvm-lib`: NUL
    /// for the linker members and `/<ECSYMBOLS>/`, `\n` for the long
    /// names) rather than after it.
    pub tables_padded_inside: bool,
    /// The byte after an odd-sized member (and table, unless padded
    /// inside).
    pub padding: u8,
}

impl From<Style> for Conventions {
    fn from(style: Style) -> Self {
        style.conventions()
    }
}

/// How large an archive may be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SizeLimit {
    /// No larger than 0xFFFFFFFF bytes, as `lib.exe` (LNK1248) and so as
    /// `link.exe` reads.
    #[default]
    FourGiB,
    /// Any size, as long as every member's header offset fits in 32 bits,
    /// as `llvm-lib` writes. Only `lld-link` reads past 4 GiB.
    Offsets,
}

/// One member to write: its name, its header's fields, and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberHeader<'a> {
    /// The name, as stored (`lib.exe` stores the path it was given, with
    /// `\` for `/`), without the trailing `/`.
    pub name: &'a [u8],
    /// The date, owner, group, and mode fields.
    pub fields: Fields,
    /// The data's size.
    pub size: u64,
}

/// One symbol of the index, and the member that defines it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IndexSymbol<'a> {
    /// The symbol's name.
    pub name: Cow<'a, [u8]>,
    /// Its member's position among the archive's members, from 0.
    pub member: u32,
}

/// A symbol a member defines, for the index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemberSymbol<'a> {
    /// Its name.
    pub name: Cow<'a, [u8]>,
    /// Whether it is a common symbol (external and undefined, with its
    /// size as its value), which a later member supersedes in `lib.exe`'s
    /// index.
    pub common: bool,
}

impl<'a> MemberSymbol<'a> {
    /// A symbol the member defines (not a common one).
    pub fn defined(name: impl Into<Cow<'a, [u8]>>) -> Self {
        Self {
            name: name.into(),
            common: false,
        }
    }
}

/// The symbols a member defines, for the index (see [`crate::symbols`]
/// for the ones `lib.exe` and `llvm-lib` list).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberSymbols<'a> {
    /// The symbols the linker members list, in the member's own order (its
    /// symbol table's).
    pub symbols: Vec<MemberSymbol<'a>>,
    /// The symbols `/<ECSYMBOLS>/` lists.
    pub ec_symbols: Vec<MemberSymbol<'a>>,
}

/// Whose way of building the index to follow.
///
/// | | `lib.exe` | `llvm-lib` |
/// |---|---|---|
/// | each member's symbols in the first linker member | sorted by name | in its symbol table's order |
/// | a name more than one member defines | the first member's, but that a common symbol yields to any later member's (so of commons only, the last) | the first member's |
///
/// (Measured on objects defining, and holding as common, the same names
/// in different orders: a common of 8 bytes then one of 16, 16 then 8, a
/// common before a definition and after one. Measured for the linker
/// members; `/<ECSYMBOLS>/` likely the same, untested.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// `lib.exe`.
    LibExe,
    /// `llvm-lib`.
    LlvmLib,
}

/// What the archive's index holds: the first linker member's symbols in
/// its order (the second linker member is the same symbols sorted), and
/// `/<ECSYMBOLS>/`'s, if the archive has one.
///
/// The second linker member lists the members, and its symbols (and
/// `/<ECSYMBOLS>/`'s) name them by their place in that list. The list is
/// in archive order, but for an import library from `lib /DEF`, which
/// lists them in the `.def` file's order (as its first linker member
/// does), while the archive holds them sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index<'a> {
    /// The first linker member's symbols, in its order.
    pub symbols: Vec<IndexSymbol<'a>>,
    /// `/<ECSYMBOLS>/`'s symbols (in any order: they are written sorted),
    /// or `None` for an archive without one.
    pub ec_symbols: Option<Vec<IndexSymbol<'a>>>,
    /// The second linker member's list of members, by their positions in
    /// the archive, if not every member in archive order. A `None` entry
    /// is listed with offset 0, as in some of Microsoft's import libraries
    /// (`link.exe`'s, likely for exports given no member).
    pub member_list: Option<Vec<Option<u32>>>,
}

/// The positions of `names` sorted by name, ties in their order.
fn sorted_order<'n>(names: impl Iterator<Item = &'n [u8]>) -> Vec<u32> {
    let names: Vec<&[u8]> = names.collect();
    // (Positions are below 2^32: a symbol table's count is 32-bit.)
    let mut order: Vec<u32> = (0..u32::try_from(names.len()).unwrap_or(u32::MAX)).collect();
    order.sort_unstable_by(|&a, &b| {
        names[a as usize]
            .cmp(names[b as usize])
            .then_with(|| a.cmp(&b))
    });
    order
}

/// `symbols` (each with its member) with each name once: the first's, or
/// with `lib.exe`, the first not superseded (a common symbol is, by any
/// later one).
fn one_of_each<'a>(symbols: Vec<(MemberSymbol<'a>, u32)>, tool: Tool) -> Vec<IndexSymbol<'a>> {
    let order = sorted_order(symbols.iter().map(|(s, _)| &*s.name));
    let mut keep = vec![false; symbols.len()];
    let mut winner: Option<usize> = None;
    for &i in &order {
        let i = i as usize;
        match winner {
            Some(w) if symbols[w].0.name == symbols[i].0.name => {
                if tool == Tool::LibExe && symbols[w].0.common {
                    keep[w] = false;
                    keep[i] = true;
                    winner = Some(i);
                }
            }
            _ => {
                keep[i] = true;
                winner = Some(i);
            }
        }
    }
    symbols
        .into_iter()
        .zip(keep)
        .filter_map(|((s, member), k)| {
            k.then_some(IndexSymbol {
                name: s.name,
                member,
            })
        })
        .collect()
}

impl<'a> Index<'a> {
    /// The index of every member, given in the first linker member's order
    /// (archive order, but for `lib /DEF`'s import libraries), each with
    /// its position in the archive and its symbols, built as `tool` builds
    /// it. The members' order is also the second linker member's list of
    /// them. `ec` gives the archive a `/<ECSYMBOLS>/` (ARM64EC and ARM64X
    /// archives).
    pub fn build(
        members: impl IntoIterator<Item = (u32, MemberSymbols<'a>)>,
        tool: Tool,
        ec: bool,
    ) -> Self {
        let mut symbols = Vec::new();
        let mut ec_symbols = Vec::new();
        let mut list = Vec::new();
        for (member, mut s) in members {
            list.push(member);
            if tool == Tool::LibExe {
                s.symbols.sort_by(|a, b| a.name.cmp(&b.name));
            }
            symbols.extend(s.symbols.into_iter().map(|s| (s, member)));
            ec_symbols.extend(s.ec_symbols.into_iter().map(|s| (s, member)));
        }
        let in_order = list.iter().enumerate().all(|(i, &m)| m as usize == i);
        Self {
            symbols: one_of_each(symbols, tool),
            ec_symbols: ec.then(|| one_of_each(ec_symbols, tool)),
            member_list: (!in_order).then(|| list.into_iter().map(Some).collect()),
        }
    }
}

/// Why an archive can't be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The archive would be larger than 0xFFFFFFFF bytes (`lib.exe`'s
    /// LNK1248), with [`SizeLimit::FourGiB`].
    TooLarge {
        /// Its size.
        size: u64,
    },
    /// A member's header would be at an offset that doesn't fit in 32
    /// bits.
    OffsetTooLarge {
        /// The member's position.
        member: usize,
        /// Its header's offset.
        offset: u64,
    },
    /// More than [`MAX_MEMBERS`] members (`lib.exe`'s LNK1189).
    TooManyMembers {
        /// How many.
        count: usize,
    },
    /// A member's size doesn't fit its header's ten digits.
    MemberTooLarge {
        /// The member's position.
        member: usize,
    },
    /// A member's name holds a NUL, which ends a long name.
    BadName {
        /// The member's position.
        member: usize,
    },
    /// A symbol's name holds a NUL, which ends it in the index.
    BadSymbol {
        /// Its position in the index's list.
        symbol: usize,
    },
    /// A symbol names a member that isn't there, or isn't in the member
    /// list.
    NoSuchMember {
        /// Its position in the index's list (the EC symbols after the
        /// others).
        symbol: usize,
    },
    /// The member list names a member that isn't there.
    NoSuchListedMember {
        /// Its position in the list.
        position: usize,
    },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::TooLarge { size } => write!(
                f,
                "the archive would be {size:#X} bytes, larger than 0xFFFFFFFF"
            ),
            WriteError::OffsetTooLarge { member, offset } => write!(
                f,
                "member {member} would be at offset {offset:#X}, past 32 bits"
            ),
            WriteError::TooManyMembers { count } => {
                write!(f, "{count} members, more than {MAX_MEMBERS}")
            }
            WriteError::MemberTooLarge { member } => {
                write!(f, "member {member} is too large for its header")
            }
            WriteError::BadName { member } => write!(f, "member {member}'s name holds a NUL"),
            WriteError::BadSymbol { symbol } => write!(f, "symbol {symbol}'s name holds a NUL"),
            WriteError::NoSuchMember { symbol } => {
                write!(f, "symbol {symbol} names a member that isn't there")
            }
            WriteError::NoSuchListedMember { position } => {
                write!(f, "member list entry {position} isn't a member")
            }
        }
    }
}

impl core::error::Error for WriteError {}

/// A count, for a table (whose counts are 32-bit).
fn u32_len(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A member's name field: its name, or its offset in the long names.
#[derive(Clone, Copy, Debug)]
enum NameField {
    Short,
    Long(u32),
}

/// Where everything in an archive goes, worked out before any of it is
/// written: the bytes before the first member (the signature and the
/// tables), and each member's header and offset.
///
/// The members can then be written in any order, or in parallel, into
/// their own parts of the output ([`Layout::split_mut`]), or their data
/// copied in even pieces ([`Layout::prepare_mut`]: 4.6 to 5.1 GiB/s on 8
/// threads, into memory, for LLVM's four largest libraries, 5.4 GiB, read
/// from the page cache).
#[derive(Clone, Debug)]
pub struct Layout<'a> {
    head: Vec<u8>,
    members: Vec<(MemberHeader<'a>, NameField, u64)>,
    size: u64,
    padding: u8,
}

/// A table's size with its padding inside (`llvm-lib`) or not.
fn padded(len: usize, inside: bool) -> usize {
    if inside { len + (len & 1) } else { len }
}

fn header(name: &[u8], fields: &Fields, size: u64) -> [u8; HEADER_LEN] {
    let mut h = [b' '; HEADER_LEN];
    h[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    h[16..28].copy_from_slice(&fields.date);
    h[28..34].copy_from_slice(&fields.uid);
    h[34..40].copy_from_slice(&fields.gid);
    h[40..48].copy_from_slice(&fields.mode);
    let (s, sl) = number(size, 10);
    h[48..48 + sl.min(10)].copy_from_slice(&s[..sl.min(10)]);
    h[58] = b'`';
    h[59] = b'\n';
    h
}

/// How a table of odd size is padded.
#[derive(Clone, Copy)]
enum Padding {
    /// With this byte, inside its size.
    Inside(u8),
    /// With this byte, after it.
    After(u8),
}

/// Appends a table member: its header, `body`, and its padding.
fn push_table(out: &mut Vec<u8>, name: &[u8], fields: &Fields, body: &[u8], pad: Padding) {
    let odd = body.len() % 2 == 1;
    let size = body.len() + usize::from(odd && matches!(pad, Padding::Inside(_)));
    out.extend_from_slice(&header(name, fields, size as u64));
    out.extend_from_slice(body);
    if odd {
        out.push(match pad {
            Padding::Inside(b) | Padding::After(b) => b,
        });
    }
}

impl<'a> Layout<'a> {
    /// Lays out an archive of `members`, in this order, with `index`.
    pub fn new(
        members: &[MemberHeader<'a>],
        index: &Index<'_>,
        conventions: impl Into<Conventions>,
        limit: SizeLimit,
    ) -> Result<Self, WriteError> {
        let conventions = conventions.into();
        if members.len() > MAX_MEMBERS {
            return Err(WriteError::TooManyMembers {
                count: members.len(),
            });
        }
        let inside = conventions.tables_padded_inside;
        let check = |symbols: &[IndexSymbol<'_>], base: usize| -> Result<usize, WriteError> {
            let mut names = 0usize;
            for (i, s) in symbols.iter().enumerate() {
                if s.member as usize >= members.len() {
                    return Err(WriteError::NoSuchMember { symbol: base + i });
                }
                if s.name.contains(&0) {
                    return Err(WriteError::BadSymbol { symbol: base + i });
                }
                names += s.name.len() + 1;
            }
            Ok(names)
        };
        let list: Vec<Option<u32>> = match &index.member_list {
            Some(list) => {
                if let Some(i) = list
                    .iter()
                    .position(|m| m.is_some_and(|m| m as usize >= members.len()))
                {
                    return Err(WriteError::NoSuchListedMember { position: i });
                }
                if list.len() > MAX_MEMBERS {
                    return Err(WriteError::TooManyMembers { count: list.len() });
                }
                list.clone()
            }
            None => (0..u32_len(members.len())).map(Some).collect(),
        };
        // Each member's number in the list (from 1; 0 if unlisted).
        let mut numbers = vec![0u16; members.len()];
        for (i, m) in list.iter().enumerate().rev() {
            if let Some(m) = m {
                // (At most 65,535 entries.)
                numbers[*m as usize] = u16::try_from(i + 1).unwrap_or(u16::MAX);
            }
        }
        let names = check(&index.symbols, 0)?;
        let ec_names = match &index.ec_symbols {
            Some(ec) => Some(check(ec, index.symbols.len())?),
            None => None,
        };
        let unlisted = |symbols: &[IndexSymbol<'_>], base: usize| {
            symbols
                .iter()
                .position(|s| numbers[s.member as usize] == 0)
                .map(|i| WriteError::NoSuchMember { symbol: base + i })
        };
        if let Some(e) = unlisted(&index.symbols, 0) {
            return Err(e);
        }
        if let Some(e) = index
            .ec_symbols
            .as_ref()
            .and_then(|ec| unlisted(ec, index.symbols.len()))
        {
            return Err(e);
        }

        // Long names: each distinct one once, in member order.
        let mut long_names: Vec<u8> = Vec::new();
        let mut seen: BTreeMap<&[u8], u32> = BTreeMap::new();
        let mut fields = Vec::with_capacity(members.len());
        for (i, m) in members.iter().enumerate() {
            if m.size > 9_999_999_999 {
                return Err(WriteError::MemberTooLarge { member: i });
            }
            if m.name.len() <= MAX_SHORT_NAME && !m.name.contains(&b'/') {
                fields.push(NameField::Short);
                continue;
            }
            if m.name.contains(&0) {
                return Err(WriteError::BadName { member: i });
            }
            let at = *seen.entry(m.name).or_insert_with(|| {
                let at = long_names.len();
                long_names.extend_from_slice(m.name);
                long_names.push(0);
                // (Below 65,535 names of at most a member's size each.)
                u32::try_from(at).unwrap_or(u32::MAX)
            });
            fields.push(NameField::Long(at));
        }

        let n = index.symbols.len();
        let first_len = 4 + 4 * n + names;
        let second_len = 4 + 4 * list.len() + 4 + 2 * n + names;
        let table = |len: usize| HEADER_LEN + padded(len, inside) + (padded(len, inside) & 1);
        let mut head_len = MAGIC.len() + table(first_len) + table(second_len);
        if !long_names.is_empty() {
            head_len += table(long_names.len());
        }
        let ec_len = index
            .ec_symbols
            .as_ref()
            .zip(ec_names)
            .map(|(ec, names)| 4 + 2 * ec.len() + names);
        if let Some(len) = ec_len {
            head_len += table(len);
        }

        // Each member's header offset.
        let mut at = head_len as u64;
        let mut laid = Vec::with_capacity(members.len());
        for (i, (m, f)) in members.iter().zip(fields).enumerate() {
            if limit == SizeLimit::Offsets && at > u64::from(u32::MAX) {
                return Err(WriteError::OffsetTooLarge {
                    member: i,
                    offset: at,
                });
            }
            laid.push((*m, f, at));
            at += HEADER_LEN as u64 + m.size + (m.size & 1);
        }
        if limit == SizeLimit::FourGiB && at > u64::from(u32::MAX) {
            return Err(WriteError::TooLarge { size: at });
        }
        // (Every offset now fits in 32 bits.)
        let offset = |member: u32| u32::try_from(laid[member as usize].2).unwrap_or(u32::MAX);

        let mut head = Vec::with_capacity(head_len);
        head.extend_from_slice(MAGIC);
        let fields = conventions.table_fields;
        let pad = if inside {
            Padding::Inside(0)
        } else {
            Padding::After(conventions.padding)
        };
        // The first linker member.
        let mut body = Vec::with_capacity(first_len);
        body.extend_from_slice(&u32_len(n).to_be_bytes());
        for s in &index.symbols {
            body.extend_from_slice(&offset(s.member).to_be_bytes());
        }
        for s in &index.symbols {
            body.extend_from_slice(&s.name);
            body.push(0);
        }
        push_table(&mut head, b"/", &fields, &body, pad);
        // The second, sorted.
        body.clear();
        body.extend_from_slice(&u32_len(list.len()).to_le_bytes());
        for m in &list {
            body.extend_from_slice(&m.map_or(0, offset).to_le_bytes());
        }
        body.extend_from_slice(&u32_len(n).to_le_bytes());
        let order = sorted_order(index.symbols.iter().map(|s| &*s.name));
        for &i in &order {
            let number = numbers[index.symbols[i as usize].member as usize];
            body.extend_from_slice(&number.to_le_bytes());
        }
        for &i in &order {
            body.extend_from_slice(&index.symbols[i as usize].name);
            body.push(0);
        }
        push_table(&mut head, b"/", &fields, &body, pad);
        if !long_names.is_empty() {
            let pad = if inside { Padding::Inside(b'\n') } else { pad };
            push_table(
                &mut head,
                b"//",
                &conventions.long_names_fields,
                &long_names,
                pad,
            );
        }
        if let Some(ec) = &index.ec_symbols {
            body.clear();
            body.extend_from_slice(&u32_len(ec.len()).to_le_bytes());
            let order = sorted_order(ec.iter().map(|s| &*s.name));
            for &i in &order {
                let number = numbers[ec[i as usize].member as usize];
                body.extend_from_slice(&number.to_le_bytes());
            }
            for &i in &order {
                body.extend_from_slice(&ec[i as usize].name);
                body.push(0);
            }
            push_table(&mut head, b"/<ECSYMBOLS>/", &fields, &body, pad);
        }
        debug_assert_eq!(head.len(), head_len);
        Ok(Self {
            head,
            members: laid,
            size: at,
            padding: conventions.padding,
        })
    }

    /// The archive's size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The bytes before the first member: the signature and the tables.
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
        self.members[i].2
    }

    /// The bytes member `i` takes: its header, its data, and its padding.
    pub fn span(&self, i: usize) -> u64 {
        let size = self.members[i].0.size;
        HEADER_LEN as u64 + size + (size & 1)
    }

    /// Member `i`'s header.
    pub fn header(&self, i: usize) -> [u8; HEADER_LEN] {
        let (m, f, _) = &self.members[i];
        match f {
            NameField::Short => {
                let mut name = [b' '; 16];
                name[..m.name.len()].copy_from_slice(m.name);
                name[m.name.len()] = b'/';
                header(&name, &m.fields, m.size)
            }
            NameField::Long(at) => {
                let (d, dl) = number(u64::from(*at), 10);
                let mut name = [b' '; 16];
                name[0] = b'/';
                name[1..=dl].copy_from_slice(&d[..dl]);
                header(&name, &m.fields, m.size)
            }
        }
    }

    /// Writes member `i` (`data`, of the size it was laid out with) into
    /// `out`, which is its [`span`](Self::span) long.
    ///
    /// # Panics
    ///
    /// If `data` or `out` is not the size laid out.
    pub fn write_member(&self, i: usize, data: &[u8], out: &mut [u8]) {
        let size = self.members[i].0.size;
        assert_eq!(data.len() as u64, size, "member {i}'s data");
        assert_eq!(out.len() as u64, self.span(i), "member {i}'s span");
        out[..HEADER_LEN].copy_from_slice(&self.header(i));
        out[HEADER_LEN..HEADER_LEN + data.len()].copy_from_slice(data);
        if size & 1 == 1 {
            out[HEADER_LEN + data.len()] = self.padding;
        }
    }

    /// Splits an output of the archive's [`size`](Self::size) into the head
    /// and each member's span, to write separately (or in parallel).
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

    /// Writes into `out` (of the archive's [`size`](Self::size)) all but
    /// the members' data: the head, each header, and each padding byte.
    /// Returns the copies left to make, each a part of `out` and the data
    /// that goes there, of at most `piece` bytes (each member's data, from
    /// `data(i)`, cut into pieces), for the caller to make in any order or
    /// on any number of threads: even shares however uneven the members.
    ///
    /// # Panics
    ///
    /// If `out` is not the archive's size, or `data(i)` not member `i`'s,
    /// or `piece` is 0.
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
            assert_eq!(d.len() as u64, self.members[i].0.size, "member {i}'s data");
            let (header, rest) = span.split_at_mut(HEADER_LEN);
            header.copy_from_slice(&self.header(i));
            let (body, pad) = rest.split_at_mut(d.len());
            pad.fill(self.padding);
            copies.extend(body.chunks_mut(piece).zip(d.chunks(piece)));
        }
        copies
    }

    /// Writes the whole archive into `out` (of its [`size`](Self::size)),
    /// with `data(i)` giving member `i`'s data.
    pub fn write_into<'d>(&self, out: &mut [u8], mut data: impl FnMut(usize) -> &'d [u8]) {
        let (head, spans) = self.split_mut(out);
        head.copy_from_slice(&self.head);
        for (i, span) in spans.into_iter().enumerate() {
            self.write_member(i, data(i), span);
        }
    }
}

/// A Microsoft archive read back into what [`write`] takes: its members
/// (with their header fields), its index, and its conventions. Writing
/// them again gives the archive back, byte for byte (as for every library
/// of the corpora tried: see the `roundtrip` example).
#[derive(Clone, Debug)]
pub struct Contents<'a> {
    /// The members, in archive order.
    pub members: Vec<NewMember<'a>>,
    /// The index, as the tables hold it.
    pub index: Index<'a>,
    /// The tables' header fields and the padding.
    pub conventions: Conventions,
    /// The tool whose tables these look like, if one's.
    pub style: Option<Style>,
}

/// Why an archive can't be read back for writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// A thin archive, or one without both linker members (GNU, BSD).
    NotMicrosoft,
    /// A table after the first file.
    TableAmongFiles {
        /// Its header's offset.
        offset: u64,
    },
    /// A table that doesn't parse, or names a member that isn't there.
    BadTable(&'static str),
    /// A member whose header fields don't fit [`Fields`] (or whose name
    /// isn't stored in the archive).
    BadMember {
        /// Its header's offset.
        offset: u64,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotMicrosoft => f.write_str("not a Microsoft archive"),
            ReadError::TableAmongFiles { offset } => {
                write!(f, "a table among the files, at {offset:#x}")
            }
            ReadError::BadTable(what) => write!(f, "{what} unreadable"),
            ReadError::BadMember { offset } => write!(f, "member at {offset:#x} unreadable"),
        }
    }
}

impl core::error::Error for ReadError {}

/// The size of the first linker member's contents, without padding.
fn first_member_len(data: &[u8]) -> Option<usize> {
    let n = usize::try_from(u32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()?;
    names_end(data, 4usize.checked_add(n.checked_mul(4)?)?, n)
}

/// The position after `n` NUL-terminated names from `at`.
fn names_end(data: &[u8], mut at: usize, n: usize) -> Option<usize> {
    for _ in 0..n {
        at += turbonop_core::find_nul(data.get(at..)?)? + 1;
    }
    Some(at)
}

/// The size of the second linker member's contents, without padding.
fn second_member_len(data: &[u8]) -> Option<usize> {
    let le = |at: usize| -> Option<usize> {
        usize::try_from(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?)).ok()
    };
    let m = le(0)?;
    let at = 4usize.checked_add(m.checked_mul(4)?)?;
    let n = le(at)?;
    names_end(data, at.checked_add(4)?.checked_add(n.checked_mul(2)?)?, n)
}

/// The size of `/<ECSYMBOLS>/`'s contents, without padding.
fn ec_member_len(data: &[u8]) -> Option<usize> {
    let n = usize::try_from(u32::from_le_bytes(data.get(..4)?.try_into().ok()?)).ok()?;
    names_end(data, 4usize.checked_add(n.checked_mul(2)?)?, n)
}

impl<'a> Contents<'a> {
    /// Reads `archive` back.
    pub fn read(archive: &Archive<'a>) -> Result<Self, ReadError> {
        if archive.is_thin() {
            return Err(ReadError::NotMicrosoft);
        }
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
        let linker: Vec<&Member<'a>> = tables.iter().filter(|m| &*m.name == b"/").collect();
        let [first, _second] = linker[..] else {
            return Err(ReadError::NotMicrosoft);
        };

        // The conventions, from the tables themselves.
        let table_fields = Fields::of(first).ok_or(ReadError::BadTable("a linker member"))?;
        let style = match (first.date, first.uid) {
            (b"-1", b"") => Some(Style::LibExeBrepro),
            (b"0", b"0") => Some(Style::LlvmLib),
            (date, b"") if !date.is_empty() && date.iter().all(u8::is_ascii_digit) => {
                core::str::from_utf8(date)
                    .ok()
                    .and_then(|d| d.parse().ok())
                    .map(|time| Style::LibExe { time })
            }
            _ => None,
        };
        // Padded inside if a table's contents are shorter than its size.
        // (If none is of odd size, either way writes the same bytes.)
        let inside = tables.iter().any(|t| {
            let natural = match t.kind {
                MemberKind::SymbolTable if core::ptr::eq(t, first) => first_member_len(t.data),
                MemberKind::SymbolTable => second_member_len(t.data),
                MemberKind::EcSymbolTable => ec_member_len(t.data),
                _ => t
                    .data
                    .ends_with(b"\0\n")
                    .then(|| t.data.len() - 1)
                    .or(Some(t.data.len())),
            };
            natural.is_some_and(|n| n < t.data.len())
        });
        let mut conventions = Conventions {
            table_fields,
            long_names_fields: table_fields,
            tables_padded_inside: inside,
            padding: b'\n',
        };
        if let Some(names) = tables.iter().find(|m| m.kind == MemberKind::LongNames) {
            conventions.long_names_fields =
                Fields::of(names).ok_or(ReadError::BadTable("the long names"))?;
        } else if let Some(style) = style {
            conventions.long_names_fields = style.long_names_fields();
        }
        let padded_after = |m: &&Member<'_>| {
            m.data.len() % 2 == 1
                && !(conventions.tables_padded_inside && m.kind != MemberKind::File)
        };
        let data = archive.data();
        if let Some(m) = all.iter().find(padded_after)
            && let Some(&b) = usize::try_from(m.data_offset)
                .ok()
                .and_then(|at| data.get(at + m.data.len()))
        {
            conventions.padding = b;
        }

        // The index, by member positions (the files' offsets ascend).
        let offsets: Vec<u64> = files.iter().map(|m| m.header_offset).collect();
        let number = |offset: u64| -> Option<u32> {
            offsets
                .binary_search(&offset)
                .ok()
                .and_then(|i| u32::try_from(i).ok())
        };
        let to_index = |symbols: Vec<ArchiveSymbol<'a>>, what| {
            symbols
                .into_iter()
                .map(|s| {
                    Some(IndexSymbol {
                        name: Cow::Borrowed(s.name),
                        member: number(s.member)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(ReadError::BadTable(what))
        };
        let first_symbols = archive
            .first_linker_member()
            .ok_or(ReadError::BadTable("the first linker member"))?;
        let symbols = to_index(first_symbols, "the first linker member")?;
        let ec_symbols = match archive.ec_symbols() {
            Some(ec) => Some(to_index(ec, "/<ECSYMBOLS>/")?),
            None if tables.iter().any(|m| m.kind == MemberKind::EcSymbolTable) => {
                return Err(ReadError::BadTable("/<ECSYMBOLS>/"));
            }
            None => None,
        };
        let (list, _) = archive
            .second_linker_member()
            .ok_or(ReadError::BadTable("the second linker member"))?;
        // (Offset 0 is a hole in the list.)
        let list = list
            .iter()
            .map(|&o| {
                if o == 0 {
                    Some(None)
                } else {
                    number(o).map(Some)
                }
            })
            .collect::<Option<Vec<Option<u32>>>>()
            .ok_or(ReadError::BadTable("the second linker member"))?;
        let in_order = list.len() == files.len()
            && list
                .iter()
                .enumerate()
                .all(|(i, &m)| m.is_some_and(|m| m as usize == i));
        let index = Index {
            symbols,
            ec_symbols,
            member_list: (!in_order).then_some(list),
        };

        let mut members = Vec::with_capacity(files.len());
        for m in files {
            let bad = ReadError::BadMember {
                offset: m.header_offset,
            };
            let Cow::Borrowed(name) = m.name else {
                return Err(bad);
            };
            members.push(NewMember {
                name,
                fields: Fields::of(m).ok_or(bad)?,
                data: m.data,
            });
        }
        Ok(Self {
            members,
            index,
            conventions,
            style,
        })
    }

    /// The archive's layout, to write it again.
    pub fn layout(&self, limit: SizeLimit) -> Result<Layout<'a>, WriteError> {
        let headers: Vec<MemberHeader<'a>> = self
            .members
            .iter()
            .map(|m| MemberHeader {
                name: m.name,
                fields: m.fields,
                size: m.data.len() as u64,
            })
            .collect();
        Layout::new(&headers, &self.index, self.conventions, limit)
    }

    /// The archive, written again.
    pub fn write(&self, limit: SizeLimit) -> Result<Vec<u8>, WriteError> {
        write(&self.members, &self.index, self.conventions, limit)
    }
}

/// One member to write, with its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewMember<'a> {
    /// The name, as stored, without the trailing `/`.
    pub name: &'a [u8],
    /// The date, owner, group, and mode fields.
    pub fields: Fields,
    /// The member's data.
    pub data: &'a [u8],
}

/// Writes an archive of `members`, in this order, with `index`.
pub fn write(
    members: &[NewMember<'_>],
    index: &Index<'_>,
    conventions: impl Into<Conventions>,
    limit: SizeLimit,
) -> Result<Vec<u8>, WriteError> {
    let headers: Vec<MemberHeader<'_>> = members
        .iter()
        .map(|m| MemberHeader {
            name: m.name,
            fields: m.fields,
            size: m.data.len() as u64,
        })
        .collect();
    let layout = Layout::new(&headers, index, conventions, limit)?;
    let size = usize::try_from(layout.size()).map_err(|_| WriteError::TooLarge {
        size: layout.size(),
    })?;
    let mut out = vec![0u8; size];
    layout.write_into(&mut out, |i| members[i].data);
    Ok(out)
}

#[cfg(test)]
mod tests;
