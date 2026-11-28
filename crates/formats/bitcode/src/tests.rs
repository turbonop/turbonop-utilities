//! Unit tests: the bit reader, abbreviations of every kind, `BLOCKINFO`,
//! blocks, records; each IR block and record kind, on streams written
//! here; and hostile inputs (every fixture truncated and corrupted).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::bitstream::{Abbrev, AbbrevOp, BitReader, Cursor, Entry, Stream, StreamKind, char6};
use crate::dump::{DumpOptions, analyze};
use crate::function::{Callee, Opcode, Operand};
use crate::ir::{
    AliasKind, Attribute, Bitcode, ConstantKind, Linkage, ThreadLocal, Visibility, sign_rotated,
};
use crate::metadata::MetadataNode;
use crate::symtab::{Symtab, flags};
use crate::tables::*;
use crate::types::{Type, TypeTable, llvm_name};

/// A bitstream writer, as LLVM's `BitstreamWriter` writes.
struct W {
    bytes: Vec<u8>,
    bit: u64,
    width: u32,
    /// Open blocks: the length word's byte offset, and the parent's width.
    open: Vec<(usize, u32)>,
}

impl W {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            bit: 0,
            width: 2,
            open: Vec::new(),
        }
    }

    /// A stream with LLVM IR's magic.
    fn ir() -> Self {
        let mut w = Self::new();
        for b in *b"BC\xC0\xDE" {
            w.emit(u64::from(b), 8);
        }
        w
    }

    fn emit(&mut self, v: u64, n: u32) {
        for i in 0..n {
            let byte = usize::try_from(self.bit / 8).unwrap();
            if byte >= self.bytes.len() {
                self.bytes.push(0);
            }
            if (v >> i) & 1 != 0 {
                self.bytes[byte] |= 1 << (self.bit % 8);
            }
            self.bit += 1;
        }
    }

    fn vbr(&mut self, mut v: u64, n: u32) {
        let hi = 1u64 << (n - 1);
        while v >= hi {
            self.emit((v & (hi - 1)) | hi, n);
            v >>= n - 1;
        }
        self.emit(v, n);
    }

    fn align(&mut self) {
        while !self.bit.is_multiple_of(32) {
            self.emit(0, 1);
        }
    }

    fn enter(&mut self, id: u32, width: u32) {
        self.emit(u64::from(ENTER_SUBBLOCK), self.width);
        self.vbr(u64::from(id), 8);
        self.vbr(u64::from(width), 4);
        self.align();
        let at = self.bytes.len();
        self.emit(0, 32);
        self.open.push((at, self.width));
        self.width = width;
    }

    fn end(&mut self) {
        self.emit(u64::from(END_BLOCK), self.width);
        self.align();
        let (at, width) = self.open.pop().unwrap();
        let words = u32::try_from((self.bytes.len() - at - 4) / 4).unwrap();
        self.bytes[at..at + 4].copy_from_slice(&words.to_le_bytes());
        self.width = width;
    }

    /// An unabbreviated record.
    fn record(&mut self, code: u32, ops: &[u64]) {
        self.emit(u64::from(UNABBREV_RECORD), self.width);
        self.vbr(u64::from(code), 6);
        self.vbr(ops.len() as u64, 6);
        for &o in ops {
            self.vbr(o, 6);
        }
    }

    /// A record of characters.
    fn text(&mut self, code: u32, s: &[u8]) {
        let ops: Vec<u64> = s.iter().map(|&c| u64::from(c)).collect();
        self.record(code, &ops);
    }

    /// `DEFINE_ABBREV` of `ops`.
    fn define(&mut self, ops: &[AbbrevOp]) {
        self.emit(u64::from(DEFINE_ABBREV), self.width);
        self.vbr(ops.len() as u64, 5);
        for op in ops {
            let (encoding, data) = match *op {
                AbbrevOp::Literal(v) => {
                    self.emit(1, 1);
                    self.vbr(v, 8);
                    continue;
                }
                AbbrevOp::Fixed(n) => (1, Some(n)),
                AbbrevOp::Vbr(n) => (2, Some(n)),
                AbbrevOp::Array => (3, None),
                AbbrevOp::Char6 => (4, None),
                AbbrevOp::Blob => (5, None),
            };
            self.emit(0, 1);
            self.emit(encoding, 3);
            if let Some(n) = data {
                self.vbr(u64::from(n), 5);
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        assert!(self.open.is_empty());
        self.align();
        self.bytes
    }
}

fn char6_code(c: u8) -> u64 {
    (0..64).find(|&v| char6(v) == c).unwrap()
}

// The bit reader.

#[test]
fn bits_and_vbrs() {
    let mut w = W::new();
    w.emit(5, 3);
    w.emit(0x1234_5678_9ABC_DEF0, 64);
    w.emit(1, 1);
    w.vbr(0, 6);
    w.vbr(31, 6);
    w.vbr(32, 6);
    w.vbr(u64::MAX, 6);
    w.vbr(1_000_000, 4);
    let data = w.finish();
    let mut r = BitReader::new(&data);
    assert_eq!(r.read(3).unwrap(), 5);
    assert_eq!(r.read(64).unwrap(), 0x1234_5678_9ABC_DEF0);
    assert_eq!(r.read(1).unwrap(), 1);
    assert_eq!(r.vbr(6).unwrap(), 0);
    assert_eq!(r.vbr(6).unwrap(), 31);
    assert_eq!(r.vbr(6).unwrap(), 32);
    assert_eq!(r.vbr(6).unwrap(), u64::MAX);
    assert_eq!(r.vbr32(4).unwrap(), 1_000_000);
    assert_eq!(r.read(0).unwrap(), 0);
    r.align32();
    assert!(r.at_end());
    assert!(r.read(1).is_err());
    assert!(r.seek(r.len_bits() + 1).is_err());
}

#[test]
fn unterminated_vbrs() {
    // Every chunk continues: 64 bits of value and more.
    let data = [0xFF; 32];
    assert!(BitReader::new(&data).vbr(6).is_err());
    assert!(BitReader::new(&data).vbr32(6).is_err());
    assert!(BitReader::new(&data[..2]).vbr(6).is_err());
    assert!(BitReader::new(&data).vbr(0).is_err());
    assert!(BitReader::new(&data).vbr(33).is_err());
    assert!(BitReader::new(&data).read(65).is_err());
}

#[test]
fn char6_table() {
    let all: Vec<u8> = (0..64).map(char6).collect();
    assert_eq!(
        &all,
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._"
    );
}

// Abbreviations and records.

#[test]
fn abbreviated_records_of_every_kind() {
    let mut w = W::new();
    w.enter(8, 4);
    w.define(&[
        AbbrevOp::Literal(7),
        AbbrevOp::Fixed(3),
        AbbrevOp::Vbr(4),
        AbbrevOp::Char6,
        AbbrevOp::Literal(99),
        AbbrevOp::Array,
        AbbrevOp::Fixed(8),
    ]);
    w.define(&[AbbrevOp::Fixed(5), AbbrevOp::Array, AbbrevOp::Char6]);
    w.define(&[AbbrevOp::Literal(1), AbbrevOp::Vbr(6), AbbrevOp::Blob]);
    w.define(&[AbbrevOp::Literal(2), AbbrevOp::Array, AbbrevOp::Vbr(3)]);
    // Abbreviation 4: code 7, fixed 6, VBR 300, char6 'Z', literal, [1, 2, 255].
    w.emit(4, 4);
    w.emit(6, 3);
    w.vbr(300, 4);
    w.emit(char6_code(b'Z'), 6);
    w.vbr(3, 6);
    for v in [1, 2, 255] {
        w.emit(v, 8);
    }
    // Abbreviation 5: code 17 (fixed), char6 "a.b_9".
    w.emit(5, 4);
    w.emit(17, 5);
    w.vbr(5, 6);
    for c in *b"a.b_9" {
        w.emit(char6_code(c), 6);
    }
    // Abbreviation 6: code 1, VBR 1234, blob "hello" (aligned, padded).
    w.emit(6, 4);
    w.vbr(1234, 6);
    w.vbr(5, 6);
    w.align();
    for c in *b"hello" {
        w.emit(u64::from(c), 8);
    }
    w.align();
    // Abbreviation 7: an empty array.
    w.emit(7, 4);
    w.vbr(0, 6);
    w.record(42, &[1, 2, u64::MAX]);
    w.end();
    let data = w.finish();
    let mut c = Cursor::new(&data, 0).unwrap();
    assert_eq!(c.advance().unwrap(), Entry::SubBlock(8));
    let h = c.enter_block(8).unwrap();
    assert_eq!(h.abbrev_width, 4);
    assert_eq!(h.end(), data.len() as u64 * 8);
    assert_eq!(c.block_id(), Some(8));
    let mut ops = Vec::new();
    fn next<'d>(c: &mut Cursor<'d>, ops: &mut Vec<u64>) -> crate::bitstream::RecordHead<'d> {
        ops.clear();
        let Entry::Record(a) = c.advance().unwrap() else {
            panic!()
        };
        c.read_record(a, ops).unwrap()
    }
    let r = next(&mut c, &mut ops);
    assert_eq!((r.code, r.blob), (7, None));
    assert_eq!(ops, [6, 300, u64::from(b'Z'), 99, 1, 2, 255]);
    let r = next(&mut c, &mut ops);
    assert_eq!(r.code, 17);
    assert_eq!(
        ops,
        b"a.b_9".iter().map(|&b| u64::from(b)).collect::<Vec<_>>()
    );
    let r = next(&mut c, &mut ops);
    assert_eq!((r.code, r.blob), (1, Some(&b"hello"[..])));
    assert_eq!(ops, [1234]);
    let r = next(&mut c, &mut ops);
    assert_eq!(r.code, 2);
    assert!(ops.is_empty());
    let r = next(&mut c, &mut ops);
    assert_eq!(r.code, 42);
    assert_eq!(ops, [1, 2, u64::MAX]);
    assert_eq!(c.advance().unwrap(), Entry::EndBlock);
    assert!(c.at_end());
    assert_eq!(c.depth(), 0);
}

fn abbrev_of(f: impl FnOnce(&mut W)) -> Result<Abbrev, turbonop_core::Error> {
    let mut w = W::new();
    f(&mut w);
    let data = w.finish();
    Abbrev::read(&mut BitReader::new(&data))
}

#[test]
fn abbreviation_definitions() {
    // Fixed(0) and VBR(0) are literal zeros.
    let a = abbrev_of(|w| {
        w.vbr(2, 5);
        w.emit(0, 1);
        w.emit(1, 3);
        w.vbr(0, 5);
        w.emit(0, 1);
        w.emit(2, 3);
        w.vbr(0, 5);
    })
    .unwrap();
    assert_eq!(a.ops, [AbbrevOp::Literal(0), AbbrevOp::Literal(0)]);
    // Too wide a field.
    let wide = abbrev_of(|w| {
        w.vbr(1, 5);
        w.emit(0, 1);
        w.emit(1, 3);
        w.vbr(33, 5);
    });
    assert!(wide.is_err());
    // An encoding that doesn't exist.
    let bad = abbrev_of(|w| {
        w.vbr(1, 5);
        w.emit(0, 1);
        w.emit(7, 3);
    });
    assert!(bad.is_err());
    // No operands; truncated.
    assert!(abbrev_of(|w| w.vbr(0, 5)).is_err());
    assert!(abbrev_of(|w| w.vbr(3, 5)).is_err());
}

#[test]
fn malformed_records() {
    // An array that isn't second to last; an array of arrays; a blob past
    // the end; an abbreviation that starts with an array; an unknown ID.
    let cases: [&dyn Fn(&mut W); 5] = [
        &|w| {
            w.define(&[
                AbbrevOp::Literal(1),
                AbbrevOp::Array,
                AbbrevOp::Fixed(1),
                AbbrevOp::Fixed(1),
            ]);
            w.emit(4, 3);
            w.vbr(0, 6);
        },
        &|w| {
            w.define(&[AbbrevOp::Literal(1), AbbrevOp::Array, AbbrevOp::Array]);
            w.emit(4, 3);
            w.vbr(1, 6);
        },
        &|w| {
            w.define(&[AbbrevOp::Literal(1), AbbrevOp::Blob]);
            w.emit(4, 3);
            w.vbr(1000, 6);
        },
        &|w| {
            w.define(&[AbbrevOp::Array, AbbrevOp::Fixed(1)]);
            w.emit(4, 3);
        },
        &|w| w.emit(7, 3),
    ];
    for (i, case) in cases.iter().enumerate() {
        let mut w = W::new();
        w.enter(8, 3);
        case(&mut w);
        w.emit(0, 64);
        w.open.clear();
        let data = w.finish();
        let mut c = Cursor::new(&data, 0).unwrap();
        c.advance().unwrap();
        c.enter_block(8).unwrap();
        let mut ops = Vec::new();
        let r = c.advance().and_then(|e| match e {
            Entry::Record(a) => c.read_record(a, &mut ops).map(|_| ()),
            _ => Ok(()),
        });
        assert!(r.is_err(), "case {i}");
    }
}

#[test]
fn block_info_applies_to_later_blocks() {
    let mut w = W::new();
    w.enter(BLOCKINFO_BLOCK_ID, 2);
    w.record(BLOCKINFO_CODE_SETBID, &[9]);
    w.define(&[AbbrevOp::Literal(5), AbbrevOp::Fixed(4)]);
    w.text(BLOCKINFO_CODE_BLOCKNAME, b"NINE");
    w.record(
        BLOCKINFO_CODE_SETRECORDNAME,
        &[5, u64::from(b'F'), u64::from(b'V')],
    );
    w.end();
    w.enter(9, 3);
    // Abbreviation 4 is BLOCKINFO's.
    w.emit(4, 3);
    w.emit(11, 4);
    // A nested block, skipped.
    w.enter(10, 2);
    w.record(1, &[1]);
    w.end();
    w.end();
    let data = w.finish();
    let mut c = Cursor::new(&data, 0).unwrap();
    assert_eq!(c.advance().unwrap(), Entry::SubBlock(0));
    let info = c.read_block_info().unwrap();
    assert_eq!(info.block_name(9), Some(&b"NINE"[..]));
    assert_eq!(info.record_name(9, 5), Some(&b"FV"[..]));
    assert_eq!(info.get(9).unwrap().abbrevs.len(), 1);
    c.set_block_info(alloc::sync::Arc::new(info));
    assert_eq!(c.advance().unwrap(), Entry::SubBlock(9));
    c.enter_block(9).unwrap();
    let mut ops = Vec::new();
    let Entry::Record(a) = c.advance().unwrap() else {
        panic!()
    };
    assert_eq!(a, 4);
    let r = c.read_record(a, &mut ops).unwrap();
    assert_eq!((r.code, ops.as_slice()), (5, &[11][..]));
    assert_eq!(c.advance().unwrap(), Entry::SubBlock(10));
    c.skip_block().unwrap();
    assert_eq!(c.advance().unwrap(), Entry::EndBlock);

    // The dump names them (in a stream of another kind).
    let mut out = Vec::new();
    let mut stream = b"XYZW".to_vec();
    stream.extend_from_slice(&data);
    analyze(&stream, &DumpOptions::default(), None, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("<BLOCKINFO_BLOCK/>\n<NINE NumWords="),
        "{text}"
    );
    assert!(text.contains("  <FV abbrevid=4 op0=11/>"), "{text}");
    assert!(
        text.contains("  <UnknownBlock10 NumWords=1 BlockCodeSize=2>"),
        "{text}"
    );
    assert!(text.contains("Stream type: unknown"), "{text}");
}

#[test]
fn malformed_block_info() {
    let cases: [fn(&mut W); 3] = [
        // An abbreviation, or a name, before SETBID; an empty SETBID.
        |w| w.define(&[AbbrevOp::Literal(1)]),
        |w| w.text(BLOCKINFO_CODE_BLOCKNAME, b"X"),
        |w| w.record(BLOCKINFO_CODE_SETBID, &[]),
    ];
    for f in cases {
        let mut w = W::new();
        w.enter(BLOCKINFO_BLOCK_ID, 2);
        f(&mut w);
        w.end();
        let data = w.finish();
        let mut c = Cursor::new(&data, 0).unwrap();
        c.advance().unwrap();
        assert!(c.read_block_info().is_err());
    }
}

#[test]
fn blocks_and_their_ends() {
    // END_BLOCK at the top level.
    let mut w = W::new();
    w.emit(0, 2);
    let data = w.finish();
    assert!(Cursor::new(&data, 0).unwrap().advance().is_err());
    // A zero abbreviation width, and one too wide.
    for width in [0, 33] {
        let mut w = W::new();
        w.emit(u64::from(ENTER_SUBBLOCK), 2);
        w.vbr(8, 8);
        w.vbr(width, 4);
        w.align();
        w.emit(1, 32);
        w.emit(0, 32);
        let data = w.finish();
        let mut c = Cursor::new(&data, 0).unwrap();
        c.advance().unwrap();
        assert!(c.enter_block(8).is_err());
    }
    // A block longer than the stream.
    let mut w = W::new();
    w.enter(8, 3);
    w.end();
    let mut data = w.finish();
    data[4] = 200;
    let mut c = Cursor::new(&data, 0).unwrap();
    c.advance().unwrap();
    assert!(c.skip_block().is_err());
    // A block that never ends.
    let mut w = W::new();
    w.enter(8, 3);
    w.record(1, &[]);
    w.open.clear();
    let data = w.finish();
    let mut c = Cursor::new(&data, 0).unwrap();
    c.advance().unwrap();
    c.enter_block(8).unwrap();
    let mut ops = Vec::new();
    let Entry::Record(a) = c.advance().unwrap() else {
        panic!()
    };
    c.read_record(a, &mut ops).unwrap();
    // Padding reads as END_BLOCK (code 0): the block ends, the stream
    // doesn't hold more.
    assert_eq!(c.advance().unwrap(), Entry::EndBlock);
    assert!(c.at_end());
}

#[test]
fn streams_and_wrappers() {
    for (magic, kind) in [
        (*b"BC\xC0\xDE", StreamKind::LlvmIr),
        (*b"DIAG", StreamKind::ClangDiagnostics),
        (*b"RMRK", StreamKind::Remarks),
        (*b"CPCH", StreamKind::ClangAst),
        (*b"ABCD", StreamKind::Unknown),
    ] {
        let s = Stream::new(&magic).unwrap();
        assert_eq!((s.kind, s.magic), (kind, magic));
        assert!(s.top_blocks().is_empty());
    }
    assert!(Stream::new(b"BC").is_err());
    let mut wrapped = Vec::new();
    for v in [0x0B17_C0DE_u32, 0, 20, 4, 7] {
        wrapped.extend_from_slice(&v.to_le_bytes());
    }
    wrapped.extend_from_slice(b"BC\xC0\xDE");
    let s = Stream::new(&wrapped).unwrap();
    assert_eq!(s.kind, StreamKind::LlvmIr);
    assert_eq!(s.offset, 20);
    assert_eq!(s.wrapper.unwrap().cpu_type, 7);
    // A wrapper that claims more than there is.
    wrapped[12] = 8;
    assert!(Stream::new(&wrapped).is_err());
    assert!(Stream::new(&wrapped[..12]).is_err());
    assert!(Bitcode::parse(b"BC\xC0\xDE").is_err());
    assert!(Bitcode::parse(b"DIAG\0\0\0\0").is_err());
}

#[test]
fn sign_rotation() {
    assert_eq!(sign_rotated(0), 0);
    assert_eq!(sign_rotated(2), 1);
    assert_eq!(sign_rotated(3), -1);
    assert_eq!(sign_rotated(1), i64::MIN);
    assert_eq!(sign_rotated(u64::MAX), -i64::MAX);
}

#[test]
fn names_as_llvm_writes_them() {
    assert_eq!(llvm_name('@', b"main"), "@main");
    assert_eq!(llvm_name('@', b"a.b$c-d_e"), "@a.b$c-d_e");
    assert_eq!(llvm_name('@', b"0x"), "@\"0x\"");
    assert_eq!(llvm_name('%', b"has space"), "%\"has space\"");
    assert_eq!(llvm_name('@', b"q\"\\\x01"), "@\"q\\22\\5C\\01\"");
}

// The IR, on modules written here.

/// A module (IDENTIFICATION, then MODULE: version, triple, data layout,
/// a type table of `types`, then `rest`).
fn module(version: u64, types: &[(u32, Vec<u64>)], rest: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::ir();
    w.enter(IDENTIFICATION_BLOCK_ID, 5);
    w.text(IDENTIFICATION_CODE_STRING, b"test");
    w.record(IDENTIFICATION_CODE_EPOCH, &[0]);
    w.end();
    w.enter(MODULE_BLOCK_ID, 3);
    w.record(MODULE_CODE_VERSION, &[version]);
    w.text(MODULE_CODE_TRIPLE, b"x86_64-unknown-linux-gnu");
    w.text(MODULE_CODE_DATALAYOUT, b"e-A5-P3");
    w.enter(TYPE_BLOCK_ID_NEW, 4);
    w.record(TYPE_CODE_NUMENTRY, &[types.len() as u64]);
    for (code, ops) in types {
        w.record(*code, ops);
    }
    w.end();
    rest(&mut w);
    w.end();
    w.finish()
}

fn text_ops(s: &[u8]) -> Vec<u64> {
    s.iter().map(|&b| u64::from(b)).collect()
}

#[test]
fn types_of_every_kind() {
    let data = module(
        2,
        &[
            (TYPE_CODE_INTEGER, vec![32]),
            (TYPE_CODE_OPAQUE_POINTER, vec![0]),
            (TYPE_CODE_OPAQUE_POINTER, vec![3]),
            (TYPE_CODE_FUNCTION, vec![1, 0, 1]),
            (TYPE_CODE_STRUCT_NAME, text_ops(b"struct.S")),
            // 4, which refers forward to 6.
            (TYPE_CODE_STRUCT_NAMED, vec![0, 0, 6]),
            (TYPE_CODE_STRUCT_ANON, vec![1, 0, 1]),
            (TYPE_CODE_ARRAY, vec![4, 0]),
            (TYPE_CODE_VECTOR, vec![4, 0, 1]),
            (TYPE_CODE_STRUCT_NAME, text_ops(b"spirv.Image")),
            (TYPE_CODE_TARGET_TYPE, vec![1, 0, 7, 8]),
            // 9: unnamed ("%0").
            (TYPE_CODE_OPAQUE, vec![0]),
            (TYPE_CODE_BYTE, vec![8]),
            (TYPE_CODE_HALF, vec![]),
            (TYPE_CODE_BFLOAT, vec![]),
            (TYPE_CODE_X86_FP80, vec![]),
            (TYPE_CODE_FP128, vec![]),
            (TYPE_CODE_PPC_FP128, vec![]),
            (TYPE_CODE_X86_AMX, vec![]),
            (TYPE_CODE_TOKEN, vec![]),
            (TYPE_CODE_METADATA, vec![]),
            (TYPE_CODE_LABEL, vec![]),
            (TYPE_CODE_VOID, vec![]),
            (TYPE_CODE_FUNCTION, vec![0, 20, 0, 0]),
            (TYPE_CODE_STRUCT_ANON, vec![0]),
            (99, vec![]),
        ],
        |_| {},
    );
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    let t = &m.types;
    let names: Vec<String> = (0..t.declared)
        .map(|i| t.name(u32::try_from(i).unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            "i32",
            "ptr",
            "ptr addrspace(3)",
            "i32 (ptr, ...)",
            "%struct.S",
            "<{ i32, ptr }>",
            "[4 x i32]",
            "<vscale x 4 x i32>",
            "target(\"spirv.Image\", i32, 7, 8)",
            "%0",
            "b8",
            "half",
            "bfloat",
            "x86_fp80",
            "fp128",
            "ppc_fp128",
            "x86_amx",
            "token",
            "metadata",
            "label",
            "void",
            "void (i32, i32)",
            "{}",
            "<unknown type record 99>",
        ]
    );
    assert_eq!(t.body(4).unwrap(), "{ i32, [4 x i32] }");
    assert_eq!(t.body(9).unwrap(), "opaque");
    assert_eq!(t.body(0), None);
    assert_eq!(t.announced, Some(26));
    assert!(t.is_fp_or_fp_vector(11));
    assert!(!t.is_fp_or_fp_vector(7));
    assert_eq!(t.function(3), Some((0, &[1u32][..], true)));
    assert_eq!(t.contained(4, 1), Some(6));
    assert_eq!(t.contained(3, 0), Some(0));
    assert_eq!(m.program_address_space(), 3);
    assert_eq!(m.identification.as_ref().unwrap().producer, b"test");
    // A type that refers to itself is written without looping.
    let mut cyclic = TypeTable::default();
    cyclic.types.push(Type::Array { len: 1, element: 0 });
    assert!(cyclic.name(0).contains("..."));
}

/// The strtab-era module: globals named by a string table after it.
fn strtab_module(rest: impl FnOnce(&mut W), strtab: &[u8]) -> Vec<u8> {
    let mut data = module(
        2,
        &[
            // 0 i32, 1 ptr, 2 i32 (i32), 3 void, 4 void (ptr), 5 i1, 6 float.
            (TYPE_CODE_INTEGER, vec![32]),
            (TYPE_CODE_OPAQUE_POINTER, vec![0]),
            (TYPE_CODE_FUNCTION, vec![0, 0, 0]),
            (TYPE_CODE_VOID, vec![]),
            (TYPE_CODE_FUNCTION, vec![0, 3, 1]),
            (TYPE_CODE_INTEGER, vec![1]),
            (TYPE_CODE_FLOAT, vec![]),
        ],
        rest,
    );
    // The STRTAB block after the module.
    let mut w = W::new();
    w.enter(STRTAB_BLOCK_ID, 3);
    w.define(&[AbbrevOp::Literal(u64::from(STRTAB_BLOB)), AbbrevOp::Blob]);
    w.emit(4, 3);
    w.vbr(strtab.len() as u64, 6);
    w.align();
    for &b in strtab {
        w.emit(u64::from(b), 8);
    }
    w.align();
    w.end();
    data.extend_from_slice(&w.finish());
    data
}

#[test]
fn globals_functions_aliases_and_comdats() {
    // Names in the string table: "c", "g", "f", "a", "i".
    let data = strtab_module(
        |w| {
            w.text(MODULE_CODE_SECTIONNAME, b".data.x");
            w.text(MODULE_CODE_GCNAME, b"shadow");
            w.record(
                MODULE_CODE_COMDAT,
                &[0, 1, u64::from(COMDAT_SELECTION_KIND_LARGEST)],
            );
            // @g: type i32, explicit|constant, no initializer, internal,
            // align 4, section 1, hidden (dropped: local), initialexec,
            // unnamed_addr, externally initialized, dllexport (dropped),
            // comdat 1, no attributes, dso_local.
            w.record(
                MODULE_CODE_GLOBALVAR,
                &[1, 1, 0, 3, 0, 3, 3, 1, 1, 3, 1, 1, 2, 1, 0, 1],
            );
            // @f: type 2, C, defined, weak_odr, no attributes, protected, gc 1.
            w.record(MODULE_CODE_FUNCTION, &[2, 1, 2, 0, 0, 17, 0, 0, 0, 2, 1]);
            // @a = alias i32 @g; @i = extern_weak ifunc ... @f.
            w.record(MODULE_CODE_ALIAS, &[3, 1, 0, 0, 0, 0, 1]);
            w.record(MODULE_CODE_IFUNC, &[4, 1, 2, 0, 1, 7]);
            w.record(MODULE_CODE_HASH, &[1, 2, 3, 4, 5]);
            w.record(MODULE_CODE_SOURCE_FILENAME, &[u64::from(b'x')]);
            // An unknown record and an unknown block: skipped.
            w.record(77, &[1, 2, 3]);
            w.enter(60, 2);
            w.record(1, &[]);
            w.end();
        },
        b"cgfai",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(m.comdats.len(), 1);
    assert_eq!(&*m.comdats[0].name, b"c");
    assert_eq!(m.comdats[0].selection_name(), "largest");
    let g = &m.globals[0];
    assert_eq!(&*g.name, b"g");
    assert_eq!(g.linkage, Linkage::Internal);
    assert!(g.constant && g.is_declaration());
    assert_eq!(g.alignment, Some(4));
    assert_eq!(g.section.as_deref(), Some(&b".data.x"[..]));
    assert_eq!(g.visibility, Visibility::Default);
    assert_eq!(g.thread_local, ThreadLocal::InitialExec);
    assert_eq!(g.comdat, Some(0));
    assert!(g.externally_initialized && g.dso_local);
    let f = &m.functions[0];
    assert_eq!(
        (&*f.name, f.linkage, f.visibility),
        (&b"f"[..], Linkage::WeakOdr, Visibility::Protected)
    );
    assert_eq!(f.gc.as_deref(), Some(&b"shadow"[..]));
    assert!(!f.is_declaration);
    assert_eq!(f.body, None, "no body block");
    let a = &m.aliases[0];
    assert_eq!(
        (a.kind, &*a.name, a.target),
        (AliasKind::Alias, &b"a"[..], 0)
    );
    let i = &m.aliases[1];
    assert_eq!(
        (i.kind, &*i.name, i.target, i.linkage),
        (AliasKind::IFunc, &b"i"[..], 1, Linkage::ExternalWeak)
    );
    assert_eq!(m.hash, Some([1, 2, 3, 4, 5]));
    assert_eq!(m.source_filename, b"x");
    assert_eq!(m.value_name(1), Some(&b"f"[..]));
    assert_eq!(m.ir_name(3).as_deref(), Some("@i"));
    assert_eq!(m.types.name(m.value_types[0]), "ptr");
}

#[test]
fn short_and_out_of_range_records() {
    let data = strtab_module(
        |w| {
            // Too short, and named outside the string table.
            w.record(MODULE_CODE_GLOBALVAR, &[0, 1, 0]);
            w.record(MODULE_CODE_FUNCTION, &[0, 1, 2]);
            w.record(MODULE_CODE_ALIAS, &[0, 1]);
            w.record(MODULE_CODE_GLOBALVAR, &[100, 5, 0, 1, 0, 3, 0, 0]);
        },
        b"x",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert_eq!(m.errors.len(), 4);
    assert!(m.globals.is_empty() && m.functions.is_empty() && m.aliases.is_empty());
}

/// A module with `i32 @f(i32)` (its body written by `body`) and a
/// declared `void @h(ptr)`, and the constant `i32 2`.
fn function_module(body: impl FnOnce(&mut W)) -> Vec<u8> {
    strtab_module(
        |w| {
            w.record(MODULE_CODE_FUNCTION, &[0, 1, 2, 0, 0, 0, 0, 0, 0, 0]);
            w.record(MODULE_CODE_FUNCTION, &[1, 1, 4, 0, 1, 0, 0, 0, 0, 0]);
            w.enter(CONSTANTS_BLOCK_ID, 4);
            w.record(CST_CODE_SETTYPE, &[0]);
            w.record(CST_CODE_INTEGER, &[4]);
            w.end();
            w.enter(FUNCTION_BLOCK_ID, 4);
            body(w);
            w.end();
        },
        b"fh",
    )
}

#[test]
fn instructions_relative_ids_and_forward_references() {
    // Values: 0 @f, 1 @h, 2 the constant, 3 the argument; then the
    // instructions' from 4.
    let data = function_module(|w| {
        w.record(FUNC_CODE_DECLAREBLOCKS, &[2]);
        // %4 = add i32 %3, 2: relative 1 (3) and 2 (2).
        w.record(FUNC_CODE_INST_BINOP, &[1, 2, u64::from(BINOP_ADD)]);
        // %5 = mul i32 %6, %4: a forward reference (relative -1, as
        // unsigned) with its type, then 1 (4).
        w.record(
            FUNC_CODE_INST_BINOP,
            &[0xFFFF_FFFF, 0, 1, u64::from(BINOP_MUL)],
        );
        // %6 = icmp eq i32 %5, %4.
        w.record(FUNC_CODE_INST_CMP2, &[1, 2, 32]);
        // call void @h(ptr @f): explicit type 4, callee 1, argument 0.
        w.record(FUNC_CODE_INST_CALL, &[0, 1 << CALL_EXPLICIT_TYPE, 4, 6, 7]);
        // br i1 %6, label 1, label 1.
        w.record(FUNC_CODE_INST_BR, &[1, 1, 1]);
        w.record(FUNC_CODE_DEBUG_LOC, &[10, 3, 0, 0]);
        // %7 = phi i32 [%5, 0]: signed relative +2 (4), from 7.
        w.record(FUNC_CODE_INST_PHI, &[0, 4, 0]);
        // ret i32 %7.
        w.record(FUNC_CODE_INST_RET, &[1]);
        w.record(FUNC_CODE_DEBUG_LOC_AGAIN, &[]);
        // Unknown.
        w.record(200, &[1]);
    });
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    let b = &m.bodies[0];
    assert!(b.error.is_none(), "{:?}", b.error);
    assert_eq!(b.function, Some(0));
    assert_eq!(m.functions[0].body, Some(0));
    assert_eq!(b.first_local, 3);
    assert_eq!(b.arguments(), 1);
    let ops: Vec<Opcode> = b.instructions.iter().map(|i| i.opcode).collect();
    assert_eq!(
        ops,
        [
            Opcode::Add,
            Opcode::Mul,
            Opcode::ICmp,
            Opcode::Call,
            Opcode::Br,
            Opcode::Phi,
            Opcode::Ret
        ]
    );
    let values: Vec<Option<u32>> = b.instructions.iter().map(|i| i.value).collect();
    assert_eq!(
        values,
        [Some(4), Some(5), Some(6), None, None, Some(7), None]
    );
    assert_eq!(
        b.operands_of(&b.instructions[0]),
        [Operand::Value(3), Operand::Value(2)]
    );
    assert_eq!(
        b.operands_of(&b.instructions[1]),
        [Operand::Value(6), Operand::Value(4)]
    );
    assert_eq!(
        b.operands_of(&b.instructions[3]),
        [Operand::Value(1), Operand::Value(0)]
    );
    assert_eq!(
        b.operands_of(&b.instructions[4]),
        [Operand::Block(1), Operand::Block(1), Operand::Value(6)]
    );
    assert_eq!(
        b.operands_of(&b.instructions[5]),
        [Operand::Value(5), Operand::Block(0)]
    );
    assert_eq!(
        b.instructions[2].ty.map(|t| b.type_name(m, t)).as_deref(),
        Some("i1")
    );
    assert_eq!(b.instructions[4].debug_loc.unwrap().line, 10);
    assert_eq!(b.instructions[6].debug_loc.unwrap().column, 3);
    assert_eq!(
        b.instructions.iter().map(|i| i.block).collect::<Vec<_>>(),
        [0, 0, 0, 0, 0, 1, 1]
    );
    assert_eq!(b.blocks, 2);
    assert_eq!(b.unknown_records, 1);
    assert_eq!(b.calls.len(), 1);
    assert_eq!(b.calls[0].callee, Callee::Function(1));
    assert_eq!(b.callees(), [1]);
    assert_eq!(b.referenced_globals, [0, 1]);
    assert!(b.instruction_bits() > 0 && b.instruction_bits() < b.bits);
}

#[test]
fn fp_operators_and_casts() {
    // With a float argument: binary operators by their type.
    let data = strtab_module(
        |w| {
            // float (float): type 7.
            w.enter(TYPE_BLOCK_ID_NEW, 4);
            w.end();
            w.record(MODULE_CODE_FUNCTION, &[0, 1, 2, 0, 0, 0, 0, 0, 0, 0]);
            w.enter(FUNCTION_BLOCK_ID, 4);
            w.record(FUNC_CODE_DECLAREBLOCKS, &[1]);
            // %2 = sitofp i32 %1 to float; %3 = fadd float %2, %2;
            // %4 = fneg float %3; %5 = fcmp olt float %3, %4; %6 = srem.
            w.record(FUNC_CODE_INST_CAST, &[1, 6, u64::from(CAST_SITOFP)]);
            w.record(FUNC_CODE_INST_BINOP, &[1, 1, u64::from(BINOP_ADD)]);
            w.record(FUNC_CODE_INST_UNOP, &[1, u64::from(UNOP_FNEG)]);
            w.record(FUNC_CODE_INST_CMP2, &[2, 1, 4]);
            w.record(FUNC_CODE_INST_BINOP, &[3, 3, u64::from(BINOP_SREM)]);
            w.record(FUNC_CODE_INST_BINOP, &[5, 5, u64::from(BINOP_UDIV)]);
            w.record(FUNC_CODE_INST_RET, &[6]);
            w.end();
        },
        b"f",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let b = &bc.modules[0].bodies[0];
    let ops: Vec<Opcode> = b.instructions.iter().map(|i| i.opcode).collect();
    assert_eq!(
        ops,
        [
            Opcode::SiToFp,
            Opcode::FAdd,
            Opcode::FNeg,
            Opcode::FCmp,
            Opcode::FRem,
            // udiv on a float: rejected as LLVM rejects it.
            Opcode::Invalid(FUNC_CODE_INST_BINOP),
            Opcode::Ret
        ]
    );
}

#[test]
fn absolute_value_ids_in_version_0() {
    let mut data = module(
        0,
        &[
            (TYPE_CODE_INTEGER, vec![32]),
            (TYPE_CODE_FUNCTION, vec![0, 0, 0]),
            (TYPE_CODE_POINTER, vec![1]),
        ],
        |w| {
            // A typed-pointer-era function record (type: pointer to the
            // function type), named by the value symbol table.
            w.record(MODULE_CODE_FUNCTION, &[2, 0, 0, 0, 0, 0, 0, 0]);
            w.enter(FUNCTION_BLOCK_ID, 4);
            w.record(FUNC_CODE_DECLAREBLOCKS, &[1]);
            // %2 = add i32 %1, %1 (absolute IDs).
            w.record(FUNC_CODE_INST_BINOP, &[1, 1, u64::from(BINOP_ADD)]);
            // %3 = call i32 @f(i32 %2): no explicit type: through @f's
            // pointer type.
            w.record(FUNC_CODE_INST_CALL, &[0, 0, 0, 2]);
            w.record(FUNC_CODE_INST_RET, &[3]);
            w.end();
            w.enter(VALUE_SYMTAB_BLOCK_ID, 4);
            w.record(VST_CODE_ENTRY, &[0, u64::from(b'f')]);
            w.end();
        },
    );
    data.extend_from_slice(&[0; 4]);
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert_eq!(&*m.functions[0].name, b"f");
    assert_eq!(m.functions[0].function_type, 1);
    let b = &m.bodies[0];
    let ops: Vec<Opcode> = b.instructions.iter().map(|i| i.opcode).collect();
    assert_eq!(ops, [Opcode::Add, Opcode::Call, Opcode::Ret]);
    assert_eq!(
        b.operands_of(&b.instructions[1]),
        [Operand::Value(0), Operand::Value(2)]
    );
    assert_eq!(b.instructions[1].value, Some(3));
    assert_eq!(b.calls[0].callee, Callee::Function(0));
}

#[test]
fn constants_of_every_kind() {
    let data = strtab_module(
        |w| {
            w.record(MODULE_CODE_GLOBALVAR, &[0, 1, 0, 2, 0, 0, 0, 0]);
            w.enter(CONSTANTS_BLOCK_ID, 4);
            // (The current type starts as i32.)
            w.record(CST_CODE_INTEGER, &[5]);
            w.record(CST_CODE_SETTYPE, &[1]);
            w.record(CST_CODE_NULL, &[]);
            w.record(CST_CODE_UNDEF, &[]);
            w.record(CST_CODE_POISON, &[]);
            w.record(CST_CODE_WIDE_INTEGER, &[1, 2]);
            w.record(CST_CODE_FLOAT, &[0x3F80_0000]);
            w.record(CST_CODE_AGGREGATE, &[0, 1]);
            w.text(CST_CODE_STRING, b"ab");
            w.text(CST_CODE_CSTRING, b"cd");
            w.record(CST_CODE_DATA, &[1, 2, 3]);
            w.record(CST_CODE_CE_BINOP, &[u64::from(BINOP_ADD), 1, 1, 3]);
            w.record(CST_CODE_CE_CAST, &[u64::from(CAST_PTRTOINT), 1, 0]);
            w.record(CST_CODE_CE_GEP, &[0, 1, 1, 0, 0, 1]);
            w.record(CST_CODE_CE_GEP_OLD, &[1, 0, 0, 1]);
            w.record(CST_CODE_CE_GEP_WITH_INRANGE, &[0, 1, 64, 0, 16, 1, 0]);
            w.record(CST_CODE_CE_SELECT, &[1, 2, 3]);
            w.record(CST_CODE_CE_EXTRACTELT, &[7, 1, 0, 1]);
            w.record(CST_CODE_CE_INSERTELT, &[1, 2, 0, 3]);
            w.record(CST_CODE_CE_SHUFFLEVEC, &[1, 2, 3]);
            w.record(CST_CODE_CE_CMP, &[0, 1, 2, 32]);
            w.record(CST_CODE_INLINEASM, &[2, 1, 3, 110, 111, 112, 2, 114, 44]);
            w.record(CST_CODE_INLINEASM_OLD, &[1, 1, 120, 0]);
            w.record(CST_CODE_BLOCKADDRESS, &[2, 0, 3]);
            w.record(CST_CODE_DSO_LOCAL_EQUIVALENT, &[1, 0]);
            w.record(CST_CODE_NO_CFI_VALUE, &[1, 0]);
            w.record(CST_CODE_PTRAUTH, &[0, 1, 2, 3]);
            w.record(CST_CODE_CE_UNOP, &[0, 6]);
            w.record(250, &[1]);
            w.end();
        },
        b"g",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    use ConstantKind as K;
    let kinds: Vec<&K> = m.constants.iter().map(|c| &c.kind).collect();
    assert_eq!(kinds.len(), 28);
    assert_eq!(*kinds[0], K::Integer(-2));
    assert_eq!(m.constants[0].ty, 0);
    assert_eq!(m.constants[1].ty, 1);
    assert_eq!(*kinds[1], K::Null);
    assert_eq!(*kinds[2], K::Undef);
    assert_eq!(*kinds[3], K::Poison);
    assert_eq!(*kinds[4], K::WideInteger(vec![1, 2]));
    assert_eq!(*kinds[5], K::Float(vec![0x3F80_0000]));
    assert_eq!(*kinds[6], K::Aggregate(vec![0, 1]));
    assert_eq!(*kinds[7], K::String(b"ab".to_vec()));
    assert_eq!(*kinds[8], K::CString(b"cd".to_vec()));
    assert_eq!(*kinds[9], K::Data(vec![1, 2, 3]));
    assert_eq!(
        *kinds[10],
        K::BinaryOp {
            op: 0,
            lhs: 1,
            rhs: 1,
            flags: 3
        }
    );
    assert_eq!(
        *kinds[11],
        K::Cast {
            op: CAST_PTRTOINT,
            from: 1,
            operand: 0
        }
    );
    assert_eq!(
        *kinds[12],
        K::GetElementPtr {
            source_type: Some(0),
            flags: 1,
            operands: vec![0, 1]
        }
    );
    assert_eq!(
        *kinds[13],
        K::GetElementPtr {
            source_type: None,
            flags: 0,
            operands: vec![0, 1]
        }
    );
    assert_eq!(
        *kinds[14],
        K::GetElementPtr {
            source_type: Some(0),
            flags: 1,
            operands: vec![0]
        }
    );
    assert_eq!(*kinds[15], K::Select([1, 2, 3]));
    assert_eq!(*kinds[16], K::ExtractElement([1, 1]));
    assert_eq!(*kinds[17], K::InsertElement([1, 2, 3]));
    assert_eq!(*kinds[18], K::ShuffleVector([1, 2, 3]));
    assert_eq!(
        *kinds[19],
        K::Compare {
            predicate: 32,
            lhs: 1,
            rhs: 2
        }
    );
    assert_eq!(
        *kinds[20],
        K::InlineAsm {
            function_type: Some(2),
            flags: 1,
            asm: b"nop".to_vec(),
            constraints: b"r,".to_vec()
        }
    );
    assert_eq!(
        *kinds[21],
        K::InlineAsm {
            function_type: None,
            flags: 1,
            asm: b"x".to_vec(),
            constraints: vec![]
        }
    );
    assert_eq!(
        *kinds[22],
        K::BlockAddress {
            function: 0,
            block: 3
        }
    );
    assert_eq!(*kinds[23], K::DsoLocalEquivalent(0));
    assert_eq!(*kinds[24], K::NoCfi(0));
    assert_eq!(*kinds[25], K::PtrAuth(vec![0, 1, 2, 3]));
    assert_eq!(*kinds[26], K::UnaryOp { op: 0, operand: 6 });
    // An unknown record is undef, and takes a value ID.
    assert_eq!(*kinds[27], K::Undef);
    assert_eq!(m.values.len(), 1 + 28);
    assert_eq!(m.constant(1).map(|c| &c.kind), Some(&K::Integer(-2)));
    assert_eq!(m.constants[24].operands(), [0]);
}

#[test]
fn attribute_groups_of_every_kind() {
    let mut rec = vec![1, 0xFFFF_FFFF];
    rec.extend([0, u64::from(ATTR_KIND_NO_INLINE)]);
    rec.extend([1, u64::from(ATTR_KIND_ALIGNMENT), 16]);
    rec.extend([3, u64::from(b'k'), 0]);
    rec.extend([4, u64::from(b'k'), 0, u64::from(b'v'), 0]);
    rec.extend([5, u64::from(ATTR_KIND_BY_VAL)]);
    rec.extend([6, u64::from(ATTR_KIND_STRUCT_RET), 0]);
    rec.extend([7, u64::from(ATTR_KIND_RANGE), 32, 0, 20]);
    rec.extend([7, u64::from(ATTR_KIND_RANGE), 128, (1 << 32) | 1, 5, 6]);
    rec.extend([8, u64::from(ATTR_KIND_INITIALIZES), 2, 64, 0, 8, 16, 24]);
    rec.extend([0, 9999]);
    let data = strtab_module(
        |w| {
            w.enter(PARAMATTR_GROUP_BLOCK_ID, 3);
            w.record(PARAMATTR_GRP_CODE_ENTRY, &rec);
            w.end();
            w.enter(PARAMATTR_BLOCK_ID, 3);
            w.record(PARAMATTR_CODE_ENTRY, &[1]);
            w.end();
        },
        b"",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    let g = &m.attribute_groups[0];
    assert_eq!((g.id, g.index), (1, 0xFFFF_FFFF));
    assert_eq!(
        g.attributes,
        [
            Attribute::Enum(ATTR_KIND_NO_INLINE),
            Attribute::Int(ATTR_KIND_ALIGNMENT, 16),
            Attribute::String(b"k".to_vec(), None),
            Attribute::String(b"k".to_vec(), Some(b"v".to_vec())),
            Attribute::Type(ATTR_KIND_BY_VAL, None),
            Attribute::Type(ATTR_KIND_STRUCT_RET, Some(0)),
            Attribute::Range(ATTR_KIND_RANGE, 32, vec![0, 20]),
            Attribute::Range(ATTR_KIND_RANGE, 128, vec![(1 << 32) | 1, 5, 6]),
            Attribute::RangeList(ATTR_KIND_INITIALIZES, 64, vec![0, 8, 16, 24]),
            Attribute::Enum(9999),
        ]
    );
    let names: Vec<String> = g.attributes.iter().map(|a| a.name().into_owned()).collect();
    assert_eq!(names[..2], ["noinline", "align"]);
    assert_eq!(names[9], "<attribute 9999>");
    assert_eq!(m.attribute_list(1).len(), 1);
    assert!(m.attribute_list(0).is_empty() && m.attribute_list(7).is_empty());
}

#[test]
fn metadata_records() {
    // Strings: lengths (VBR6, a bitstream) then characters.
    let mut lengths = W::new();
    for n in [4u64, 3, 1, 0] {
        lengths.vbr(n, 6);
    }
    let mut blob = lengths.finish();
    let offset = blob.len() as u64;
    blob.extend_from_slice(b"filedirf");
    let data = strtab_module(
        |w| {
            w.enter(METADATA_KIND_BLOCK_ID, 3);
            w.record(
                METADATA_KIND,
                &[0, u64::from(b'd'), u64::from(b'b'), u64::from(b'g')],
            );
            w.end();
            w.enter(METADATA_BLOCK_ID, 4);
            w.define(&[
                AbbrevOp::Literal(u64::from(METADATA_STRINGS)),
                AbbrevOp::Vbr(6),
                AbbrevOp::Vbr(6),
                AbbrevOp::Blob,
            ]);
            w.emit(4, 4);
            w.vbr(4, 6);
            w.vbr(offset, 6);
            w.vbr(blob.len() as u64, 6);
            w.align();
            for &b in &blob {
                w.emit(u64::from(b), 8);
            }
            w.align();
            // 4: !DIFile("file", "dir"); 5: a tuple; 6: a value; 7: an old
            // string; 8: a defined !DISubprogram "f" at line 42.
            w.record(METADATA_FILE, &[0, 1, 2]);
            w.record(METADATA_NODE, &[5, 0]);
            w.record(METADATA_VALUE, &[0, 0]);
            w.text(METADATA_STRING_OLD, b"old");
            w.record(
                METADATA_SUBPROGRAM,
                &[
                    4 | 2 | 1,
                    0,
                    3,
                    0,
                    5,
                    42,
                    0,
                    0,
                    0,
                    8,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
            );
            w.text(METADATA_NAME, b"named");
            w.record(METADATA_NAMED_NODE, &[5]);
            w.record(METADATA_GLOBAL_DECL_ATTACHMENT, &[0, 0, 8]);
            w.record(METADATA_INDEX_OFFSET, &[1, 0]);
            // Unknown: no ID.
            w.record(120, &[1]);
            // 9: a compile unit (language 12, file !4, producer "file").
            w.record(
                METADATA_COMPILE_UNIT,
                &[1, 12, 5, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            );
            w.end();
        },
        b"",
    );
    let bc = Bitcode::parse(&data).unwrap();
    let md = &bc.modules[0].metadata;
    assert_eq!(md.kind(b"dbg"), Some(0));
    assert_eq!(md.string(0), Some(&b"file"[..]));
    assert_eq!(md.string(3), Some(&b""[..]));
    let f = md.file(4).unwrap();
    assert_eq!((f.filename, f.directory), (&b"file"[..], &b"dir"[..]));
    assert_eq!(md.node(5), Some((METADATA_NODE, &[5, 0][..])));
    assert_eq!(md.get(6), Some(&MetadataNode::Value { ty: 0, value: 0 }));
    assert_eq!(md.string(7), Some(&b"old"[..]));
    let sp = md.subprogram(8).unwrap();
    assert_eq!((sp.name, sp.line, sp.definition), (&b"f"[..], 42, true));
    assert_eq!(sp.file.unwrap().filename, b"file");
    assert_eq!(md.named, [(b"named".to_vec(), vec![5])]);
    assert_eq!(md.global_attachments, [(0, vec![(0, 8)])]);
    assert_eq!(md.nodes.len(), 10);
    let cu = &md.compile_units()[0];
    assert_eq!(
        (cu.id, cu.language, cu.producer, cu.optimized),
        (9, 12, &b"file"[..], true)
    );
    assert_eq!(md.subprograms().len(), 1);
}

#[test]
fn symbol_tables() {
    // A header, one module of two symbols (the second with an Uncommon),
    // a comdat, and strings.
    let strtab = b"producertriplesrcnameirnamecomdatsection";
    let at = |s: &[u8]| -> [u32; 2] {
        let at = strtab.windows(s.len()).position(|w| w == s).unwrap();
        [u32::try_from(at).unwrap(), u32::try_from(s.len()).unwrap()]
    };
    let mut words: Vec<u32> = vec![4];
    words.extend(at(b"producer"));
    let modules = 76;
    let comdats = modules + 12;
    let symbols = comdats + 12;
    let uncommons = symbols + 48;
    words.extend([modules, 1, comdats, 1, symbols, 2, uncommons, 1]);
    words.extend(at(b"triple"));
    words.extend(at(b"src"));
    words.extend([0, 0, 0, 0]);
    assert_eq!(words.len() * 4, 76);
    words.extend([0, 2, 0]);
    words.extend(at(b"comdat"));
    words.push(2);
    words.extend(at(b"name"));
    words.extend(at(b"irname"));
    words.extend([
        u32::MAX,
        flags::GLOBAL | flags::EXECUTABLE | (1 << flags::VISIBILITY_SHIFT),
    ]);
    words.extend(at(b"name"));
    words.extend([0, 0, 0, flags::GLOBAL | flags::COMMON | flags::HAS_UNCOMMON]);
    words.extend([16, 0, 8, 0, 0]);
    words.extend(at(b"section"));
    let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let st = Symtab::new(&data, strtab);
    assert!(st.is_valid());
    assert_eq!(st.version(), Some(4));
    assert_eq!(st.producer(), b"producer");
    assert_eq!(st.target_triple(), b"triple");
    assert_eq!(st.source_filename(), b"src");
    assert!(st.dependent_libraries().is_empty());
    assert_eq!(st.comdats()[0].name, b"comdat");
    assert_eq!(st.comdats()[0].selection, 2);
    let syms = st.symbols();
    assert_eq!(syms.len(), 2);
    assert_eq!(
        (syms[0].ir_name, syms[0].comdat, syms[0].visibility()),
        (&b"irname"[..], None, 1)
    );
    assert!(syms[0].has(flags::EXECUTABLE));
    assert_eq!(
        (syms[1].common_size, syms[1].common_align, syms[1].section),
        (16, 8, &b"section"[..])
    );
    assert_eq!(syms[1].comdat, Some(0));
    // Truncated: nothing read out of range.
    for n in 0..data.len() {
        let st = Symtab::new(&data[..n], strtab);
        let _ = (
            st.symbols(),
            st.comdats(),
            st.modules(),
            st.producer(),
            st.dependent_libraries(),
        );
    }
    assert_eq!(Symtab::new(&data, b"").symbols()[0].name, b"");
}

// Hostile inputs.

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/bitcode");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            let n = p.file_name()?.to_str()?.to_string();
            let text = n.ends_with(".txt") || n.ends_with(".ll") || n.ends_with(".md");
            (!text).then(|| (n, std::fs::read(&p).unwrap()))
        })
        .collect();
    out.sort();
    assert!(out.len() >= 14);
    out
}

/// Everything the crate reads from `data`, for a hostile input.
fn exercise(data: &[u8]) {
    let mut out = Vec::new();
    let _ = analyze(data, &DumpOptions::default(), Some("x"), &mut out);
    for e in crate::embedded::find(data) {
        exercise_bitcode(e.data);
    }
    exercise_bitcode(data);
}

fn exercise_bitcode(data: &[u8]) {
    let Ok(bc) = Bitcode::parse(data) else { return };
    for m in &bc.modules {
        let _ = (m.metadata.compile_units(), m.metadata.subprograms());
        for t in 0..m.types.len().min(4096) {
            let t = u32::try_from(t).unwrap();
            let _ = (m.types.name(t), m.types.body(t));
        }
        for g in &m.globals {
            let _ = m.ir_name(g.value);
        }
        for b in &m.bodies {
            let _ = (b.subprogram(m), b.callees(), b.metadata(m).files());
            for i in &b.instructions {
                let _ = (b.operands_of(i), i.ty.map(|t| b.type_name(m, t)));
            }
        }
    }
    if let Some(st) = bc.symtab {
        let _ = (st.symbols(), st.comdats(), st.dependent_libraries());
    }
}

#[test]
fn truncated_everywhere() {
    for (_, data) in fixtures() {
        // Every length near the start and end, and a spread between.
        let step = (data.len() / 40).max(1);
        let lens = (0..data.len().min(24))
            .chain((0..data.len()).step_by(step))
            .chain(data.len().saturating_sub(24)..data.len());
        for n in lens {
            exercise(&data[..n]);
        }
    }
}

#[test]
fn corrupted_everywhere() {
    // Deterministic: an xorshift's choice of bits flipped, bytes zeroed,
    // words set to all ones, bytes bumped, a few at a time.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for (_, data) in fixtures() {
        let len = data.len() as u64;
        for _ in 0..30 {
            let mut d = data.clone();
            for _ in 0..1 + next() % 4 {
                let at = usize::try_from(next() % len).unwrap();
                match next() % 4 {
                    0 => d[at] ^= 1 << (next() % 8),
                    1 => d[at] = 0,
                    2 => {
                        let end = (at + 4).min(d.len());
                        d[at..end].fill(0xFF);
                    }
                    _ => d[at] = d[at].wrapping_add(1),
                }
            }
            exercise(&d);
        }
    }
}

#[test]
fn deep_nesting() {
    // Blocks nested far deeper than any real stream's: nothing recurses
    // to overflow the stack.
    let mut w = W::ir();
    for _ in 0..20_000 {
        w.enter(60, 2);
    }
    for _ in 0..20_000 {
        w.end();
    }
    let data = w.finish();
    let mut out = Vec::new();
    analyze(&data, &DumpOptions::default(), None, &mut out).unwrap();
    // (Not a module: none.)
    assert!(Bitcode::parse(&data).unwrap().modules.is_empty());
}

#[test]
fn many_bodies_on_every_core() {
    // Enough function blocks (over 256 KB) to be read on every core: each
    // body belongs to its function and is read as on one.
    const N: u64 = 3000;
    let mut names = Vec::new();
    for i in 0..N {
        names.extend_from_slice(format!("f{i:05}").as_bytes());
    }
    let data = strtab_module(
        |w| {
            for i in 0..N {
                w.record(MODULE_CODE_FUNCTION, &[i * 6, 6, 2, 0, 0, 0, 0, 0, 0, 0]);
            }
            for i in 0..N {
                w.enter(FUNCTION_BLOCK_ID, 4);
                w.record(FUNC_CODE_DECLAREBLOCKS, &[1]);
                // i % 50 + 1 adds, each of the one before (the argument first).
                for _ in 0..i % 50 + 1 {
                    w.record(FUNC_CODE_INST_BINOP, &[1, 1, u64::from(BINOP_ADD)]);
                }
                w.record(FUNC_CODE_INST_RET, &[1]);
                // Padding records, to make the blocks large.
                for _ in 0..20 {
                    w.record(FUNC_CODE_DEBUG_LOC, &[1, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
                }
                w.end();
            }
        },
        &names,
    );
    assert!(data.len() > 256 * 1024, "{}", data.len());
    let bc = Bitcode::parse(&data).unwrap();
    let m = &bc.modules[0];
    assert_eq!(m.bodies.len(), usize::try_from(N).unwrap());
    for (i, b) in m.bodies.iter().enumerate() {
        assert_eq!(b.function, Some(i));
        assert_eq!(m.functions[i].body, Some(i));
        assert_eq!(b.instructions.len(), i % 50 + 2);
        let first = u32::try_from(N).unwrap() + 1;
        let last = b.instructions[b.instructions.len() - 2].value.unwrap();
        assert_eq!(last, first + u32::try_from(i % 50).unwrap());
        assert!(b.error.is_none());
    }
}

#[test]
fn circular_wide_types_have_bounded_names() {
    // (A fuzzer's find.) Literal structures of 50 elements that refer to
    // each other: their names would grow as 50^64.
    let mut t = TypeTable::default();
    t.types.push(Type::Struct {
        name: None,
        packed: false,
        elements: vec![1; 50],
        opaque: false,
    });
    t.types.push(Type::Array { len: 2, element: 0 });
    let name = t.name(0);
    // (What's open when the cap is reached still closes.)
    assert!(name.len() < 4 * crate::types::MAX_NAME, "{}", name.len());
    assert!(name.contains("..."));
}

#[test]
fn fuzzer_finds() {
    // Each an input the fuzzer found a panic with, kept in testdata.
    // fuzz-target-type.bin: an empty TYPE_CODE_TARGET_TYPE record.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/bitcode");
    let data = std::fs::read(dir.join("fuzz-target-type.bin")).unwrap();
    exercise(&data);
    // And on its own: a target type with no operands.
    let data = module(2, &[(TYPE_CODE_TARGET_TYPE, vec![])], |_| {});
    let bc = Bitcode::parse(&data).unwrap();
    assert_eq!(bc.modules[0].types.name(0), "target(\"\")");
}
