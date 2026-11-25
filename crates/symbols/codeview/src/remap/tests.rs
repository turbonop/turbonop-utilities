use super::*;
use crate::c13::push_subsection;
use crate::symbols::kind::*;
use crate::types::leaf;
use crate::write::{RecordBuilder, push_type};

/// Indices moved up by 0x100; strings by 1; checksums by 0x10.
struct Shift;

impl Remap for Shift {
    fn index(&mut self, index: u32, _: bool) -> Option<u32> {
        Some(index + 0x100)
    }
    fn string(&mut self, offset: u32) -> Option<u32> {
        Some(offset + 1)
    }
    fn checksum(&mut self, offset: u32) -> Option<u32> {
        Some(offset + 0x10)
    }
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[test]
fn type_streams() {
    let mut stream = Vec::new();
    // LF_ARGLIST (int, 0x1000): only the second is renumbered.
    let mut a = RecordBuilder::new(leaf::LF_ARGLIST);
    a.u32(2).u32(0x74).u32(0x1000);
    a.write_type(&mut stream).unwrap();
    // LF_PROCEDURE: return 0x1001, arglist 0x1002.
    let proc_at = stream.len();
    let mut p = RecordBuilder::new(leaf::LF_PROCEDURE);
    p.u32(0x1001).u8(0).u8(0).u16(2).u32(0x1002);
    p.write_type(&mut stream).unwrap();
    // LF_FUNC_ID: scope (an id) 0, type 0x1003, name.
    let mut f = RecordBuilder::new(leaf::LF_FUNC_ID);
    f.u32(0).u32(0x1003).name(b"f");
    f.write_type(&mut stream).unwrap();
    let mut section = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    section.extend_from_slice(&stream);
    let before = section.clone();
    let mut seen = Vec::new();
    debug_t(&mut section, &mut |i, id| {
        seen.push((i, id));
        Some(i + 0x10)
    })
    .unwrap();
    assert_eq!(
        seen,
        [
            (0x1000, false),
            (0x1001, false),
            (0x1002, false),
            (0x1003, false)
        ]
    );
    let s = &section[4..];
    assert_eq!(le32(s, 4 + 4), 0x74);
    assert_eq!(le32(s, 4 + 8), 0x1010);
    assert_eq!(le32(s, proc_at + 4), 0x1011);
    assert_eq!(le32(s, proc_at + 12), 0x1012);
    // Back again: the same bytes.
    debug_t(&mut section, &mut |i, _| Some(i - 0x10)).unwrap();
    assert_eq!(section, before);
    // An index without a new number: an error at its place in the section.
    let mut copy = before.clone();
    assert_eq!(
        debug_t(&mut copy, &mut |i, _| (i != 0x1002).then_some(i)),
        Err(Error::Unmapped {
            at: 4 + proc_at + 12,
            value: 0x1002,
            field: FieldKind::Type
        })
    );
    // An unknown kind; a record cut short; no signature.
    let mut unknown = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    push_type(&mut unknown, 0x1234, &[0; 4]).unwrap();
    assert_eq!(
        debug_t(&mut unknown, &mut |i, _| Some(i)),
        Err(Error::Type {
            at: 4,
            error: RefError::Unknown(0x1234)
        })
    );
    let mut cut = before.clone();
    cut.pop();
    assert!(matches!(
        debug_t(&mut cut, &mut |i, _| Some(i)),
        Err(Error::Truncated { .. })
    ));
    assert!(debug_t(&mut [0; 4], &mut |i, _| Some(i)).is_err());
    // type_records reads the same records.
    let records: Vec<_> = type_records(&stream).collect::<Result<_, _>>().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].0, proc_at);
    assert_eq!(records[1].1.kind, leaf::LF_PROCEDURE);
    assert_eq!(type_records(&stream[..5]).last(), Some(Err(0)));
}

/// A `.debug$S` with every kind of field: each is renumbered, and nothing
/// else changes.
#[test]
fn debug_s_sections() {
    let mut symbols = Vec::new();
    // S_GPROC32_ID: id 0x1005.
    let mut p = RecordBuilder::new(S_GPROC32_ID);
    p.bytes(&[0; 24]).u32(0x1005).u32(0).u16(0).u8(0).name(b"f");
    p.write_symbol(&mut symbols, 1).unwrap();
    // S_REGREL32: type 0x1006; S_LOCAL: int (simple, left alone).
    let regrel_at = symbols.len();
    let mut r = RecordBuilder::new(S_REGREL32);
    r.u32(8).u32(0x1006).u16(0).name(b"x");
    r.write_symbol(&mut symbols, 1).unwrap();
    let local_at = symbols.len();
    let mut l = RecordBuilder::new(S_LOCAL);
    l.u32(0x74).u16(0).name(b"i");
    l.write_symbol(&mut symbols, 1).unwrap();
    // S_FILESTATIC: type 0x1007, string 0x20.
    let fs_at = symbols.len();
    let mut s = RecordBuilder::new(S_FILESTATIC);
    s.u32(0x1007).u32(0x20).u16(0).name(b"g");
    s.write_symbol(&mut symbols, 1).unwrap();
    // S_INLINESITE: id 0x1008, annotations changing to file 0x18.
    let site_at = symbols.len();
    let mut i = RecordBuilder::new(S_INLINESITE);
    i.u32(0).u32(0).u32(0x1008).bytes(&[5, 0x18, 3, 4, 0, 0]);
    i.write_symbol(&mut symbols, 4).unwrap();
    let mut inlinees = c13::CV_INLINEE_SOURCE_LINE_SIGNATURE.to_le_bytes().to_vec();
    for v in [0x1008u32, 0x18, 3] {
        inlinees.extend_from_slice(&v.to_le_bytes());
    }
    let mut checksums = Vec::new();
    c13::push_file_checksum(
        &mut checksums,
        &crate::lines::FileChecksum {
            name: 1,
            kind: 0,
            checksum: &[],
        },
    );
    let mut section = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    push_subsection(&mut section, kind::DEBUG_S_SYMBOLS, &symbols);
    let inlinees_at = section.len() + 8;
    push_subsection(&mut section, kind::DEBUG_S_INLINEELINES, &inlinees);
    let checksums_at = section.len() + 8;
    push_subsection(&mut section, kind::DEBUG_S_FILECHKSMS, &checksums);
    push_subsection(&mut section, kind::DEBUG_S_STRINGTABLE, b"\0a\0");
    let before = section.clone();
    debug_s(&mut section, &mut Shift).unwrap();
    let sym = &section[12..];
    assert_eq!(le32(sym, 4 + 24), 0x1105);
    assert_eq!(le32(sym, regrel_at + 8), 0x1106);
    assert_eq!(le32(sym, local_at + 4), 0x74);
    assert_eq!(le32(sym, fs_at + 4), 0x1107);
    assert_eq!(le32(sym, fs_at + 8), 0x21);
    assert_eq!(le32(sym, site_at + 12), 0x1108);
    assert_eq!(sym[site_at + 17], 0x28);
    assert_eq!(le32(&section, inlinees_at + 4), 0x1108);
    assert_eq!(le32(&section, inlinees_at + 8), 0x28);
    assert_eq!(le32(&section, checksums_at), 2);
    // Only those bytes changed.
    let changed: Vec<usize> = (0..section.len())
        .filter(|&i| section[i] != before[i])
        .collect();
    let base = 12;
    let want = [
        base + 4 + 24 + 1,
        base + regrel_at + 8 + 1,
        base + fs_at + 4 + 1,
        base + fs_at + 8,
        base + site_at + 12 + 1,
        base + site_at + 17,
        inlinees_at + 4 + 1,
        inlinees_at + 8,
        checksums_at,
    ];
    assert_eq!(changed, want);

    // A checksum that compresses longer can't be patched in place.
    let mut copy = before.clone();
    struct Far;
    impl Remap for Far {
        fn index(&mut self, i: u32, _: bool) -> Option<u32> {
            Some(i)
        }
        fn checksum(&mut self, o: u32) -> Option<u32> {
            Some(o + 0x100)
        }
    }
    assert_eq!(
        debug_s(&mut copy, &mut Far),
        Err(Error::AnnotationLength {
            at: base + site_at + 17
        })
    );
    // A symbol kind without a layout: an error at its record.
    let mut bad = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    push_subsection(&mut bad, kind::DEBUG_S_SYMBOLS, &[2, 0, 0x99, 0x11]);
    assert_eq!(
        debug_s(&mut bad, &mut Shift),
        Err(Error::Symbol {
            at: 12,
            error: FieldError::Unknown(0x1199)
        })
    );
    // A header past the end.
    let mut cut = before.clone();
    cut.truncate(10);
    assert!(debug_s(&mut cut, &mut Shift).is_err());
}

#[test]
fn single_records() {
    let mut body = vec![0u8; 12];
    body[8..12].copy_from_slice(&0x1234u32.to_le_bytes());
    let mut scratch = Vec::new();
    symbol_record(S_CALLSITEINFO, &mut body, &mut scratch, &mut Shift).unwrap();
    assert_eq!(le32(&body, 8), 0x1334);
    let mut t = 0x1000u32.to_le_bytes().to_vec();
    t.extend_from_slice(&[1, 0]);
    type_record(leaf::LF_MODIFIER, &mut t, &mut Vec::new(), &mut Shift).unwrap();
    assert_eq!(le32(&t, 0), 0x1100);
    assert!(symbol_record(0x1199, &mut [], &mut scratch, &mut Shift).is_err());
    assert!(type_record(0x1234, &mut [], &mut Vec::new(), &mut Shift).is_err());
    assert_eq!(
        Error::Unmapped {
            at: 8,
            value: 0x1000,
            field: FieldKind::Id
        }
        .to_string(),
        "no new number for Id 0x1000 (at 0x8)"
    );
}
