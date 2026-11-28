//! The bitstream container: a stream of bits (read from 32-bit
//! little-endian words, least significant bit first) holding nested
//! blocks of records, each record either unabbreviated (VBR6 fields) or
//! read through an abbreviation the block (or `BLOCKINFO`) defines.
//!
//! This is the format of every LLVM bitstream, not only IR: clang's
//! serialized diagnostics (`DIAG`), optimization remarks (`RMRK`), and
//! precompiled headers (`CPCH`) use it too. [`Cursor`] walks any of them
//! the way LLVM's `BitstreamCursor` does: [`Cursor::advance`] gives the
//! next entry (a record, a sub-block, or the end of the block),
//! [`Cursor::enter_block`] or [`Cursor::skip_block`] takes a sub-block,
//! and [`Cursor::read_record`] reads a record.
//!
//! Every read is bounds-checked; a stream that ends early or holds what
//! the format doesn't allow gives an error, never a panic.

use alloc::sync::Arc;
use alloc::vec::Vec;

use turbonop_core::{Error, ErrorKind, Result};

use crate::tables::{
    BLOCKINFO_BLOCK_ID, BLOCKINFO_CODE_BLOCKNAME, BLOCKINFO_CODE_SETBID,
    BLOCKINFO_CODE_SETRECORDNAME, DEFINE_ABBREV, END_BLOCK, ENTER_SUBBLOCK,
    FIRST_APPLICATION_ABBREV, UNABBREV_RECORD,
};

/// The widest fixed or VBR field an abbreviation may have (LLVM's
/// `MaxChunkSize`).
pub const MAX_CHUNK: u32 = 32;

/// The magic of the bitcode wrapper (Darwin's), little-endian.
pub const WRAPPER_MAGIC: u32 = 0x0B17_C0DE;

/// `usize` of a value known to fit (a position within the data).
pub(crate) fn idx(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// The low 32 bits of `v`.
pub(crate) fn low32(v: u64) -> u32 {
    u32::try_from(v & 0xFFFF_FFFF).unwrap_or(u32::MAX)
}

fn truncated(bit: u64, what: &'static str) -> Error {
    Error::new(ErrorKind::Truncated, bit / 8, what)
}

fn malformed(bit: u64, what: &'static str) -> Error {
    Error::new(ErrorKind::Malformed, bit / 8, what)
}

/// A reader of bits from bytes: 32-bit little-endian words, least
/// significant bit first.
#[derive(Clone, Copy, Debug)]
pub struct BitReader<'d> {
    data: &'d [u8],
    pos: u64,
}

impl<'d> BitReader<'d> {
    /// A reader at the start of `data`.
    pub fn new(data: &'d [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The bytes read.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// The position, in bits from the start.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// The length, in bits.
    pub fn len_bits(&self) -> u64 {
        self.data.len() as u64 * 8
    }

    /// Whether every bit has been read.
    pub fn at_end(&self) -> bool {
        self.pos >= self.len_bits()
    }

    /// Moves to bit `bit` (at most the end).
    pub fn seek(&mut self, bit: u64) -> Result<()> {
        if bit > self.len_bits() {
            return Err(truncated(self.pos, "bitstream position"));
        }
        self.pos = bit;
        Ok(())
    }

    /// Reads an `n`-bit field (`n` at most 64; 0 reads nothing and gives 0).
    #[inline]
    pub fn read(&mut self, n: u32) -> Result<u64> {
        if n == 0 {
            return Ok(0);
        }
        if n > 64 {
            return Err(malformed(self.pos, "bitstream field width"));
        }
        let end = self.pos + u64::from(n);
        if end > self.len_bits() {
            return Err(truncated(self.pos, "bitstream field"));
        }
        let byte = idx(self.pos >> 3);
        let shift = (self.pos & 7) as u32;
        let rest = self.data.get(byte..).unwrap_or(&[]);
        let mut buf = [0u8; 9];
        let take = rest.len().min(9);
        buf[..take].copy_from_slice(&rest[..take]);
        let lo = u64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ]);
        let mut v = lo >> shift;
        if shift != 0 && n + shift > 64 {
            v |= u64::from(buf[8]) << (64 - shift);
        }
        if n < 64 {
            v &= (1u64 << n) - 1;
        }
        self.pos = end;
        Ok(v)
    }

    /// Reads a variable-width integer of `n`-bit chunks (as LLVM's
    /// `ReadVBR64`: at most 64 bits of value).
    pub fn vbr(&mut self, n: u32) -> Result<u64> {
        if n == 0 || n > MAX_CHUNK {
            return Err(malformed(self.pos, "VBR width"));
        }
        let hi = 1u64 << (n - 1);
        let mut piece = self.read(n)?;
        if piece & hi == 0 {
            return Ok(piece);
        }
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            result |= (piece & (hi - 1)) << shift;
            if piece & hi == 0 {
                return Ok(result);
            }
            shift += n - 1;
            if shift >= 64 {
                return Err(malformed(self.pos, "unterminated VBR"));
            }
            piece = self.read(n)?;
        }
    }

    /// Reads a variable-width integer of at most 32 bits (as LLVM's
    /// `ReadVBR`).
    pub fn vbr32(&mut self, n: u32) -> Result<u32> {
        if n == 0 || n > MAX_CHUNK {
            return Err(malformed(self.pos, "VBR width"));
        }
        let hi = 1u64 << (n - 1);
        let mut piece = self.read(n)?;
        if piece & hi == 0 {
            return Ok(low32(piece));
        }
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            result |= (piece & (hi - 1)) << shift;
            if piece & hi == 0 {
                return Ok(low32(result));
            }
            shift += n - 1;
            if shift >= 32 {
                return Err(malformed(self.pos, "unterminated VBR"));
            }
            piece = self.read(n)?;
        }
    }

    /// Moves to the next 32-bit boundary (or the end, if that comes
    /// first).
    pub fn align32(&mut self) {
        self.pos = self
            .pos
            .div_ceil(32)
            .saturating_mul(32)
            .min(self.len_bits());
    }
}

/// How an abbreviation encodes one operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AbbrevOp {
    /// A value not in the stream.
    Literal(u64),
    /// A fixed-width field of this many bits (1 to 32).
    Fixed(u32),
    /// A variable-width field of chunks of this many bits (1 to 32).
    Vbr(u32),
    /// An array: a VBR6 count, then that many of the next operand.
    Array,
    /// A 6-bit character (`[a-zA-Z0-9._]`).
    Char6,
    /// A blob: a VBR6 byte count, 32-bit alignment, the bytes, and
    /// alignment again.
    Blob,
}

/// The character a 6-bit value encodes.
pub fn char6(v: u64) -> u8 {
    const TABLE: &[u8; 64] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._";
    TABLE[(v & 63) as usize]
}

/// An abbreviation: how a record's code and operands are encoded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Abbrev {
    /// The code's encoding, then each operand's.
    pub ops: Vec<AbbrevOp>,
}

impl Abbrev {
    /// Reads a `DEFINE_ABBREV` record's body (after its abbreviation ID).
    pub fn read(r: &mut BitReader<'_>) -> Result<Self> {
        let at = r.position();
        let n = r.vbr32(5)?;
        let mut ops = Vec::new();
        for _ in 0..n {
            if r.read(1)? == 1 {
                ops.push(AbbrevOp::Literal(r.vbr(8)?));
                continue;
            }
            let op = match r.read(3)? {
                e @ (1 | 2) => {
                    let width = r.vbr(5)?;
                    if width == 0 {
                        // Fixed(0) and VBR(0) are a literal zero.
                        AbbrevOp::Literal(0)
                    } else if width > u64::from(MAX_CHUNK) {
                        return Err(malformed(at, "abbreviation field width"));
                    } else {
                        let width = low32(width);
                        if e == 1 {
                            AbbrevOp::Fixed(width)
                        } else {
                            AbbrevOp::Vbr(width)
                        }
                    }
                }
                3 => AbbrevOp::Array,
                4 => AbbrevOp::Char6,
                5 => AbbrevOp::Blob,
                _ => return Err(malformed(at, "abbreviation encoding")),
            };
            ops.push(op);
        }
        if ops.is_empty() {
            return Err(malformed(at, "abbreviation with no operands"));
        }
        Ok(Self { ops })
    }
}

/// What `BLOCKINFO` says about one block ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockInfoEntry {
    /// The block ID.
    pub id: u32,
    /// Abbreviations every block with the ID starts with.
    pub abbrevs: Vec<Arc<Abbrev>>,
    /// Its name (`BLOCKNAME`), if given.
    pub name: Option<Vec<u8>>,
    /// Record names (`SETRECORDNAME`): code and name.
    pub record_names: Vec<(u32, Vec<u8>)>,
}

/// A `BLOCKINFO` block's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockInfo {
    /// What it says, by block ID, in the order the IDs first appear.
    pub blocks: Vec<BlockInfoEntry>,
}

impl BlockInfo {
    /// What it says about block `id`.
    pub fn get(&self, id: u32) -> Option<&BlockInfoEntry> {
        self.blocks.iter().find(|b| b.id == id)
    }

    fn get_or_create(&mut self, id: u32) -> usize {
        match self.blocks.iter().position(|b| b.id == id) {
            Some(i) => i,
            None => {
                self.blocks.push(BlockInfoEntry {
                    id,
                    ..BlockInfoEntry::default()
                });
                self.blocks.len() - 1
            }
        }
    }

    /// The name `BLOCKINFO` gives block `id`.
    pub fn block_name(&self, id: u32) -> Option<&[u8]> {
        self.get(id)?.name.as_deref()
    }

    /// The name `BLOCKINFO` gives record `code` of block `id`.
    pub fn record_name(&self, id: u32, code: u32) -> Option<&[u8]> {
        self.get(id)?
            .record_names
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, n)| n.as_slice())
    }
}

/// One step through a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The block ended (and the cursor left it).
    EndBlock,
    /// A sub-block with this ID starts: enter or skip it.
    SubBlock(u32),
    /// A record with this abbreviation ID: read it.
    Record(u32),
}

/// A block entered: its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockHeader {
    /// The block ID.
    pub id: u32,
    /// The width of its abbreviation IDs.
    pub abbrev_width: u32,
    /// Its length in 32-bit words (after the length field), as given.
    pub num_words: u32,
    /// Where its contents start (after the length field), in bits.
    pub start: u64,
}

impl BlockHeader {
    /// Where the block ends, by its length, in bits.
    pub fn end(&self) -> u64 {
        self.start + u64::from(self.num_words) * 32
    }
}

/// A record read: its code, and its blob, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordHead<'d> {
    /// The record code.
    pub code: u32,
    /// The blob operand's bytes, if the abbreviation has one.
    pub blob: Option<&'d [u8]>,
}

#[derive(Clone, Debug)]
struct Scope {
    width: u32,
    abbrevs: Vec<Arc<Abbrev>>,
    id: u32,
}

/// A position in a bitstream, with the state of the blocks it's in: the
/// abbreviation width and abbreviations of each, and the `BLOCKINFO` in
/// effect.
#[derive(Clone, Debug)]
pub struct Cursor<'d> {
    r: BitReader<'d>,
    width: u32,
    abbrevs: Vec<Arc<Abbrev>>,
    scopes: Vec<Scope>,
    info: Arc<BlockInfo>,
}

impl<'d> Cursor<'d> {
    /// A cursor at bit `bit` of `data`, at the top level (abbreviation IDs
    /// 2 bits wide), with no `BLOCKINFO`.
    pub fn new(data: &'d [u8], bit: u64) -> Result<Self> {
        let mut r = BitReader::new(data);
        r.seek(bit)?;
        Ok(Self {
            r,
            width: 2,
            abbrevs: Vec::new(),
            scopes: Vec::new(),
            info: Arc::new(BlockInfo::default()),
        })
    }

    /// A cursor at bit `bit` of `data` (the start of a block's contents),
    /// as if inside block `id` (with abbreviation IDs `width` bits wide),
    /// with `info`'s abbreviations for it: for reading a block apart from
    /// its parents (each function body on its own thread).
    pub fn in_block(
        data: &'d [u8],
        bit: u64,
        id: u32,
        width: u32,
        info: Arc<BlockInfo>,
    ) -> Result<Self> {
        let mut c = Self::new(data, bit)?;
        c.abbrevs = info.get(id).map(|b| b.abbrevs.clone()).unwrap_or_default();
        c.width = width;
        c.scopes.push(Scope {
            width: 2,
            abbrevs: Vec::new(),
            id,
        });
        c.info = info;
        Ok(c)
    }

    /// The bit reader (for reading blobs' contents, or peeking).
    pub fn reader(&self) -> &BitReader<'d> {
        &self.r
    }

    /// The bytes.
    pub fn data(&self) -> &'d [u8] {
        self.r.data
    }

    /// The position, in bits.
    pub fn position(&self) -> u64 {
        self.r.pos
    }

    /// Moves to bit `bit`, keeping the block state.
    pub fn seek(&mut self, bit: u64) -> Result<()> {
        self.r.seek(bit)
    }

    /// Whether the stream has been read to its end.
    pub fn at_end(&self) -> bool {
        self.r.at_end()
    }

    /// The width of abbreviation IDs in the current block.
    pub fn abbrev_width(&self) -> u32 {
        self.width
    }

    /// How deep in blocks the cursor is (0 at the top level).
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

    /// The ID of the block the cursor is in.
    pub fn block_id(&self) -> Option<u32> {
        self.scopes.last().map(|s| s.id)
    }

    /// The `BLOCKINFO` in effect.
    pub fn block_info(&self) -> &Arc<BlockInfo> {
        &self.info
    }

    /// Puts `info` in effect (for blocks entered from now on).
    pub fn set_block_info(&mut self, info: Arc<BlockInfo>) {
        self.info = info;
    }

    /// The abbreviations of the current block.
    pub fn abbrevs(&self) -> &[Arc<Abbrev>] {
        &self.abbrevs
    }

    /// The abbreviation with ID `id` in the current block.
    pub fn abbrev(&self, id: u32) -> Result<&Arc<Abbrev>> {
        id.checked_sub(FIRST_APPLICATION_ABBREV)
            .and_then(|i| self.abbrevs.get(i as usize))
            .ok_or_else(|| malformed(self.r.pos, "abbreviation ID"))
    }

    /// The next entry of the current block. `DEFINE_ABBREV` records are
    /// read and installed on the way, unless `keep_abbrevs` is set: then
    /// they're given as `Record(DEFINE_ABBREV)`, to read with
    /// [`read_abbrev`](Self::read_abbrev).
    pub fn advance_with(&mut self, keep_abbrevs: bool) -> Result<Entry> {
        loop {
            if self.r.at_end() {
                return Err(truncated(self.r.pos, "bitstream (no END_BLOCK)"));
            }
            let code = low32(self.r.read(self.width)?);
            match code {
                END_BLOCK => {
                    let Some(scope) = self.scopes.pop() else {
                        return Err(malformed(self.r.pos, "END_BLOCK at the top level"));
                    };
                    self.r.align32();
                    self.width = scope.width;
                    self.abbrevs = scope.abbrevs;
                    return Ok(Entry::EndBlock);
                }
                ENTER_SUBBLOCK => return Ok(Entry::SubBlock(self.r.vbr32(8)?)),
                DEFINE_ABBREV if !keep_abbrevs => self.read_abbrev()?,
                _ => return Ok(Entry::Record(code)),
            }
        }
    }

    /// The next entry, abbreviations installed on the way.
    pub fn advance(&mut self) -> Result<Entry> {
        self.advance_with(false)
    }

    /// The next entry that isn't a sub-block (sub-blocks are skipped).
    pub fn advance_skipping_blocks(&mut self) -> Result<Entry> {
        loop {
            match self.advance()? {
                Entry::SubBlock(_) => self.skip_block()?,
                e => return Ok(e),
            }
        }
    }

    /// Reads a `DEFINE_ABBREV` (its ID read) and adds it to the current
    /// block's abbreviations.
    pub fn read_abbrev(&mut self) -> Result<()> {
        let a = Abbrev::read(&mut self.r)?;
        self.abbrevs.push(Arc::new(a));
        Ok(())
    }

    /// Enters sub-block `id` (its ID read): reads its header and starts its
    /// abbreviations with `BLOCKINFO`'s for it.
    pub fn enter_block(&mut self, id: u32) -> Result<BlockHeader> {
        let mut abbrevs = self
            .info
            .get(id)
            .map(|b| b.abbrevs.clone())
            .unwrap_or_default();
        core::mem::swap(&mut abbrevs, &mut self.abbrevs);
        self.scopes.push(Scope {
            width: self.width,
            abbrevs,
            id,
        });
        let at = self.r.pos;
        let width = self.r.vbr32(4)?;
        if width > MAX_CHUNK {
            return Err(malformed(at, "block abbreviation width"));
        }
        self.r.align32();
        let num_words = low32(self.r.read(32)?);
        if width == 0 {
            return Err(malformed(at, "block abbreviation width"));
        }
        if self.r.at_end() {
            return Err(truncated(at, "block (empty)"));
        }
        self.width = width;
        Ok(BlockHeader {
            id,
            abbrev_width: width,
            num_words,
            start: self.r.pos,
        })
    }

    /// Skips a sub-block (its ID read), by its length.
    pub fn skip_block(&mut self) -> Result<()> {
        let at = self.r.pos;
        self.r.vbr32(4)?;
        self.r.align32();
        let num_words = self.r.read(32)?;
        if self.r.at_end() {
            return Err(truncated(at, "block (empty)"));
        }
        let to = self.r.pos + num_words * 32;
        if to > self.r.len_bits() {
            return Err(truncated(at, "block (longer than the stream)"));
        }
        self.r.pos = to;
        Ok(())
    }

    /// Reads a record with abbreviation ID `abbrev_id` (read by
    /// [`advance`](Self::advance)): its operands are appended to `ops`, and
    /// its code and blob (if any) returned. A blob's bytes aren't added to
    /// `ops`.
    pub fn read_record(&mut self, abbrev_id: u32, ops: &mut Vec<u64>) -> Result<RecordHead<'d>> {
        let at = self.r.pos;
        let len_bits = self.r.len_bits();
        let plausible = |n: u32| u64::from(n) < len_bits;
        if abbrev_id == UNABBREV_RECORD {
            let code = self.r.vbr32(6)?;
            let n = self.r.vbr32(6)?;
            if !plausible(n) {
                return Err(malformed(at, "record size"));
            }
            for _ in 0..n {
                ops.push(self.r.vbr(6)?);
            }
            return Ok(RecordHead { code, blob: None });
        }
        let abbrev = Arc::clone(self.abbrev(abbrev_id)?);
        let mut blob = None;
        let code = match abbrev.ops[0] {
            AbbrevOp::Literal(v) => low32(v),
            AbbrevOp::Array | AbbrevOp::Blob => {
                return Err(malformed(at, "abbreviation starting with an array or blob"));
            }
            op => low32(self.field(op)?),
        };
        let n_ops = abbrev.ops.len();
        let mut i = 1;
        while i < n_ops {
            match abbrev.ops[i] {
                AbbrevOp::Literal(v) => ops.push(v),
                AbbrevOp::Array => {
                    let n = self.r.vbr32(6)?;
                    if !plausible(n) {
                        return Err(malformed(at, "array size"));
                    }
                    if i + 2 != n_ops {
                        return Err(malformed(at, "array not second to last"));
                    }
                    i += 1;
                    let elt = abbrev.ops[i];
                    match elt {
                        AbbrevOp::Fixed(_) | AbbrevOp::Vbr(_) | AbbrevOp::Char6 => {
                            for _ in 0..n {
                                ops.push(self.field(elt)?);
                            }
                        }
                        _ => return Err(malformed(at, "array element encoding")),
                    }
                }
                AbbrevOp::Blob => {
                    let n = self.r.vbr32(6)?;
                    self.r.align32();
                    let start = self.r.pos;
                    let end = start + u64::from(n).div_ceil(4) * 32;
                    if end > self.r.len_bits() {
                        return Err(truncated(at, "blob"));
                    }
                    self.r.pos = end;
                    let s = idx(start / 8);
                    blob = Some(self.r.data.get(s..s + n as usize).unwrap_or(&[]));
                }
                op => ops.push(self.field(op)?),
            }
            i += 1;
        }
        Ok(RecordHead { code, blob })
    }

    fn field(&mut self, op: AbbrevOp) -> Result<u64> {
        match op {
            AbbrevOp::Fixed(w) => self.r.read(w),
            AbbrevOp::Vbr(w) => self.r.vbr(w),
            AbbrevOp::Char6 => Ok(u64::from(char6(self.r.read(6)?))),
            AbbrevOp::Literal(v) => Ok(v),
            AbbrevOp::Array | AbbrevOp::Blob => Err(malformed(self.r.pos, "abbreviation field")),
        }
    }

    /// Reads a `BLOCKINFO` block (its ID read): its contents, which the
    /// caller puts in effect ([`set_block_info`](Self::set_block_info)).
    pub fn read_block_info(&mut self) -> Result<BlockInfo> {
        self.enter_block(BLOCKINFO_BLOCK_ID)?;
        let mut info = BlockInfo::default();
        let mut current: Option<usize> = None;
        let mut ops = Vec::new();
        loop {
            let at = self.r.pos;
            let id = match self.advance_with(true)? {
                Entry::SubBlock(_) => {
                    self.skip_block()?;
                    continue;
                }
                Entry::EndBlock => return Ok(info),
                Entry::Record(id) => id,
            };
            if id == DEFINE_ABBREV {
                let Some(c) = current else {
                    return Err(malformed(at, "BLOCKINFO abbreviation before SETBID"));
                };
                let a = Abbrev::read(&mut self.r)?;
                info.blocks[c].abbrevs.push(Arc::new(a));
                continue;
            }
            ops.clear();
            let head = self.read_record(id, &mut ops)?;
            match head.code {
                BLOCKINFO_CODE_SETBID => {
                    let Some(&b) = ops.first() else {
                        return Err(malformed(at, "BLOCKINFO SETBID"));
                    };
                    current = Some(info.get_or_create(low32(b)));
                }
                BLOCKINFO_CODE_BLOCKNAME => {
                    let Some(c) = current else {
                        return Err(malformed(at, "BLOCKINFO BLOCKNAME before SETBID"));
                    };
                    info.blocks[c].name = Some(crate::types::chars(&ops));
                }
                BLOCKINFO_CODE_SETRECORDNAME => {
                    let Some(c) = current else {
                        return Err(malformed(at, "BLOCKINFO SETRECORDNAME before SETBID"));
                    };
                    let Some((&code, name)) = ops.split_first() else {
                        return Err(malformed(at, "BLOCKINFO SETRECORDNAME"));
                    };
                    info.blocks[c]
                        .record_names
                        .push((low32(code), crate::types::chars(name)));
                }
                _ => {}
            }
        }
    }
}

/// What a bitstream holds, by its magic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamKind {
    /// LLVM IR (`BC` `0xC0DE`).
    LlvmIr,
    /// A clang precompiled header or module (`CPCH`).
    ClangAst,
    /// Clang's serialized diagnostics (`DIAG`).
    ClangDiagnostics,
    /// LLVM's optimization remarks (`RMRK`).
    Remarks,
    /// Another stream.
    Unknown,
}

/// The bitcode wrapper header (Darwin's): where the bitcode is in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wrapper {
    /// `0x0B17C0DE`.
    pub magic: u32,
    /// The wrapper's version (0).
    pub version: u32,
    /// Where the bitcode starts.
    pub offset: u32,
    /// Its size.
    pub size: u32,
    /// The target's Mach-O CPU type.
    pub cpu_type: u32,
}

/// A bitstream: its bytes (from its magic), what it is, and the wrapper it
/// was in.
#[derive(Clone, Copy, Debug)]
pub struct Stream<'d> {
    /// The stream's bytes, from its magic.
    pub data: &'d [u8],
    /// Where they start in the input.
    pub offset: u64,
    /// The wrapper around it, if any.
    pub wrapper: Option<Wrapper>,
    /// Its first four bytes.
    pub magic: [u8; 4],
    /// What it holds.
    pub kind: StreamKind,
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Whether `data` starts like LLVM IR bitcode (bare or wrapped).
pub fn is_bitcode(data: &[u8]) -> bool {
    data.starts_with(b"BC\xC0\xDE") || le32(data, 0) == Some(WRAPPER_MAGIC)
}

impl<'d> Stream<'d> {
    /// The bitstream in `data`: after its wrapper, if it has one. Any
    /// four-byte magic is taken; [`kind`](Self::kind) says which it is.
    pub fn new(data: &'d [u8]) -> Result<Self> {
        let mut wrapper = None;
        let mut bytes = data;
        let mut offset = 0u64;
        if le32(data, 0) == Some(WRAPPER_MAGIC) {
            let field =
                |at| le32(data, at).ok_or(Error::new(ErrorKind::Truncated, 0, "bitcode wrapper"));
            let w = Wrapper {
                magic: WRAPPER_MAGIC,
                version: field(4)?,
                offset: field(8)?,
                size: field(12)?,
                cpu_type: field(16)?,
            };
            let start = u64::from(w.offset);
            let end = start + u64::from(w.size);
            if end > data.len() as u64 {
                return Err(Error::new(ErrorKind::Truncated, 8, "bitcode wrapper size"));
            }
            bytes = &data[idx(start)..idx(end)];
            offset = start;
            wrapper = Some(w);
        }
        let Some(m) = bytes.get(..4) else {
            return Err(Error::new(ErrorKind::Truncated, offset, "bitstream magic"));
        };
        let magic = [m[0], m[1], m[2], m[3]];
        let kind = match &magic {
            b"BC\xC0\xDE" => StreamKind::LlvmIr,
            b"CPCH" => StreamKind::ClangAst,
            b"DIAG" => StreamKind::ClangDiagnostics,
            b"RMRK" => StreamKind::Remarks,
            _ => StreamKind::Unknown,
        };
        Ok(Self {
            data: bytes,
            offset,
            wrapper,
            magic,
            kind,
        })
    }

    /// A cursor at the stream's first block (after the magic).
    pub fn cursor(&self) -> Cursor<'d> {
        // The magic is there (checked when made), so bit 32 is in range.
        Cursor::new(self.data, 32).unwrap_or_else(|_| Cursor {
            r: BitReader::new(&[]),
            width: 2,
            abbrevs: Vec::new(),
            scopes: Vec::new(),
            info: Arc::new(BlockInfo::default()),
        })
    }

    /// The stream's top-level blocks (skipped by their lengths, not
    /// read): their IDs and where each starts (its `ENTER_SUBBLOCK`) and
    /// ends, in bits. Stops at the first thing that isn't a block.
    pub fn top_blocks(&self) -> Vec<TopBlock> {
        let mut out = Vec::new();
        let mut c = self.cursor();
        while !c.at_end() {
            let start = c.position();
            let Ok(code) = c.r.read(2) else { break };
            if low32(code) != ENTER_SUBBLOCK {
                break;
            }
            let Ok(id) = c.r.vbr32(8) else { break };
            let header = c.position();
            if c.skip_block().is_err() {
                break;
            }
            out.push(TopBlock {
                id,
                start,
                header,
                end: c.position(),
            });
        }
        out
    }
}

/// A top-level block of a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopBlock {
    /// The block ID.
    pub id: u32,
    /// Where its `ENTER_SUBBLOCK` starts, in bits.
    pub start: u64,
    /// Where its header (after the block ID) starts, in bits: where
    /// [`Cursor::enter_block`] reads.
    pub header: u64,
    /// Where it ends (by its length), in bits.
    pub end: u64,
}
