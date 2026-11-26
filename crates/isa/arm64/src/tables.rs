//! Types for the generated decode tables (see `build.rs`).

#![allow(clippy::unreadable_literal)]

/// Bits of the instruction word, or literal bits.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Piece {
    Bits(u32, u32),
    Lit(u64, u32),
}

/// The value of concatenated pieces, most significant first.
pub(crate) fn value(pieces: &[Piece], w: u32) -> u64 {
    let mut v = 0u64;
    for p in pieces {
        match *p {
            Piece::Bits(hi, lo) => {
                let n = hi - lo + 1;
                v = (v << n) | ((u64::from(w) >> lo) & ((1u64 << n) - 1));
            }
            Piece::Lit(bits, n) => v = (v << n) | bits,
        }
    }
    v
}

///
/// (Compact: there are thousands. Its name, `ne`, conditions, and
/// templates are indexes into shared tables, read by its methods.)
pub(crate) struct Encoding {
    /// Its name ([`Encoding::name`]): `NAMES[name_at..][..name_len]`.
    pub name_at: u32,
    pub name_len: u8,
    /// The instruction class (an index into `CLASSES`).
    pub class: u8,
    /// The architecture feature the encoding needs, such as `FEAT_SVE2` (an
    /// index into `FEATURES`).
    pub feature: u8,
    pub mask: u32,
    pub value: u32,
    /// Should-be bits (Arm's `(0)` and `(1)`): `(mask, value)`. Not
    /// matched: when they differ, the instruction is CONSTRAINED
    /// UNPREDICTABLE, not UNDEFINED.
    pub should: (u32, u32),
    /// `(mask, value)` pairs the word must not match ([`Encoding::ne`]):
    /// `NE[ne_at..][..ne_len]`.
    pub ne_at: u16,
    pub ne_len: u8,
    /// When the encoding is UNDEFINED ([`Encoding::undef`]): an index into
    /// `CONDS`, or `NONE`.
    pub undef: u16,
    /// When the encoding is CONSTRAINED UNPREDICTABLE
    /// ([`Encoding::unpred`]): the same.
    pub unpred: u16,
    /// For aliases: the index of the encoding this stands for.
    pub alias_of: u16,
    /// For aliases: when to prefer the alias ([`Encoding::prefer`]): the
    /// same.
    pub prefer: u16,
    /// The template compiled (see build.rs's `tokens`): the mnemonic and
    /// the operands ([`Encoding::mnem`], [`Encoding::ops`]: indexes into
    /// `TOK_LISTS`), and the condition operand folded into the mnemonic (a
    /// symbol, or `NONE`).
    pub mnem: u16,
    pub ops: u16,
    pub cond: u16,
    /// Whether a word can fail to render (some symbol of the template has
    /// a table, a condition, or a logical immediate): if not, every word
    /// that matches renders.
    pub checked: bool,
    /// The first encoding with this one's name (an encoding can be in
    /// parts, one per operand layout).
    pub group: u16,
    /// For aliases: where Microsoft's preference differs from ARM's (an
    /// [`ms`] constant).
    pub ms: u8,
    /// Whether Microsoft keeps a space before the register of a
    /// post-indexed structure load or store ("[x2], x0").
    pub post_index: bool,
    /// Which of Microsoft's formatting rules applies (a [`rule`] constant).
    pub rule: u8,
}

/// What an encoding reads and writes, and its operands (apart from
/// [`Encoding`], which decoding scans).
pub(crate) struct Semantics {
    /// Its LLVM record (an index into `RECORDS`).
    pub record: u16,
    /// Its operands, as src/operand.rs evaluates them.
    pub operands: &'static [OpSpec],
}

/// An operand of an encoding (build.rs's `operand_specs`): its kind (an
/// [`opk`] constant), its access (1 read, 2 write; for memory, 1 load, 2
/// store), and its parts: each a [`role`], a symbol (or `NONE`), and
/// literal text.
pub(crate) struct OpSpec {
    pub kind: u8,
    pub access: u8,
    pub parts: &'static [(u8, u16, &'static str)],
}

/// Operand kinds (build.rs's `opk`).
#[allow(dead_code, reason = "the full set build.rs uses")]
pub(crate) mod opk {
    pub const REG: u8 = 0;
    pub const LIST: u8 = 1;
    pub const IMM: u8 = 2;
    pub const FPIMM: u8 = 3;
    pub const LABEL: u8 = 4;
    pub const COND: u8 = 5;
    pub const SHIFT: u8 = 6;
    pub const MUL: u8 = 7;
    pub const MEM: u8 = 8;
    pub const ZA: u8 = 9;
    pub const SYSREG: u8 = 10;
    pub const NAME: u8 = 11;
}

/// The roles of an operand's parts (build.rs's `role`).
#[allow(dead_code, reason = "the full set build.rs uses")]
pub(crate) mod role {
    pub const REG: u8 = 0;
    pub const PREFIX: u8 = 1;
    pub const ARR: u8 = 2;
    pub const LANE: u8 = 3;
    pub const PRED: u8 = 4;
    pub const WB: u8 = 5;
    pub const RANGE: u8 = 6;
    pub const VALUE: u8 = 7;
    pub const SHIFT: u8 = 8;
    pub const INDEX: u8 = 9;
    pub const OFFSET: u8 = 10;
    pub const MULVL: u8 = 11;
    pub const POST: u8 = 12;
    pub const TILE: u8 = 13;
    pub const HV: u8 = 14;
    pub const ZAINDEX: u8 = 15;
    pub const ZAOFF: u8 = 16;
    pub const ZALAST: u8 = 17;
    pub const VGX: u8 = 18;
    pub const AMOUNT: u8 = 19;
}

/// How a register class's field decodes (LLVM's decoder for the class;
/// see tools/arm64/access.py): the first register for each field value
/// (255 where the value is invalid), and how many registers there are,
/// this far apart.
pub(crate) struct RegClass {
    #[allow(dead_code, reason = "for tools and debugging")]
    pub name: &'static str,
    /// A [`file`] constant.
    pub file: u8,
    /// The element size: 0 none, then 1 to 5 for B, H, S, D, Q.
    #[allow(dead_code, reason = "for tools and debugging")]
    pub element: u8,
    pub count: u8,
    pub stride: u8,
    pub firsts: &'static [u8],
}

/// Register files (build.rs's `FILES`, in the same order).
pub(crate) mod file {
    /// General-purpose, 64-bit (31 is the zero register).
    pub const X: u8 = 0;
    /// General-purpose, 64-bit (31 is the stack pointer).
    pub const XSP: u8 = 1;
    pub const W: u8 = 2;
    pub const WSP: u8 = 3;
    /// Scalar SIMD&FP registers: B, H, S, D, Q (4 to 8); then vectors of
    /// 64 and 128 bits (9, 10).
    pub const B: u8 = 4;
    pub const Z: u8 = 11;
    pub const P: u8 = 12;
    pub const PN: u8 = 13;
    /// The ZA array, or a tile of it (numbered); then ZT0 (15).
    pub const ZA: u8 = 14;
}

/// An operand of an LLVM record: whether it is read (1) and written (2),
/// its register class (an index into `REG_CLASSES`), and its bits.
pub(crate) struct LlvmOperand {
    pub access: u8,
    pub class: u8,
    pub bits: &'static [Piece],
}

/// What an LLVM record reads and writes: its register operands, its
/// implicit registers (`RegSet` bits), and [`flag`]s.
pub(crate) struct Record {
    #[allow(dead_code, reason = "for tools and debugging")]
    pub name: &'static str,
    pub flags: u32,
    pub ops: &'static [LlvmOperand],
    pub defs: u128,
    pub uses: u128,
}

/// Record flags (build.rs's `ACCESS_FLAGS`, in the same order).
pub(crate) mod flag {
    pub const LOAD: u32 = 1 << 0;
    pub const STORE: u32 = 1 << 1;
    pub const CALL: u32 = 1 << 2;
    pub const BRANCH: u32 = 1 << 3;
    pub const INDIRECT: u32 = 1 << 4;
    pub const RETURN: u32 = 1 << 5;
    pub const BARRIER: u32 = 1 << 6;
    pub const SIDE: u32 = 1 << 7;
    pub const FPEXC: u32 = 1 << 8;
    pub const TRAP: u32 = 1 << 9;
    pub const EXCEPTION: u32 = 1 << 10;
    /// MSR (register): the system register is in the word.
    pub const MSR: u32 = 1 << 11;
    /// MRS: the system register is in the word.
    pub const MRS: u32 = 1 << 12;
    /// MSR (immediate): the PSTATE field is in the word.
    pub const MSRIMM: u32 = 1 << 13;
}

/// Microsoft's formatting rules for particular encodings (build.rs's
/// `ms_rule`; see format.rs's `render_parts`).
pub(crate) mod rule {
    /// None.
    pub const NONE: u8 = 0;
    /// MOV (bitmask immediate): MOV or MOVI.
    pub const MOV_ORR: u8 = 1;
    /// ORR (immediate): MOV or MOVI from the zero register.
    pub const ORR_IMM: u8 = 2;
    /// LSR (immediate): `lsl #0` when the shift is zero.
    pub const LSR_UBFM: u8 = 3;
    /// UBFX (64-bit): UXTB, UXTH, UXTW.
    pub const UBFX_64: u8 = 4;
    /// ADD/SUB (shifted register): a zero LSL kept with the zero register.
    pub const ADD_SUB_SHIFT: u8 = 5;
    /// EXT: Microsoft's `ext8`.
    pub const EXT8: u8 = 6;
    /// MOVPRFX: Microsoft's `moveprfx`.
    pub const MOVEPRFX: u8 = 7;
}

/// Microsoft's own mnemonic for an encoding, where it has one.
pub(crate) fn ms_mnemonic(e: &Encoding) -> Option<&'static str> {
    match e.rule {
        rule::EXT8 => Some("ext8"),
        rule::MOVEPRFX => Some("moveprfx"),
        _ => None,
    }
}

/// Where Microsoft's disassembler prefers an alias differently from ARM
/// (build.rs's `ms_preference`).
pub(crate) mod ms {
    /// As ARM's description says.
    pub const ARM: u8 = 0;
    /// Never (ARM prefers it; Microsoft does not use it).
    pub const NOT_PREFERRED: u8 = 1;
    /// CINC and CINV: where Rn is Rm, and not the zero register.
    pub const CINC: u8 = 2;
    /// CNEG: where Rn is Rm.
    pub const CNEG: u8 = 3;
    /// CSET and CSETM: where Rn and Rm are the zero register.
    pub const CSET: u8 = 4;
}

/// A piece of a compiled template.
/// (Four bytes: its text and its group's tokens by index.)
#[derive(Clone, Copy, Debug)]
pub(crate) enum Tok {
    /// Text, as shown: an index into `LITS` (see [`lit`]).
    Lit(u16),
    /// A symbol, by index.
    Sym(u16),
    /// `{...}`, its tokens an index into `TOK_LISTS` (see [`toks`]): left
    /// out when every symbol in it is a default (and it has symbols, or is
    /// `, #0`: `optional`).
    Group(u16, bool),
}

/// The literal text `i`.
#[inline]
pub(crate) fn lit(i: u16) -> &'static str {
    LITS.get(usize::from(i)).copied().unwrap_or("")
}

/// The token list `i`.
#[inline]
pub(crate) fn toks(i: u16) -> &'static [Tok] {
    TOK_LISTS.get(usize::from(i)).copied().unwrap_or(&[])
}

/// A table row: the mask and value it matches over the table's key, the
/// result text, for computed results ("UInt(H:L:M)") the bits and
/// `mul * bits + add`, and for results shown as they are, the text shown
/// and whether it is a default (see build.rs's `shown_as_is`).
pub(crate) type Row = (
    u64,
    u64,
    &'static str,
    &'static [Piece],
    i64,
    i64,
    Option<(&'static str, bool)>,
);

/// A table mapping field values to text: the first row matching the key
/// (the table's columns side by side).
pub(crate) struct Table {
    pub key: &'static [Piece],
    pub rows: &'static [Row],
}

/// How a symbol's number is computed from its field(s).
pub(crate) struct Value {
    pub pieces: &'static [Piece],
    pub width: u32,
    pub mul: i64,
    pub add: i64,
    pub modulo: i64,
    /// If non-zero, the value is `sub_from - field`.
    pub sub_from: i64,
    pub signed: bool,
}

impl Value {
    pub fn eval(&self, w: u32) -> i64 {
        let raw = value(self.pieces, w);
        let mut v = if self.signed && self.width > 0 && self.width < 64 {
            let shift = 64 - self.width;
            ((raw << shift) as i64) >> shift
        } else {
            raw as i64
        };
        if self.sub_from != 0 {
            v = self.sub_from - v;
        }
        v = v * self.mul + self.add;
        if self.modulo != 0 {
            v = v.rem_euclid(self.modulo);
        }
        v
    }
}

/// (Compact: its display, bits, table, value, condition, and attributes
/// are indexes into tables of them (`DISPLAYS`, `PIECE_LISTS`, `TABLES`,
/// `VALUES`, `ATTRS`; `NONE` for none), read by its methods.)
pub(crate) struct Symbol {
    display: u16,
    encoded: u16,
    table: u16,
    value: u16,
    when: u16,
    attrs: u16,
    when_value: u32,
    /// The attributes in [`attr`], as flags.
    pub flags: u32,
    /// How it shows, where it has no table and is not special.
    pub kind: Kind,
    /// Its LLVM operand's register class (an index into `REG_CLASSES`),
    /// or 255.
    pub class: u8,
}

/// How a symbol without a table shows (worked out from its display name by
/// build.rs's `symbol_kind`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    /// The second register of a pair, `<X(t+1)>` (64-bit if true).
    Pair(bool),
    /// A general-purpose register where 31 is the stack pointer.
    GprSp(bool),
    /// A general-purpose register where 31 is the zero register.
    Gpr(bool),
    /// A register or number shown as this prefix and the decimal number
    /// (`v`, `z`, `p`, `za`, `C`; nothing for lane indexes).
    Reg(&'static str),
    /// A predicate-as-counter register (`pn8` for 0 with `pn8`).
    Pn,
    /// A register number after a `<R>` or `<V>` prefix symbol.
    Num,
    /// A branch target.
    Label,
    /// A logical immediate.
    Bitmask,
    /// A system register.
    #[expect(
        dead_code,
        reason = "no symbol is shown as <systemreg>: system registers show by number (S3_0_C4_C2_2), never by name"
    )]
    SysReg,
    /// Any other number: an immediate.
    Imm,
}

impl Symbol {
    /// `<Wd|WSP>`, `<T>`, `<imm>`, ...
    #[inline]
    pub fn display(&self) -> &'static str {
        DISPLAYS
            .get(usize::from(self.display))
            .copied()
            .unwrap_or("")
    }

    /// The bits the symbol is encoded in.
    #[inline]
    pub fn encoded(&self) -> &'static [Piece] {
        PIECE_LISTS
            .get(usize::from(self.encoded))
            .copied()
            .unwrap_or(&[])
    }

    #[inline]
    pub fn table(&self) -> Option<&'static Table> {
        TABLES.get(usize::from(self.table))
    }

    #[inline]
    pub fn value(&self) -> Option<&'static Value> {
        VALUES.get(usize::from(self.value))
    }

    /// The symbol applies only when these bits have this value.
    #[inline]
    pub fn when(&self) -> Option<(&'static [Piece], u64)> {
        let p = PIECE_LISTS.get(usize::from(self.when))?;
        Some((p, u64::from(self.when_value)))
    }

    /// What the operand is, beyond its display name and value: `fp8`,
    /// `bitmask`, `page`, `implicit=1`, ... (space-separated; see
    /// tools/arm64/extract.py, which takes them from LLVM's operand types).
    #[inline]
    pub fn attrs(&self) -> &'static str {
        ATTRS.get(usize::from(self.attrs)).copied().unwrap_or("")
    }

    /// Whether the symbol has attribute `a` (an [`attr`] flag).
    #[inline]
    pub fn has(&self, a: u32) -> bool {
        self.flags & a != 0
    }

    /// The value of attribute `key=value`.
    pub fn attr(&self, key: &str) -> Option<&'static str> {
        self.attrs()
            .split(' ')
            .find_map(|x| x.strip_prefix(key)?.strip_prefix('='))
    }
}

/// The attributes the formatter tests, as flags (build.rs's `ATTR_FLAGS`,
/// in the same order): an attribute `a` or `a=value`.
pub(crate) mod attr {
    pub const ORIMM: u32 = 1 << 0;
    pub const LSLSP: u32 = 1 << 1;
    pub const FP8: u32 = 1 << 2;
    pub const X30: u32 = 1 << 3;
    pub const ZR31: u32 = 1 << 4;
    pub const PN8: u32 = 1 << 5;
    pub const PAGE: u32 = 1 << 6;
    pub const BITMASK: u32 = 1 << 7;
    pub const MEMEXT: u32 = 1 << 8;
    pub const PRESENT01: u32 = 1 << 9;
    pub const TSZ: u32 = 1 << 10;
    pub const BYTEMASK: u32 = 1 << 11;
    pub const MULT16: u32 = 1 << 12;
    pub const DUPINDEX: u32 = 1 << 13;
    pub const ADD: u32 = 1 << 14;
    /// Not an attribute: build.rs sets it where format.rs's `special` may
    /// apply.
    pub const MAY_SPECIAL: u32 = 1 << 15;
}

include!(concat!(env!("OUT_DIR"), "/tables.rs"));

impl Encoding {
    /// Its name (`ADD_32_addsub_imm`).
    #[inline]
    pub fn name(&self) -> &'static str {
        let at = self.name_at as usize;
        NAMES.get(at..at + usize::from(self.name_len)).unwrap_or("")
    }

    /// The `(mask, value)` pairs the word must not match.
    #[inline]
    pub fn ne(&self) -> &'static [(u32, u32)] {
        let at = usize::from(self.ne_at);
        NE.get(at..at + usize::from(self.ne_len)).unwrap_or(&[])
    }

    /// Whether `w` is UNDEFINED by the encoding's conditions.
    #[inline]
    pub fn undef(&self, w: u32) -> bool {
        holds(self.undef, w)
    }

    /// Whether `w` is CONSTRAINED UNPREDICTABLE by its conditions.
    #[inline]
    pub fn unpred(&self, w: u32) -> bool {
        holds(self.unpred, w)
    }

    /// For aliases: whether the alias is preferred for `w`.
    #[inline]
    pub fn prefer(&self, w: u32) -> bool {
        holds(self.prefer, w)
    }

    /// The template's mnemonic.
    #[inline]
    pub fn mnem(&self) -> &'static [Tok] {
        TOK_LISTS
            .get(usize::from(self.mnem))
            .copied()
            .unwrap_or(&[])
    }

    /// The template's operands.
    #[inline]
    pub fn ops(&self) -> &'static [Tok] {
        TOK_LISTS.get(usize::from(self.ops)).copied().unwrap_or(&[])
    }
}

/// Whether condition `i` (an index into `CONDS`) holds of `w`: never if
/// it is `NONE`.
#[inline]
fn holds(i: u16, w: u32) -> bool {
    CONDS.get(usize::from(i)).is_some_and(|f| f(w))
}

// (The large tables are in chunks, which rustc type-checks in seconds
// where one array takes minutes: see build.rs's `chunked`.)

/// The encoding at `i`.
#[inline]
pub(crate) fn encoding(i: usize) -> &'static Encoding {
    &ENCODINGS[i / ENCODINGS_CHUNK][i % ENCODINGS_CHUNK]
}

// (One `Semantics` for each encoding.)
const _: () = assert!(SEMANTICS_LEN == ENCODINGS_LEN);

/// The semantics of the encoding at `i`.
#[inline]
pub(crate) fn semantics(i: usize) -> &'static Semantics {
    &SEMANTICS[i / SEMANTICS_CHUNK][i % SEMANTICS_CHUNK]
}

/// The LLVM record at `i`.
#[inline]
pub(crate) fn record(i: usize) -> &'static Record {
    &RECORDS[i / RECORDS_CHUNK][i % RECORDS_CHUNK]
}

/// The symbol at `i`.
#[inline]
pub(crate) fn symbol(i: usize) -> &'static Symbol {
    &SYMBOLS[i / SYMBOLS_CHUNK][i % SYMBOLS_CHUNK]
}

/// Every encoding, in order.
pub(crate) fn encodings() -> impl Iterator<Item = &'static Encoding> {
    ENCODINGS.iter().flat_map(|c| c.iter())
}

/// How many encodings there are.
#[allow(dead_code, reason = "for tests and tools")]
pub(crate) const fn encoding_count() -> usize {
    ENCODINGS_LEN
}

/// How many LLVM records there are.
#[allow(dead_code, reason = "for tests and tools")]
pub(crate) const fn record_count() -> usize {
    RECORDS_LEN
}

/// How many symbols there are.
#[allow(dead_code, reason = "for tests and tools")]
pub(crate) const fn symbol_count() -> usize {
    SYMBOLS_LEN
}
