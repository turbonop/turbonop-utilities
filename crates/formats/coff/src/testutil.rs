//! Test inputs: the COFF test objects, and a synthetic object that uses
//! every part of the model.

extern crate std;

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use turbonop_pe::consts::machine;
use turbonop_pe::consts::section_characteristics as scn;

use crate::object::{
    Aux, BeginEnd, ClrToken, Format, FunctionDefinition, LineNumber, LineNumberRef, Object,
    Relocation, Section, SectionData, SectionDefinition, SectionId, SectionNumber, Symbol,
    SymbolId, WeakExternal, comdat_select, storage_class as sc, weak_search,
};

/// A file of `testdata/coff`.
pub(crate) fn testdata(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/coff")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Whether two objects hold the same thing, their layouts aside (and the
/// relocation-overflow flag, which a fresh layout sets as needed).
pub(crate) fn same_content(a: &Object<'_>, b: &Object<'_>) -> bool {
    let sections = a.sections.len() == b.sections.len()
        && a.sections.iter().zip(&b.sections).all(|(x, y)| {
            x.name == y.name
                && x.virtual_size == y.virtual_size
                && x.virtual_address == y.virtual_address
                && x.characteristics & !scn::LNK_NRELOC_OVFL
                    == y.characteristics & !scn::LNK_NRELOC_OVFL
                && x.data == y.data
                && x.relocations == y.relocations
                && x.line_numbers == y.line_numbers
        });
    a.format == b.format
        && a.machine == b.machine
        && a.time_date_stamp == b.time_date_stamp
        && a.characteristics == b.characteristics
        && a.optional_header == b.optional_header
        && (a.format == Format::Regular || a.bigobj == b.bigobj)
        && a.symbols == b.symbols
        && sections
}

/// The first difference between two objects' contents, described.
pub(crate) fn difference(a: &Object<'_>, b: &Object<'_>) -> std::string::String {
    use std::format;
    for (i, (x, y)) in a.symbols.iter().zip(&b.symbols).enumerate() {
        if x != y {
            return format!("symbol {i}: {x:?} vs {y:?}");
        }
    }
    for (i, (x, y)) in a.sections.iter().zip(&b.sections).enumerate() {
        if x.name != y.name
            || x.data != y.data
            || x.relocations != y.relocations
            || x.line_numbers != y.line_numbers
            || x.characteristics != y.characteristics
        {
            return format!(
                "section {i}: {:?} {:?} vs {:?} {:?}",
                x.name, x.line_numbers, y.name, y.line_numbers
            );
        }
    }
    format!(
        "{} vs {} sections, {} vs {} symbols",
        a.sections.len(),
        b.sections.len(),
        a.symbols.len(),
        b.symbols.len()
    )
}

/// Writes `object`, reads it back, and checks that the copy holds the same
/// and writes the same bytes. Returns the bytes.
pub(crate) fn round_trip(object: &Object<'_>) -> Vec<u8> {
    let bytes = object.write().expect("write");
    let again = Object::parse(&bytes).expect("read back");
    assert!(
        same_content(object, &again),
        "read back differently:\n{object:#?}\n{again:#?}"
    );
    assert_eq!(again.write().expect("write again"), bytes);
    bytes
}

fn sym(
    name: &'static [u8],
    value: u32,
    section: SectionNumber,
    kind: u16,
    class: u8,
) -> Symbol<'static> {
    Symbol::new(name, value, section, kind, class)
}

fn in_section(n: u32) -> SectionNumber {
    SectionNumber::Section(SectionId(n))
}

fn definition(section: &Section<'_>, selection: u8, number: Option<u32>) -> Aux<'static> {
    Aux::Section(SectionDefinition {
        length: section.size(),
        relocations: section.relocations.len() as u16,
        line_numbers: section.line_numbers.len() as u16,
        checksum: crate::checksum(section.bytes()),
        number: number.map(SectionId),
        selection,
    })
}

/// The ids of [`sample`]'s symbols.
pub(crate) mod ids {
    use crate::object::SymbolId;

    pub const FILE: SymbolId = SymbolId(0);
    /// The section symbols: section `n`'s is `SECTION_SYMBOLS + n`.
    pub const SECTION_SYMBOLS: u32 = 2;
    pub const MAIN: SymbolId = SymbolId(9);
    pub const BF: SymbolId = SymbolId(10);
    pub const LEADER: SymbolId = SymbolId(13);
    pub const UNDEFINED: SymbolId = SymbolId(14);
    pub const CLR_TOKEN: SymbolId = SymbolId(19);
    pub const RAW: SymbolId = SymbolId(20);
    pub const UNWIND_LABEL: SymbolId = SymbolId(24);
}

/// An object with every kind of section, symbol, and auxiliary record:
///
/// | Section | |
/// |---|---|
/// | 0 `.text$mn` | code, relocations, line numbers |
/// | 1 `.data` | data |
/// | 2 `.bss` | uninitialized |
/// | 3 `.a_long_section_name` | a long name |
/// | 4 `.text$mn` | a COMDAT (`any`) |
/// | 5 `.xdata` | associative to section 4 |
/// | 6 `.drectve` | empty |
///
/// Symbols: 0 `.file` (a name of 40 bytes: 3 records of 18, 2 of 20), 1
/// `@comp.id`, 2..=8 the sections' symbols, 9 `main` (a function
/// definition), 10 `.bf`, 11 `.lf`, 12 `.ef`, 13 `comdat_leader`, 14
/// `undefined_function_name`, 15..=18 weak externals of each search kind,
/// 19 a CLR token, 20 `raw_aux` (two raw records), 21 `exactly8` (an
/// 8-byte name), 22 the empty name, 23 a common symbol, 24 a label in
/// `.xdata`.
pub(crate) fn sample(format: Format) -> Object<'static> {
    let mut o = Object::new(format, machine::AMD64);
    o.time_date_stamp = 0x1234_5678;
    let code = scn::CNT_CODE | scn::MEM_EXECUTE | scn::MEM_READ | (5 << 20);
    let data = scn::CNT_INITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE | (4 << 20);
    let mut text = Section::new(
        ".text$mn",
        code,
        SectionData::Bytes(Cow::Owned(vec![0x90; 32])),
    );
    text.relocations = vec![
        Relocation {
            offset: 4,
            symbol: ids::UNDEFINED,
            kind: 4,
        },
        Relocation {
            offset: 12,
            symbol: SymbolId(ids::SECTION_SYMBOLS + 1),
            kind: 4,
        },
        Relocation {
            offset: 20,
            symbol: ids::LEADER,
            kind: 4,
        },
    ];
    text.line_numbers = vec![
        LineNumber::Function(ids::MAIN),
        LineNumber::Line {
            address: 4,
            number: 1,
        },
        LineNumber::Line {
            address: 12,
            number: 2,
        },
    ];
    o.sections.push(text);
    o.sections.push(Section::new(
        ".data",
        data,
        SectionData::Bytes(Cow::Owned((0..16).collect())),
    ));
    o.sections.push(Section::new(
        ".bss",
        scn::CNT_UNINITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE,
        SectionData::Uninitialized(0x400),
    ));
    o.sections.push(Section::new(
        ".a_long_section_name",
        scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
        SectionData::Bytes(Cow::Owned(b"long".to_vec())),
    ));
    o.sections.push(Section::new(
        ".text$mn",
        code | scn::LNK_COMDAT,
        SectionData::Bytes(Cow::Owned(vec![0xC3])),
    ));
    let mut xdata = Section::new(
        ".xdata",
        scn::CNT_INITIALIZED_DATA | scn::MEM_READ | scn::LNK_COMDAT,
        SectionData::Bytes(Cow::Owned(vec![1, 0, 0, 0])),
    );
    xdata.relocations = vec![Relocation {
        offset: 0,
        symbol: ids::LEADER,
        kind: 3,
    }];
    o.sections.push(xdata);
    o.sections.push(Section::new(
        ".drectve",
        scn::LNK_INFO | scn::LNK_REMOVE,
        SectionData::Bytes(Cow::Borrowed(&[])),
    ));

    let mut file = sym(b".file", 0, SectionNumber::Debug, 0, sc::FILE);
    file.aux = Aux::File(Cow::Borrowed(
        b"C:\\a\\path\\long\\enough\\for\\three\\recs.cpp",
    ));
    o.symbols.push(file);
    o.symbols.push(sym(
        b"@comp.id",
        0x0101_9F5E,
        SectionNumber::Absolute,
        0,
        sc::STATIC,
    ));
    let selections = [
        (0, 0, None),
        (1, 0, None),
        (2, 0, None),
        (3, 0, None),
        (4, comdat_select::ANY, None),
        (5, comdat_select::ASSOCIATIVE, Some(4)),
        (6, 0, None),
    ];
    for (i, selection, number) in selections {
        let name: &'static [u8] = match i {
            0 | 4 => b".text$mn",
            1 => b".data",
            2 => b".bss",
            3 => b".a_long_section_name",
            5 => b".xdata",
            _ => b".drectve",
        };
        let mut s = sym(name, 0, in_section(i), 0, sc::STATIC);
        s.aux = definition(&o.sections[i as usize], selection, number);
        o.symbols.push(s);
    }
    let mut main = sym(b"main", 0, in_section(0), 0x20, sc::EXTERNAL);
    main.aux = Aux::Function(FunctionDefinition {
        tag: Some(ids::BF),
        total_size: 32,
        line_numbers: Some(LineNumberRef {
            section: SectionId(0),
            index: 0,
        }),
        next: None,
    });
    o.symbols.push(main);
    let mut bf = sym(b".bf", 0, in_section(0), 0, sc::FUNCTION);
    bf.aux = Aux::BeginEnd(BeginEnd {
        line: 10,
        next: Some(ids::LEADER),
    });
    o.symbols.push(bf);
    o.symbols
        .push(sym(b".lf", 3, in_section(0), 0, sc::FUNCTION));
    let mut ef = sym(b".ef", 32, in_section(0), 0, sc::FUNCTION);
    ef.aux = Aux::BeginEnd(BeginEnd {
        line: 12,
        next: None,
    });
    o.symbols.push(ef);
    o.symbols
        .push(sym(b"comdat_leader", 0, in_section(4), 0x20, sc::EXTERNAL));
    o.symbols.push(sym(
        b"undefined_function_name",
        0,
        SectionNumber::Undefined,
        0x20,
        sc::EXTERNAL,
    ));
    for (name, search) in [
        (&b"weak_nolibrary"[..], weak_search::NOLIBRARY),
        (b"weak_library", weak_search::LIBRARY),
        (b"weak_alias", weak_search::ALIAS),
        (b"#weak_anti_dependency", weak_search::ANTI_DEPENDENCY),
    ] {
        let mut w = sym(name, 0, SectionNumber::Undefined, 0, sc::WEAK_EXTERNAL);
        w.aux = Aux::WeakExternal(WeakExternal {
            tag: ids::MAIN,
            search,
        });
        o.symbols.push(w);
    }
    let mut token = sym(
        b"clr_token",
        0x0600_0001,
        SectionNumber::Absolute,
        0,
        sc::CLR_TOKEN,
    );
    token.aux = Aux::ClrToken(ClrToken {
        aux_type: 1,
        symbol: ids::MAIN,
    });
    o.symbols.push(token);
    let stride = format.record_size();
    let mut raw = sym(b"raw_aux", 8, in_section(1), 0, sc::EXTERNAL);
    raw.aux = Aux::Raw(Cow::Owned((1..=(2 * stride) as u8).collect()));
    o.symbols.push(raw);
    o.symbols
        .push(sym(b"exactly8", 0, in_section(1), 0, sc::EXTERNAL));
    o.symbols.push(sym(b"", 4, in_section(1), 0, sc::STATIC));
    o.symbols.push(sym(
        b"common_symbol_name",
        64,
        SectionNumber::Undefined,
        0,
        sc::EXTERNAL,
    ));
    o.symbols
        .push(sym(b"$unwind$leader", 0, in_section(5), 0, sc::STATIC));
    o
}
