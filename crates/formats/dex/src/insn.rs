//! Dalvik bytecode: every opcode and instruction format, decoded
//! instruction by instruction as ART decodes it (its `VRegA` ... `VRegH`
//! operands), with the payloads of `packed-switch`, `sparse-switch`, and
//! `fill-array-data`.

use turbonop_core::{Error, ErrorKind, Result};

/// An instruction format (the Dalvik bytecode document's names: the
/// first digit is how many code units, the second how many registers,
/// the letter what else).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Format {
    F10x,
    F12x,
    F11n,
    F11x,
    F10t,
    F20t,
    F20bc,
    F22x,
    F21t,
    F21s,
    F21h,
    F21c,
    F23x,
    F22b,
    F22t,
    F22s,
    F22c,
    F22cs,
    F30t,
    F32x,
    F31i,
    F31t,
    F31c,
    F35c,
    F35ms,
    F35mi,
    F3rc,
    F3rms,
    F3rmi,
    F45cc,
    F4rcc,
    F51l,
}

impl Format {
    /// How many code units an instruction of this format takes.
    pub fn units(self) -> u32 {
        match self {
            Self::F10x | Self::F12x | Self::F11n | Self::F11x | Self::F10t => 1,
            Self::F20t
            | Self::F20bc
            | Self::F22x
            | Self::F21t
            | Self::F21s
            | Self::F21h
            | Self::F21c
            | Self::F23x
            | Self::F22b
            | Self::F22t
            | Self::F22s
            | Self::F22c
            | Self::F22cs => 2,
            Self::F30t
            | Self::F32x
            | Self::F31i
            | Self::F31t
            | Self::F31c
            | Self::F35c
            | Self::F35ms
            | Self::F35mi
            | Self::F3rc
            | Self::F3rms
            | Self::F3rmi => 3,
            Self::F45cc | Self::F4rcc => 4,
            Self::F51l => 5,
        }
    }

    /// Its name (`"22c"`).
    pub fn name(self) -> &'static str {
        match self {
            Self::F10x => "10x",
            Self::F12x => "12x",
            Self::F11n => "11n",
            Self::F11x => "11x",
            Self::F10t => "10t",
            Self::F20t => "20t",
            Self::F20bc => "20bc",
            Self::F22x => "22x",
            Self::F21t => "21t",
            Self::F21s => "21s",
            Self::F21h => "21h",
            Self::F21c => "21c",
            Self::F23x => "23x",
            Self::F22b => "22b",
            Self::F22t => "22t",
            Self::F22s => "22s",
            Self::F22c => "22c",
            Self::F22cs => "22cs",
            Self::F30t => "30t",
            Self::F32x => "32x",
            Self::F31i => "31i",
            Self::F31t => "31t",
            Self::F31c => "31c",
            Self::F35c => "35c",
            Self::F35ms => "35ms",
            Self::F35mi => "35mi",
            Self::F3rc => "3rc",
            Self::F3rms => "3rms",
            Self::F3rmi => "3rmi",
            Self::F45cc => "45cc",
            Self::F4rcc => "4rcc",
            Self::F51l => "51l",
        }
    }
}

/// What an instruction's index refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndexType {
    /// Nothing.
    None,
    /// A string.
    String,
    /// A type.
    Type,
    /// A field.
    Field,
    /// A method.
    Method,
    /// A method (`B`) and a prototype (`H`).
    MethodAndProto,
    /// A call site.
    CallSite,
    /// A method handle.
    MethodHandle,
    /// A prototype.
    Proto,
    /// A v-table offset (optimized DEX's `-quick` invokes).
    VtableOffset,
    /// A field offset (optimized DEX's `-quick` field accesses).
    FieldOffset,
    /// An inline method (optimized DEX's `execute-inline`).
    InlineMethod,
    /// A verification error's kind and reference (`throw-verification-error`).
    VerificationError,
}

/// An opcode's name, format, and index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpInfo {
    /// Its value.
    pub value: u8,
    /// Its name (`invoke-virtual/range`); unused values are `unused-NN`.
    pub name: &'static str,
    /// Its format.
    pub format: Format,
    /// What its index refers to.
    pub index: IndexType,
}

use Format::*;
use IndexType as I;

const fn op(value: u8, name: &'static str, format: Format, index: IndexType) -> OpInfo {
    OpInfo {
        value,
        name,
        format,
        index,
    }
}

/// Every opcode, by value (ART's `dex_instruction_list.h`).
pub static OPCODES: [OpInfo; 256] = [
    op(0x00, "nop", F10x, I::None),
    op(0x01, "move", F12x, I::None),
    op(0x02, "move/from16", F22x, I::None),
    op(0x03, "move/16", F32x, I::None),
    op(0x04, "move-wide", F12x, I::None),
    op(0x05, "move-wide/from16", F22x, I::None),
    op(0x06, "move-wide/16", F32x, I::None),
    op(0x07, "move-object", F12x, I::None),
    op(0x08, "move-object/from16", F22x, I::None),
    op(0x09, "move-object/16", F32x, I::None),
    op(0x0A, "move-result", F11x, I::None),
    op(0x0B, "move-result-wide", F11x, I::None),
    op(0x0C, "move-result-object", F11x, I::None),
    op(0x0D, "move-exception", F11x, I::None),
    op(0x0E, "return-void", F10x, I::None),
    op(0x0F, "return", F11x, I::None),
    op(0x10, "return-wide", F11x, I::None),
    op(0x11, "return-object", F11x, I::None),
    op(0x12, "const/4", F11n, I::None),
    op(0x13, "const/16", F21s, I::None),
    op(0x14, "const", F31i, I::None),
    op(0x15, "const/high16", F21h, I::None),
    op(0x16, "const-wide/16", F21s, I::None),
    op(0x17, "const-wide/32", F31i, I::None),
    op(0x18, "const-wide", F51l, I::None),
    op(0x19, "const-wide/high16", F21h, I::None),
    op(0x1A, "const-string", F21c, I::String),
    op(0x1B, "const-string/jumbo", F31c, I::String),
    op(0x1C, "const-class", F21c, I::Type),
    op(0x1D, "monitor-enter", F11x, I::None),
    op(0x1E, "monitor-exit", F11x, I::None),
    op(0x1F, "check-cast", F21c, I::Type),
    op(0x20, "instance-of", F22c, I::Type),
    op(0x21, "array-length", F12x, I::None),
    op(0x22, "new-instance", F21c, I::Type),
    op(0x23, "new-array", F22c, I::Type),
    op(0x24, "filled-new-array", F35c, I::Type),
    op(0x25, "filled-new-array/range", F3rc, I::Type),
    op(0x26, "fill-array-data", F31t, I::None),
    op(0x27, "throw", F11x, I::None),
    op(0x28, "goto", F10t, I::None),
    op(0x29, "goto/16", F20t, I::None),
    op(0x2A, "goto/32", F30t, I::None),
    op(0x2B, "packed-switch", F31t, I::None),
    op(0x2C, "sparse-switch", F31t, I::None),
    op(0x2D, "cmpl-float", F23x, I::None),
    op(0x2E, "cmpg-float", F23x, I::None),
    op(0x2F, "cmpl-double", F23x, I::None),
    op(0x30, "cmpg-double", F23x, I::None),
    op(0x31, "cmp-long", F23x, I::None),
    op(0x32, "if-eq", F22t, I::None),
    op(0x33, "if-ne", F22t, I::None),
    op(0x34, "if-lt", F22t, I::None),
    op(0x35, "if-ge", F22t, I::None),
    op(0x36, "if-gt", F22t, I::None),
    op(0x37, "if-le", F22t, I::None),
    op(0x38, "if-eqz", F21t, I::None),
    op(0x39, "if-nez", F21t, I::None),
    op(0x3A, "if-ltz", F21t, I::None),
    op(0x3B, "if-gez", F21t, I::None),
    op(0x3C, "if-gtz", F21t, I::None),
    op(0x3D, "if-lez", F21t, I::None),
    op(0x3E, "unused-3e", F10x, I::None),
    op(0x3F, "unused-3f", F10x, I::None),
    op(0x40, "unused-40", F10x, I::None),
    op(0x41, "unused-41", F10x, I::None),
    op(0x42, "unused-42", F10x, I::None),
    op(0x43, "unused-43", F10x, I::None),
    op(0x44, "aget", F23x, I::None),
    op(0x45, "aget-wide", F23x, I::None),
    op(0x46, "aget-object", F23x, I::None),
    op(0x47, "aget-boolean", F23x, I::None),
    op(0x48, "aget-byte", F23x, I::None),
    op(0x49, "aget-char", F23x, I::None),
    op(0x4A, "aget-short", F23x, I::None),
    op(0x4B, "aput", F23x, I::None),
    op(0x4C, "aput-wide", F23x, I::None),
    op(0x4D, "aput-object", F23x, I::None),
    op(0x4E, "aput-boolean", F23x, I::None),
    op(0x4F, "aput-byte", F23x, I::None),
    op(0x50, "aput-char", F23x, I::None),
    op(0x51, "aput-short", F23x, I::None),
    op(0x52, "iget", F22c, I::Field),
    op(0x53, "iget-wide", F22c, I::Field),
    op(0x54, "iget-object", F22c, I::Field),
    op(0x55, "iget-boolean", F22c, I::Field),
    op(0x56, "iget-byte", F22c, I::Field),
    op(0x57, "iget-char", F22c, I::Field),
    op(0x58, "iget-short", F22c, I::Field),
    op(0x59, "iput", F22c, I::Field),
    op(0x5A, "iput-wide", F22c, I::Field),
    op(0x5B, "iput-object", F22c, I::Field),
    op(0x5C, "iput-boolean", F22c, I::Field),
    op(0x5D, "iput-byte", F22c, I::Field),
    op(0x5E, "iput-char", F22c, I::Field),
    op(0x5F, "iput-short", F22c, I::Field),
    op(0x60, "sget", F21c, I::Field),
    op(0x61, "sget-wide", F21c, I::Field),
    op(0x62, "sget-object", F21c, I::Field),
    op(0x63, "sget-boolean", F21c, I::Field),
    op(0x64, "sget-byte", F21c, I::Field),
    op(0x65, "sget-char", F21c, I::Field),
    op(0x66, "sget-short", F21c, I::Field),
    op(0x67, "sput", F21c, I::Field),
    op(0x68, "sput-wide", F21c, I::Field),
    op(0x69, "sput-object", F21c, I::Field),
    op(0x6A, "sput-boolean", F21c, I::Field),
    op(0x6B, "sput-byte", F21c, I::Field),
    op(0x6C, "sput-char", F21c, I::Field),
    op(0x6D, "sput-short", F21c, I::Field),
    op(0x6E, "invoke-virtual", F35c, I::Method),
    op(0x6F, "invoke-super", F35c, I::Method),
    op(0x70, "invoke-direct", F35c, I::Method),
    op(0x71, "invoke-static", F35c, I::Method),
    op(0x72, "invoke-interface", F35c, I::Method),
    op(0x73, "unused-73", F10x, I::None),
    op(0x74, "invoke-virtual/range", F3rc, I::Method),
    op(0x75, "invoke-super/range", F3rc, I::Method),
    op(0x76, "invoke-direct/range", F3rc, I::Method),
    op(0x77, "invoke-static/range", F3rc, I::Method),
    op(0x78, "invoke-interface/range", F3rc, I::Method),
    op(0x79, "unused-79", F10x, I::None),
    op(0x7A, "unused-7a", F10x, I::None),
    op(0x7B, "neg-int", F12x, I::None),
    op(0x7C, "not-int", F12x, I::None),
    op(0x7D, "neg-long", F12x, I::None),
    op(0x7E, "not-long", F12x, I::None),
    op(0x7F, "neg-float", F12x, I::None),
    op(0x80, "neg-double", F12x, I::None),
    op(0x81, "int-to-long", F12x, I::None),
    op(0x82, "int-to-float", F12x, I::None),
    op(0x83, "int-to-double", F12x, I::None),
    op(0x84, "long-to-int", F12x, I::None),
    op(0x85, "long-to-float", F12x, I::None),
    op(0x86, "long-to-double", F12x, I::None),
    op(0x87, "float-to-int", F12x, I::None),
    op(0x88, "float-to-long", F12x, I::None),
    op(0x89, "float-to-double", F12x, I::None),
    op(0x8A, "double-to-int", F12x, I::None),
    op(0x8B, "double-to-long", F12x, I::None),
    op(0x8C, "double-to-float", F12x, I::None),
    op(0x8D, "int-to-byte", F12x, I::None),
    op(0x8E, "int-to-char", F12x, I::None),
    op(0x8F, "int-to-short", F12x, I::None),
    op(0x90, "add-int", F23x, I::None),
    op(0x91, "sub-int", F23x, I::None),
    op(0x92, "mul-int", F23x, I::None),
    op(0x93, "div-int", F23x, I::None),
    op(0x94, "rem-int", F23x, I::None),
    op(0x95, "and-int", F23x, I::None),
    op(0x96, "or-int", F23x, I::None),
    op(0x97, "xor-int", F23x, I::None),
    op(0x98, "shl-int", F23x, I::None),
    op(0x99, "shr-int", F23x, I::None),
    op(0x9A, "ushr-int", F23x, I::None),
    op(0x9B, "add-long", F23x, I::None),
    op(0x9C, "sub-long", F23x, I::None),
    op(0x9D, "mul-long", F23x, I::None),
    op(0x9E, "div-long", F23x, I::None),
    op(0x9F, "rem-long", F23x, I::None),
    op(0xA0, "and-long", F23x, I::None),
    op(0xA1, "or-long", F23x, I::None),
    op(0xA2, "xor-long", F23x, I::None),
    op(0xA3, "shl-long", F23x, I::None),
    op(0xA4, "shr-long", F23x, I::None),
    op(0xA5, "ushr-long", F23x, I::None),
    op(0xA6, "add-float", F23x, I::None),
    op(0xA7, "sub-float", F23x, I::None),
    op(0xA8, "mul-float", F23x, I::None),
    op(0xA9, "div-float", F23x, I::None),
    op(0xAA, "rem-float", F23x, I::None),
    op(0xAB, "add-double", F23x, I::None),
    op(0xAC, "sub-double", F23x, I::None),
    op(0xAD, "mul-double", F23x, I::None),
    op(0xAE, "div-double", F23x, I::None),
    op(0xAF, "rem-double", F23x, I::None),
    op(0xB0, "add-int/2addr", F12x, I::None),
    op(0xB1, "sub-int/2addr", F12x, I::None),
    op(0xB2, "mul-int/2addr", F12x, I::None),
    op(0xB3, "div-int/2addr", F12x, I::None),
    op(0xB4, "rem-int/2addr", F12x, I::None),
    op(0xB5, "and-int/2addr", F12x, I::None),
    op(0xB6, "or-int/2addr", F12x, I::None),
    op(0xB7, "xor-int/2addr", F12x, I::None),
    op(0xB8, "shl-int/2addr", F12x, I::None),
    op(0xB9, "shr-int/2addr", F12x, I::None),
    op(0xBA, "ushr-int/2addr", F12x, I::None),
    op(0xBB, "add-long/2addr", F12x, I::None),
    op(0xBC, "sub-long/2addr", F12x, I::None),
    op(0xBD, "mul-long/2addr", F12x, I::None),
    op(0xBE, "div-long/2addr", F12x, I::None),
    op(0xBF, "rem-long/2addr", F12x, I::None),
    op(0xC0, "and-long/2addr", F12x, I::None),
    op(0xC1, "or-long/2addr", F12x, I::None),
    op(0xC2, "xor-long/2addr", F12x, I::None),
    op(0xC3, "shl-long/2addr", F12x, I::None),
    op(0xC4, "shr-long/2addr", F12x, I::None),
    op(0xC5, "ushr-long/2addr", F12x, I::None),
    op(0xC6, "add-float/2addr", F12x, I::None),
    op(0xC7, "sub-float/2addr", F12x, I::None),
    op(0xC8, "mul-float/2addr", F12x, I::None),
    op(0xC9, "div-float/2addr", F12x, I::None),
    op(0xCA, "rem-float/2addr", F12x, I::None),
    op(0xCB, "add-double/2addr", F12x, I::None),
    op(0xCC, "sub-double/2addr", F12x, I::None),
    op(0xCD, "mul-double/2addr", F12x, I::None),
    op(0xCE, "div-double/2addr", F12x, I::None),
    op(0xCF, "rem-double/2addr", F12x, I::None),
    op(0xD0, "add-int/lit16", F22s, I::None),
    op(0xD1, "rsub-int", F22s, I::None),
    op(0xD2, "mul-int/lit16", F22s, I::None),
    op(0xD3, "div-int/lit16", F22s, I::None),
    op(0xD4, "rem-int/lit16", F22s, I::None),
    op(0xD5, "and-int/lit16", F22s, I::None),
    op(0xD6, "or-int/lit16", F22s, I::None),
    op(0xD7, "xor-int/lit16", F22s, I::None),
    op(0xD8, "add-int/lit8", F22b, I::None),
    op(0xD9, "rsub-int/lit8", F22b, I::None),
    op(0xDA, "mul-int/lit8", F22b, I::None),
    op(0xDB, "div-int/lit8", F22b, I::None),
    op(0xDC, "rem-int/lit8", F22b, I::None),
    op(0xDD, "and-int/lit8", F22b, I::None),
    op(0xDE, "or-int/lit8", F22b, I::None),
    op(0xDF, "xor-int/lit8", F22b, I::None),
    op(0xE0, "shl-int/lit8", F22b, I::None),
    op(0xE1, "shr-int/lit8", F22b, I::None),
    op(0xE2, "ushr-int/lit8", F22b, I::None),
    op(0xE3, "unused-e3", F10x, I::None),
    op(0xE4, "unused-e4", F10x, I::None),
    op(0xE5, "unused-e5", F10x, I::None),
    op(0xE6, "unused-e6", F10x, I::None),
    op(0xE7, "unused-e7", F10x, I::None),
    op(0xE8, "unused-e8", F10x, I::None),
    op(0xE9, "unused-e9", F10x, I::None),
    op(0xEA, "unused-ea", F10x, I::None),
    op(0xEB, "unused-eb", F10x, I::None),
    op(0xEC, "unused-ec", F10x, I::None),
    op(0xED, "unused-ed", F10x, I::None),
    op(0xEE, "unused-ee", F10x, I::None),
    op(0xEF, "unused-ef", F10x, I::None),
    op(0xF0, "unused-f0", F10x, I::None),
    op(0xF1, "unused-f1", F10x, I::None),
    op(0xF2, "unused-f2", F10x, I::None),
    op(0xF3, "unused-f3", F10x, I::None),
    op(0xF4, "unused-f4", F10x, I::None),
    op(0xF5, "unused-f5", F10x, I::None),
    op(0xF6, "unused-f6", F10x, I::None),
    op(0xF7, "unused-f7", F10x, I::None),
    op(0xF8, "unused-f8", F10x, I::None),
    op(0xF9, "unused-f9", F10x, I::None),
    op(0xFA, "invoke-polymorphic", F45cc, I::MethodAndProto),
    op(0xFB, "invoke-polymorphic/range", F4rcc, I::MethodAndProto),
    op(0xFC, "invoke-custom", F35c, I::CallSite),
    op(0xFD, "invoke-custom/range", F3rc, I::CallSite),
    op(0xFE, "const-method-handle", F21c, I::MethodHandle),
    op(0xFF, "const-method-type", F21c, I::Proto),
];

/// The first code unit of a `packed-switch` payload.
pub const PACKED_SWITCH_SIGNATURE: u16 = 0x0100;
/// The first code unit of a `sparse-switch` payload.
pub const SPARSE_SWITCH_SIGNATURE: u16 = 0x0200;
/// The first code unit of a `fill-array-data` payload.
pub const ARRAY_DATA_SIGNATURE: u16 = 0x0300;

/// What an instruction is: an operation, or a payload (which is in the
/// instruction stream, a `nop` with data).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// An operation.
    Op,
    /// A `packed-switch` payload.
    PackedSwitchPayload,
    /// A `sparse-switch` payload.
    SparseSwitchPayload,
    /// A `fill-array-data` payload.
    FillArrayDataPayload,
}

/// An instruction, its operands as ART names them: `a` ... `h`, wide
/// `b` for `const-wide` (51l), and the argument registers of the 35c-like
/// formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instruction {
    /// Its address (in code units, from the code's start).
    pub address: u32,
    /// Its opcode.
    pub opcode: u8,
    /// An operation or a payload.
    pub kind: Kind,
    /// How many code units it takes (a payload's, all of it).
    pub width: u32,
    /// `vA` (a register, a count, a literal, a branch offset: see the
    /// format).
    pub a: u32,
    /// `vB` (for 51l, all 64 bits).
    pub b: u64,
    /// `vC`.
    pub c: u32,
    /// `vH` (45cc, 4rcc: the prototype).
    pub h: u32,
    /// 35c, 35ms, 35mi, 45cc: the argument registers (`vC` `vD` `vE` `vF`
    /// `vG`), the first `a` of them meant.
    pub args: [u8; 5],
}

impl Instruction {
    /// Its opcode's information.
    pub fn info(&self) -> &'static OpInfo {
        &OPCODES[usize::from(self.opcode)]
    }

    /// Its format.
    pub fn format(&self) -> Format {
        self.info().format
    }

    /// The argument registers of a 35c-like instruction (none for more
    /// than five, which isn't valid).
    pub fn arg_registers(&self) -> &[u8] {
        self.args.get(..self.a as usize).unwrap_or_default()
    }

    /// `b` as the 32-bit value it is outside 51l.
    pub fn b32(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let b = self.b as u32;
        b
    }

    /// The index it refers to (a string, type, field, method, call site,
    /// method handle, or proto index), if it has one.
    pub fn index(&self) -> Option<u32> {
        if self.kind != Kind::Op {
            return None;
        }
        match self.info().index {
            IndexType::None => None,
            _ => Some(match self.format() {
                F22c | F22cs => self.c,
                _ => self.b32(),
            }),
        }
    }

    /// A branch's or payload reference's target address (`goto`, `if-*`,
    /// `packed-switch`, `sparse-switch`, `fill-array-data`).
    pub fn target(&self) -> Option<u32> {
        if self.kind != Kind::Op {
            return None;
        }
        #[allow(clippy::cast_possible_wrap)]
        let off = match self.format() {
            F10t | F20t | F30t => self.a as i32,
            F21t | F31t => self.b32() as i32,
            F22t => self.c as i32,
            _ => return None,
        };
        Some(self.address.wrapping_add_signed(off))
    }
}

/// A payload's contents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Payload<'d> {
    /// `packed-switch`: consecutive keys from `first_key`, each with its
    /// target (relative to the switch).
    PackedSwitch {
        /// The first key.
        first_key: i32,
        /// The targets.
        targets: Vec<i32>,
    },
    /// `sparse-switch`: keys, sorted, each with its target.
    SparseSwitch {
        /// The keys.
        keys: Vec<i32>,
        /// The targets.
        targets: Vec<i32>,
    },
    /// `fill-array-data`: the elements' width and count, and their bytes.
    FillArrayData {
        /// Each element's size in bytes.
        element_width: u16,
        /// How many elements.
        size: u32,
        /// Their bytes (`element_width * size`).
        data: &'d [u8],
    },
}

fn unit(insns: &[u8], i: usize) -> Option<u16> {
    crate::reader::u16_at(insns, i.checked_mul(2)?)
}

/// How many code units the instruction at `pc` takes, if it's all there
/// to say (a payload's size is in it).
fn width_at(insns: &[u8], pc: usize) -> Option<(u32, Kind)> {
    let u0 = unit(insns, pc)?;
    let opcode = (u0 & 0xFF) as u8;
    if opcode == 0 {
        match u0 {
            PACKED_SWITCH_SIGNATURE => {
                let size = u64::from(unit(insns, pc + 1)?);
                return Some((u32::try_from(size * 2 + 4).ok()?, Kind::PackedSwitchPayload));
            }
            SPARSE_SWITCH_SIGNATURE => {
                let size = u64::from(unit(insns, pc + 1)?);
                return Some((u32::try_from(size * 4 + 2).ok()?, Kind::SparseSwitchPayload));
            }
            ARRAY_DATA_SIGNATURE => {
                let width = u64::from(unit(insns, pc + 1)?);
                let size =
                    u64::from(unit(insns, pc + 2)?) | (u64::from(unit(insns, pc + 3)?) << 16);
                let units = (width * size).div_ceil(2) + 4;
                return Some((u32::try_from(units).ok()?, Kind::FillArrayDataPayload));
            }
            _ => {}
        }
    }
    Some((OPCODES[usize::from(opcode)].format.units(), Kind::Op))
}

/// Decodes the instruction at `pc` (in code units) of `insns`.
pub fn decode_at(insns: &[u8], pc: u32) -> Result<Instruction> {
    let p = pc as usize;
    let err = |what| Error::new(ErrorKind::Truncated, u64::from(pc) * 2, what);
    let (width, kind) = width_at(insns, p).ok_or_else(|| err("instruction"))?;
    let total = insns.len() / 2;
    if (p as u64) + u64::from(width) > total as u64 {
        return Err(err("instruction"));
    }
    let u = |i: usize| unit(insns, p + i).unwrap_or(0);
    let u0 = u(0);
    let opcode = (u0 & 0xFF) as u8;
    let mut insn = Instruction {
        address: pc,
        opcode,
        kind,
        width,
        a: 0,
        b: 0,
        c: 0,
        h: 0,
        args: [0; 5],
    };
    if kind != Kind::Op {
        return Ok(insn);
    }
    decode_operands(&mut insn, OPCODES[usize::from(opcode)].format, &u);
    Ok(insn)
}

/// Fills in `insn`'s operands as `format` gives them, from its code units
/// (`u(0)` the first).
pub fn decode_operands(insn: &mut Instruction, format: Format, u: &dyn Fn(usize) -> u16) {
    let u0 = u(0);
    let aa = u32::from(u0 >> 8);
    let a4 = u32::from((u0 >> 8) & 0xF);
    let b4 = u32::from(u0 >> 12);
    let wide32 = |i: usize| u32::from(u(i)) | (u32::from(u(i + 1)) << 16);
    #[allow(clippy::cast_sign_loss)]
    let sext16 = |v: u16| i32::from(v as i16) as u32;
    match format {
        F10x => {}
        F12x => {
            insn.a = a4;
            insn.b = u64::from(b4);
        }
        F11n => {
            insn.a = a4;
            // A four-bit signed literal.
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
            let lit = (((b4 << 28) as i32) >> 28) as u32;
            insn.b = u64::from(lit);
        }
        F11x => insn.a = aa,
        F10t => {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let off = i32::from((u0 >> 8) as u8 as i8) as u32;
            insn.a = off;
        }
        F20t => insn.a = sext16(u(1)),
        F20bc | F22x | F21c | F21h => {
            insn.a = aa;
            insn.b = u64::from(u(1));
        }
        F21t | F21s => {
            insn.a = aa;
            insn.b = u64::from(sext16(u(1)));
        }
        F23x => {
            insn.a = aa;
            insn.b = u64::from(u(1) & 0xFF);
            insn.c = u32::from(u(1) >> 8);
        }
        F22b => {
            insn.a = aa;
            insn.b = u64::from(u(1) & 0xFF);
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let lit = i32::from((u(1) >> 8) as u8 as i8) as u32;
            insn.c = lit;
        }
        F22t | F22s => {
            insn.a = a4;
            insn.b = u64::from(b4);
            insn.c = sext16(u(1));
        }
        F22c | F22cs => {
            insn.a = a4;
            insn.b = u64::from(b4);
            insn.c = u32::from(u(1));
        }
        F30t => insn.a = wide32(1),
        F32x => {
            insn.a = u32::from(u(1));
            insn.b = u64::from(u(2));
        }
        F31i | F31t | F31c => {
            insn.a = aa;
            insn.b = u64::from(wide32(1));
        }
        F35c | F35ms | F35mi | F45cc => {
            insn.a = b4;
            insn.b = u64::from(u(1));
            let regs = u(2);
            #[allow(clippy::cast_possible_truncation)]
            {
                insn.args = [
                    (regs & 0xF) as u8,
                    ((regs >> 4) & 0xF) as u8,
                    ((regs >> 8) & 0xF) as u8,
                    (regs >> 12) as u8,
                    a4 as u8,
                ];
            }
            insn.c = u32::from(regs & 0xF);
            if format == F45cc {
                insn.h = u32::from(u(3));
            }
        }
        F3rc | F3rms | F3rmi | F4rcc => {
            insn.a = aa;
            insn.b = u64::from(u(1));
            insn.c = u32::from(u(2));
            if format == F4rcc {
                insn.h = u32::from(u(3));
            }
        }
        F51l => {
            insn.a = aa;
            insn.b = u64::from(wide32(1)) | (u64::from(wide32(3)) << 32);
        }
    }
}

/// The instructions of `insns` (a code item's), in order; an error for an
/// instruction that runs past the end, and none after it.
pub fn decode(insns: &[u8]) -> Decoder<'_> {
    Decoder {
        insns,
        pc: 0,
        done: false,
    }
}

/// The instructions of a method's code (see [`decode`]).
#[derive(Clone, Debug)]
pub struct Decoder<'d> {
    insns: &'d [u8],
    pc: u32,
    done: bool,
}

impl Iterator for Decoder<'_> {
    type Item = Result<Instruction>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pc as usize >= self.insns.len() / 2 {
            return None;
        }
        match decode_at(self.insns, self.pc) {
            Ok(i) => {
                self.pc = self.pc.saturating_add(i.width);
                Some(Ok(i))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// The payload of `insn`, a payload pseudo-instruction of `insns`.
pub fn payload<'d>(insns: &'d [u8], insn: &Instruction) -> Result<Payload<'d>> {
    let p = insn.address as usize;
    let bad = || Error::new(ErrorKind::Truncated, u64::from(insn.address) * 2, "payload");
    let u = |i: usize| unit(insns, p + i).ok_or_else(bad);
    #[allow(clippy::cast_possible_wrap)]
    let w =
        |i: usize| -> Result<i32> { Ok((u32::from(u(i)?) | (u32::from(u(i + 1)?) << 16)) as i32) };
    match insn.kind {
        Kind::Op => Err(Error::new(
            ErrorKind::Malformed,
            u64::from(insn.address) * 2,
            "not a payload",
        )),
        Kind::PackedSwitchPayload => {
            let n = usize::from(u(1)?);
            let first_key = w(2)?;
            let targets = (0..n).map(|i| w(4 + 2 * i)).collect::<Result<_>>()?;
            Ok(Payload::PackedSwitch { first_key, targets })
        }
        Kind::SparseSwitchPayload => {
            let n = usize::from(u(1)?);
            let keys = (0..n).map(|i| w(2 + 2 * i)).collect::<Result<_>>()?;
            let targets = (0..n)
                .map(|i| w(2 + 2 * n + 2 * i))
                .collect::<Result<_>>()?;
            Ok(Payload::SparseSwitch { keys, targets })
        }
        Kind::FillArrayDataPayload => {
            let element_width = u(1)?;
            let size = u32::from(u(2)?) | (u32::from(u(3)?) << 16);
            let len = usize::from(element_width)
                .checked_mul(size as usize)
                .ok_or_else(bad)?;
            let start = (p + 4) * 2;
            let data = insns
                .get(start..start.checked_add(len).ok_or_else(bad)?)
                .ok_or_else(bad)?;
            Ok(Payload::FillArrayData {
                element_width,
                size,
                data,
            })
        }
    }
}

/// The payload a `packed-switch`, `sparse-switch`, or `fill-array-data`
/// instruction refers to.
pub fn payload_of<'d>(insns: &'d [u8], insn: &Instruction) -> Result<Payload<'d>> {
    let bad = |what| Error::new(ErrorKind::Malformed, u64::from(insn.address) * 2, what);
    let want = match insn.opcode {
        0x26 => Kind::FillArrayDataPayload,
        0x2B => Kind::PackedSwitchPayload,
        0x2C => Kind::SparseSwitchPayload,
        _ => return Err(bad("no payload")),
    };
    let target = insn.target().ok_or_else(|| bad("no payload"))?;
    let p = decode_at(insns, target)?;
    if p.kind != want {
        return Err(bad("payload kind"));
    }
    payload(insns, &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn table_is_by_value() {
        for (i, o) in OPCODES.iter().enumerate() {
            assert_eq!(usize::from(o.value), i, "{}", o.name);
        }
        let names: std::collections::HashSet<_> = OPCODES.iter().map(|o| o.name).collect();
        assert_eq!(names.len(), 256);
    }

    #[test]
    fn operands_of_each_format() {
        // const/4 v1, #-1
        let i = decode_at(&bytes(&[0xF112]), 0).unwrap();
        assert_eq!((i.a, i.b32()), (1, u32::MAX));
        // goto -2
        let i = decode_at(&bytes(&[0x0000, 0xFE28]), 1).unwrap();
        assert_eq!(i.target(), Some(u32::MAX));
        // if-ne v0, v1, +14
        let i = decode_at(&bytes(&[0x1033, 0x000E]), 0).unwrap();
        assert_eq!((i.a, i.b, i.c, i.target()), (0, 1, 14, Some(14)));
        // add-int/lit8 v4, v4, #-1
        let i = decode_at(&bytes(&[0x04D8, 0xFF04]), 0).unwrap();
        assert_eq!((i.a, i.b, i.c), (4, 4, u32::MAX));
        // invoke-virtual {v0, v4, v5, v6, v7}, method@0021
        let i = decode_at(&bytes(&[0x576E, 0x0021, 0x6540]), 0).unwrap();
        assert_eq!(
            (i.a, i.b, i.arg_registers()),
            (5, 0x21, &[0, 4, 5, 6, 7][..])
        );
        assert_eq!(i.index(), Some(0x21));
        // invoke-polymorphic {v1, v0}, method@0023, proto@0001
        let i = decode_at(&bytes(&[0x20FA, 0x0023, 0x0001, 0x0001]), 0).unwrap();
        assert_eq!(
            (i.a, i.arg_registers(), i.b, i.h),
            (2, &[1, 0][..], 0x23, 1)
        );
        // filled-new-array/range {v0 .. v6}, type@0059
        let i = decode_at(&bytes(&[0x0725, 0x0059, 0x0000]), 0).unwrap();
        assert_eq!((i.a, i.b, i.c), (7, 0x59, 0));
        // const-wide v11, #0x0123456789abcdef
        let i = decode_at(&bytes(&[0x0B18, 0xCDEF, 0x89AB, 0x4567, 0x0123]), 0).unwrap();
        assert_eq!((i.a, i.b), (11, 0x0123_4567_89AB_CDEF));
        // goto/32 and move/16
        let i = decode_at(&bytes(&[0x002A, 0x0000, 0x8000]), 0).unwrap();
        assert_eq!(i.target(), Some(0x8000_0000));
        let i = decode_at(&bytes(&[0x0003, 0x1234, 0x5678]), 0).unwrap();
        assert_eq!((i.a, i.b), (0x1234, 0x5678));
        // iget v0, v2, field@0000 (22c's index is C)
        let i = decode_at(&bytes(&[0x2052, 0x0007]), 0).unwrap();
        assert_eq!((i.a, i.b, i.index()), (0, 2, Some(7)));
    }

    #[test]
    fn payloads() {
        // packed-switch v2, +3; the payload: 2 keys from 10.
        let code = bytes(&[0x022B, 3, 0, 0x0100, 2, 10, 0, 5, 0, 6, 0]);
        let all: Vec<_> = decode(&code).collect::<Result<_>>().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].kind, all[1].width), (Kind::PackedSwitchPayload, 8));
        assert_eq!(
            payload_of(&code, &all[0]).unwrap(),
            Payload::PackedSwitch {
                first_key: 10,
                targets: vec![5, 6]
            }
        );
        let code = bytes(&[0x0200, 1, 7, 0, 9, 0]);
        let p = decode_at(&code, 0).unwrap();
        assert_eq!(
            payload(&code, &p).unwrap(),
            Payload::SparseSwitch {
                keys: vec![7],
                targets: vec![9]
            }
        );
        let code = bytes(&[0x0300, 1, 3, 0, 0x0201, 0x0003]);
        let p = decode_at(&code, 0).unwrap();
        assert_eq!(p.width, 6);
        assert_eq!(
            payload(&code, &p).unwrap(),
            Payload::FillArrayData {
                element_width: 1,
                size: 3,
                data: &[1, 2, 3]
            }
        );
        // Truncated: the size says more than there is.
        assert!(decode_at(&bytes(&[0x0300, 4, 0xFFFF, 0xFFFF]), 0).is_err());
        let code = bytes(&[0x0E00, 0x0014, 0x0001]);
        let mut d = decode(&code);
        assert!(d.next().unwrap().is_ok());
        assert!(d.next().unwrap().is_err());
        assert!(d.next().is_none());
        // An odd byte at the end is no code unit.
        assert_eq!(decode(&[0x0E, 0x00, 0x0E]).count(), 1);
    }
}
