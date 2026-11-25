use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::object::{comdat_select, weak_search};

/// Table indices are ids times 2 (each symbol has one auxiliary record);
/// sections 1..=0x2_0000 exist; line numbers are at 1000 + 6 × index in
/// section 1.
struct Fixed;

impl Resolve for Fixed {
    fn symbol(&self, index: u32) -> Option<SymbolId> {
        (index.is_multiple_of(2) && index < 200).then_some(SymbolId(index / 2))
    }
    fn section(&self, number: u32) -> Option<SectionId> {
        (1..=0x2_0000)
            .contains(&number)
            .then(|| SectionId(number - 1))
    }
    fn line_number(&self, offset: u32) -> Option<LineNumberRef> {
        let delta = offset.checked_sub(1000)?;
        (delta % 6 == 0).then_some(LineNumberRef {
            section: SectionId(0),
            index: delta / 6,
        })
    }
}

impl Place for Fixed {
    fn symbol(&self, id: SymbolId) -> u32 {
        id.0 * 2
    }
    fn line_number(&self, at: LineNumberRef) -> u32 {
        1000 + at.index * 6
    }
}

fn primary(storage_class: u8, section: SectionNumber, kind: u16, value: u32) -> Primary {
    Primary {
        value,
        section,
        kind,
        storage_class,
    }
}

const IN_SECTION: SectionNumber = SectionNumber::Section(SectionId(0));

/// Decodes `bytes` and encodes the result, checking that it gives the
/// same bytes.
fn decoded(p: &Primary, bytes: &[u8], format: Format) -> Aux<'static> {
    let stride = format.record_size();
    let count = bytes.len() / stride;
    let aux = decode(p, bytes, count, format, &Fixed).into_owned();
    assert_eq!(aux.count(stride), count, "{aux:?}");
    let mut out = vec![0u8; bytes.len()];
    encode(&aux, format, &Fixed, &mut out);
    assert_eq!(out, bytes, "{aux:?}");
    aux
}

fn record(format: Format, fields: &[(usize, &[u8])]) -> Vec<u8> {
    let mut r = vec![0u8; format.record_size()];
    for &(at, bytes) in fields {
        r[at..at + bytes.len()].copy_from_slice(bytes);
    }
    r
}

const FORMATS: [Format; 2] = [Format::Regular, Format::BigObj];

#[test]
fn function_definitions() {
    for f in FORMATS {
        let p = primary(storage_class::EXTERNAL, IN_SECTION, 0x20, 16);
        let r = record(
            f,
            &[
                (0, &4u32.to_le_bytes()),
                (4, &99u32.to_le_bytes()),
                (8, &1012u32.to_le_bytes()),
                (12, &8u32.to_le_bytes()),
            ],
        );
        assert_eq!(
            decoded(&p, &r, f),
            Aux::Function(FunctionDefinition {
                tag: Some(SymbolId(2)),
                total_size: 99,
                line_numbers: Some(LineNumberRef {
                    section: SectionId(0),
                    index: 2,
                }),
                next: Some(SymbolId(4)),
            })
        );
        // No .bf, no line numbers, no next function.
        let r = record(f, &[(4, &7u32.to_le_bytes())]);
        assert_eq!(
            decoded(&p, &r, f),
            Aux::Function(FunctionDefinition {
                total_size: 7,
                ..FunctionDefinition::default()
            })
        );
        // A tag that isn't a symbol, a line number pointer that isn't a
        // record, or unused bytes: kept raw.
        for r in [
            record(f, &[(0, &3u32.to_le_bytes())]),
            record(f, &[(8, &1001u32.to_le_bytes())]),
            record(f, &[(16, &[1])]),
            record(f, &[(f.record_size() - 1, &[1])]),
        ] {
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)), "{r:?}");
        }
        // Not a function: an undefined one, a static one, a data symbol.
        let r = record(f, &[(4, &7u32.to_le_bytes())]);
        for p in [
            primary(storage_class::EXTERNAL, SectionNumber::Undefined, 0x20, 16),
            primary(storage_class::EXTERNAL, IN_SECTION, 0, 16),
        ] {
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)));
        }
    }
}

#[test]
fn begin_and_end_of_function() {
    for f in FORMATS {
        let p = primary(storage_class::FUNCTION, IN_SECTION, 0, 0);
        let r = record(f, &[(4, &42u16.to_le_bytes()), (12, &6u32.to_le_bytes())]);
        assert_eq!(
            decoded(&p, &r, f),
            Aux::BeginEnd(BeginEnd {
                line: 42,
                next: Some(SymbolId(3)),
            })
        );
        let r = record(f, &[(4, &43u16.to_le_bytes())]);
        assert_eq!(
            decoded(&p, &r, f),
            Aux::BeginEnd(BeginEnd {
                line: 43,
                next: None
            })
        );
        for at in [0, 3, 6, 11, 16] {
            let r = record(f, &[(at, &[1])]);
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)), "{at}");
        }
    }
}

#[test]
fn weak_externals() {
    for f in FORMATS {
        for search in [
            weak_search::NOLIBRARY,
            weak_search::LIBRARY,
            weak_search::ALIAS,
            weak_search::ANTI_DEPENDENCY,
            0x1234,
        ] {
            let r = record(f, &[(0, &0u32.to_le_bytes()), (4, &search.to_le_bytes())]);
            let weak = Aux::WeakExternal(WeakExternal {
                tag: SymbolId(0),
                search,
            });
            let p = primary(storage_class::WEAK_EXTERNAL, SectionNumber::Undefined, 0, 0);
            assert_eq!(decoded(&p, &r, f), weak);
            // The PE specification's form: an undefined external of value 0.
            let p = primary(storage_class::EXTERNAL, SectionNumber::Undefined, 0, 0);
            assert_eq!(decoded(&p, &r, f), weak);
        }
        let p = primary(storage_class::WEAK_EXTERNAL, SectionNumber::Undefined, 0, 0);
        for r in [
            record(f, &[(0, &1u32.to_le_bytes())]),
            record(f, &[(8, &[1])]),
        ] {
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)));
        }
    }
}

#[test]
fn file_names() {
    let p = primary(storage_class::FILE, SectionNumber::Debug, 0, 0);
    // 18 bytes exactly (no NUL), then 19 (two records).
    let name = b"exactly-18-bytes.c";
    assert_eq!(
        decoded(&p, name, Format::Regular),
        Aux::File(name[..].into())
    );
    let mut two = b"nineteen-bytes-.cpp".to_vec();
    two.resize(36, 0);
    assert_eq!(
        decoded(&p, &two, Format::Regular),
        Aux::File(two[..19].to_vec().into())
    );
    let mut big = name.to_vec();
    big.resize(20, 0);
    assert_eq!(
        decoded(&p, &big, Format::BigObj),
        Aux::File(name[..].into())
    );
    // An empty name, a record more than the name needs, and bytes after
    // the NUL: kept raw.
    let mut extra = name.to_vec();
    extra.resize(36, 0);
    let mut junk = b"a.c".to_vec();
    junk.resize(18, 0);
    junk[10] = b'x';
    for r in [vec![0u8; 18], extra, junk] {
        assert!(
            matches!(decoded(&p, &r, Format::Regular), Aux::Raw(_)),
            "{r:?}"
        );
    }
}

#[test]
fn section_definitions() {
    for f in FORMATS {
        let p = primary(storage_class::STATIC, IN_SECTION, 0, 0);
        let r = record(
            f,
            &[
                (0, &0x1234u32.to_le_bytes()),
                (4, &5u16.to_le_bytes()),
                (6, &6u16.to_le_bytes()),
                (8, &0xDEAD_BEEFu32.to_le_bytes()),
                (12, &7u16.to_le_bytes()),
                (14, &[comdat_select::ASSOCIATIVE]),
            ],
        );
        assert_eq!(
            decoded(&p, &r, f),
            Aux::Section(SectionDefinition {
                length: 0x1234,
                relocations: 5,
                line_numbers: 6,
                checksum: 0xDEAD_BEEF,
                number: Some(SectionId(6)),
                selection: comdat_select::ASSOCIATIVE,
            })
        );
        // `bReserved`, and a section that doesn't exist: raw.
        for r in [
            record(f, &[(15, &[1])]),
            record(
                f,
                &[
                    (12, &0xFFFFu16.to_le_bytes()),
                    (16, &0xFFFFu16.to_le_bytes()),
                ],
            ),
        ] {
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)));
        }
        // C++/CLI's appdomain globals: absolute externals.
        let p = primary(storage_class::EXTERNAL, SectionNumber::Absolute, 0, 0);
        assert!(matches!(decoded(&p, &record(f, &[]), f), Aux::Section(_)));
    }
    // Big-obj's high 16 bits of the number.
    let p = primary(storage_class::STATIC, IN_SECTION, 0, 0);
    let r = record(
        Format::BigObj,
        &[
            (12, &0x0001u16.to_le_bytes()),
            (16, &0x0001u16.to_le_bytes()),
        ],
    );
    let Aux::Section(d) = decoded(&p, &r, Format::BigObj) else {
        panic!()
    };
    assert_eq!(d.number, Some(SectionId(0x1_0000)));
    // A regular object has no high number: bytes 16 and 17 are unused.
    let r = record(
        Format::Regular,
        &[
            (12, &0x0001u16.to_le_bytes()),
            (16, &0x0001u16.to_le_bytes()),
        ],
    );
    assert!(matches!(decoded(&p, &r, Format::Regular), Aux::Raw(_)));
    // Bytes 18 and 19 of a big-obj record are unused.
    let r = record(Format::BigObj, &[(18, &[1])]);
    assert!(matches!(decoded(&p, &r, Format::BigObj), Aux::Raw(_)));
}

#[test]
fn clr_tokens() {
    for f in FORMATS {
        let p = primary(
            storage_class::CLR_TOKEN,
            SectionNumber::Absolute,
            0,
            0x0600_0001,
        );
        let r = record(f, &[(0, &[1]), (2, &10u32.to_le_bytes())]);
        assert_eq!(
            decoded(&p, &r, f),
            Aux::ClrToken(ClrToken {
                aux_type: 1,
                symbol: SymbolId(5),
            })
        );
        for r in [
            record(f, &[(1, &[1])]),
            record(f, &[(6, &[1])]),
            record(f, &[(2, &1u32.to_le_bytes())]),
        ] {
            assert!(matches!(decoded(&p, &r, f), Aux::Raw(_)));
        }
    }
}

#[test]
fn other_records_are_raw() {
    for f in FORMATS {
        let stride = f.record_size();
        // A label with a record, two records where one is expected, and a
        // storage class with no typed form.
        let one = record(f, &[(0, &[9])]);
        let mut two = one.clone();
        two.extend_from_slice(&one);
        for (p, r) in [
            (primary(storage_class::LABEL, IN_SECTION, 0, 0), &one),
            (primary(storage_class::STATIC, IN_SECTION, 0, 0), &two),
            (
                primary(storage_class::WEAK_EXTERNAL, SectionNumber::Undefined, 0, 0),
                &two,
            ),
            (primary(storage_class::SECTION, IN_SECTION, 0, 0), &one),
        ] {
            let aux = decoded(&p, r, f);
            assert_eq!(aux, Aux::Raw(r.clone().into()));
            assert_eq!(aux.count(stride), r.len() / stride);
        }
        let p = primary(storage_class::STATIC, IN_SECTION, 0, 0);
        assert_eq!(decode(&p, &[], 0, f, &Fixed), Aux::None);
    }
}
