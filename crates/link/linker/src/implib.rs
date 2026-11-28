//! Import libraries (`/IMPLIB`): what a DLL's link writes beside it, for
//! programs that use the DLL to link against. A Microsoft archive whose
//! members are the import descriptor (`__IMPORT_DESCRIPTOR_x`), the null
//! import descriptor, the null thunk (`\x7fx_NULL_THUNK_DATA`), and a
//! short import object for each export not `PRIVATE`; with both linker
//! members (symbol tables) and a long-names member. As link.exe 14.52
//! writes it (docs/turbolink.md, "The import library"):
//!
//! - every member's date is -1, with or without `/Brepro`;
//! - each member's time stamp is the first 4 bytes of the SHA-256 of the
//!   member with its time stamp 0xFFFFFFFF, read little-endian, with the top
//!   two bits set;
//! - the three descriptor objects start with a `.debug$S` (`S_OBJNAME_ST`
//!   with the DLL's name, `S_COMPILE2_ST` for LINK) and an `@comp.id`;
//! - the import members are in the order of their names (bytes); the
//!   first linker member's symbols in the exports' order, each member's
//!   sorted;
//! - an export given an ordinal (`@n`) is imported by it, as `NONAME`'s.

use crate::config::Export;

/// `IMAGE_SCN_*` for the descriptors' sections.
const ALIGN_1: u32 = 0x0010_0000;
const ALIGN_2: u32 = 0x0020_0000;
const ALIGN_4: u32 = 0x0030_0000;
const ALIGN_8: u32 = 0x0040_0000;
const DATA_RW: u32 = 0x0000_0040 | 0x4000_0000 | 0x8000_0000;
/// `.debug$S`'s: initialized, discardable, read-only, on 1.
const DEBUG: u32 = 0x0000_0040 | 0x0200_0000 | 0x4000_0000 | ALIGN_1;

/// The descriptors' `@comp.id`: link.exe 14.52.37029's import library
/// (product 0x101, build 37029).
const COMP_ID: u32 = 0x0101_90A5;

/// One member: its name, its data, the symbols it defines, and what the
/// imports are ordered by (the imported name).
struct Member {
    name: String,
    data: Vec<u8>,
    symbols: Vec<Vec<u8>>,
    key: Vec<u8>,
}

/// A section to build: name, characteristics, contents, relocations
/// (address, symbol, type).
type BuiltSection = (&'static [u8], u32, Vec<u8>, Vec<(u32, u32, u16)>);

/// A COFF object with initialized-data sections, for the descriptors.
struct CoffBuilder {
    machine: u16,
    sections: Vec<BuiltSection>,
    symbols: Vec<(Vec<u8>, u32, i16, u8)>,
}

impl CoffBuilder {
    /// An object for `dll`'s descriptors, its `.debug$S` (section 1) and
    /// `@comp.id` (symbol 0) already in it.
    fn new(machine: u16, dll: &str) -> CoffBuilder {
        let mut o = CoffBuilder {
            machine,
            sections: Vec::new(),
            symbols: Vec::new(),
        };
        o.section(b".debug$S", DEBUG, debug_s(dll, machine), Vec::new());
        o.symbol(b"@comp.id", COMP_ID, -1, STATIC);
        o
    }

    fn section(
        &mut self,
        name: &'static [u8],
        chars: u32,
        data: Vec<u8>,
        relocs: Vec<(u32, u32, u16)>,
    ) {
        self.sections.push((name, chars, data, relocs));
    }

    /// A symbol: name, value, section number (1-based; 0 undefined, -1
    /// absolute), storage class.
    fn symbol(&mut self, name: &[u8], value: u32, section: i16, class: u8) {
        self.symbols.push((name.to_vec(), value, section, class));
    }

    fn build(self) -> Vec<u8> {
        let mut out = Vec::new();
        let n = self.sections.len();
        let headers = 20 + 40 * n;
        let mut at = headers;
        let mut placed = Vec::new();
        // A section without relocations has as its pointer to them the
        // last relocations' before it (0 if none), as link.exe's.
        let mut last_relocations = 0;
        for (_, _, data, relocs) in &self.sections {
            let raw = at;
            at += data.len();
            if !relocs.is_empty() {
                last_relocations = at;
            }
            at += relocs.len() * 10;
            placed.push((raw, last_relocations));
        }
        let symtab = at;
        out.extend_from_slice(&self.machine.to_le_bytes());
        out.extend_from_slice(&(crate::n16(n)).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(crate::n32(symtab)).to_le_bytes());
        out.extend_from_slice(&(crate::n32(self.symbols.len())).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for ((name, chars, data, relocs), (raw, rel)) in self.sections.iter().zip(&placed) {
            let mut h = [0u8; 8];
            h[..name.len()].copy_from_slice(name);
            out.extend_from_slice(&h);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(crate::n32(data.len())).to_le_bytes());
            out.extend_from_slice(&(crate::n32(*raw)).to_le_bytes());
            out.extend_from_slice(&(crate::n32(*rel)).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(crate::n16(relocs.len())).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&chars.to_le_bytes());
        }
        for (_, _, data, relocs) in &self.sections {
            out.extend_from_slice(data);
            for &(va, sym, ty) in relocs {
                out.extend_from_slice(&va.to_le_bytes());
                out.extend_from_slice(&sym.to_le_bytes());
                out.extend_from_slice(&ty.to_le_bytes());
            }
        }
        let mut strings: Vec<u8> = Vec::new();
        for (name, value, section, class) in &self.symbols {
            if name.len() <= 8 {
                let mut n = [0u8; 8];
                n[..name.len()].copy_from_slice(name);
                out.extend_from_slice(&n);
            } else {
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&(4 + crate::n32(strings.len())).to_le_bytes());
                strings.extend_from_slice(name);
                strings.push(0);
            }
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&section.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.push(*class);
            out.push(0);
        }
        out.extend_from_slice(&(4 + crate::n32(strings.len())).to_le_bytes());
        out.extend_from_slice(&strings);
        out
    }
}

const EXTERNAL: u8 = 2;
const STATIC: u8 = 3;
const SECTION: u8 = 104;

/// `IMAGE_REL_*_ADDR32NB` for `machine`.
fn addr32nb(machine: u16) -> u16 {
    match machine {
        0x14C => 7,                    // I386 DIR32NB
        0x1C4 => 2,                    // ARM ADDR32NB
        0xAA64 | 0xA641 | 0xA64E => 2, // ARM64 ADDR32NB
        _ => 3,                        // AMD64 ADDR32NB
    }
}

/// CodeView's machine (`CV_CFL_*`) for `machine`.
fn cv_machine(machine: u16) -> u16 {
    match machine {
        0x14C => 0x03,                    // 80386
        0x1C4 => 0xF4,                    // ARMNT
        0xAA64 | 0xA641 | 0xA64E => 0xF6, // ARM64
        _ => 0xD0,                        // AMD64
    }
}

/// A length-prefixed (Pascal) string, as the `_ST` records hold them.
fn pascal(out: &mut Vec<u8>, s: &[u8]) {
    let s = &s[..s.len().min(255)];
    out.push(u8::try_from(s.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(s);
}

/// The descriptors' `.debug$S`: signature 2 (records, no subsections),
/// `S_OBJNAME_ST` (signature 0, the DLL's name) and `S_COMPILE2_ST` (LINK,
/// the machine, front end 0.0.0, back end 14.52.37029, "Microsoft (R)
/// LINK"), not padded.
fn debug_s(dll: &str, machine: u16) -> Vec<u8> {
    let mut out = 2u32.to_le_bytes().to_vec();
    let mut record = |kind: u16, body: &[u8]| {
        out.extend_from_slice(&crate::n16(body.len() + 2).to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(body);
    };
    let mut b = 0u32.to_le_bytes().to_vec();
    pascal(&mut b, dll.as_bytes());
    record(0x0009, &b);
    let mut b = 7u32.to_le_bytes().to_vec();
    b.extend_from_slice(&cv_machine(machine).to_le_bytes());
    for v in [0u16, 0, 0, 14, 52, 37029] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    pascal(&mut b, b"Microsoft (R) LINK");
    record(0x1013, &b);
    out
}

/// The library name the descriptors are named for: the DLL's name without
/// its extension.
fn stem(dll: &str) -> &str {
    dll.rsplit_once('.').map_or(dll, |(s, _)| s)
}

fn descriptor(dll: &str, machine: u16) -> Member {
    let lib = stem(dll);
    let mut o = CoffBuilder::new(machine, dll);
    let nb = addr32nb(machine);
    // Relocations to .idata$6 (the name), .idata$4 and .idata$5.
    o.section(
        b".idata$2",
        ALIGN_4 | DATA_RW,
        vec![0; 20],
        vec![(12, 3, nb), (0, 4, nb), (16, 5, nb)],
    );
    let mut name = dll.as_bytes().to_vec();
    name.push(0);
    if name.len() % 2 == 1 {
        name.push(0);
    }
    o.section(b".idata$6", ALIGN_2 | DATA_RW, name, Vec::new());
    let descriptor = format!("__IMPORT_DESCRIPTOR_{lib}").into_bytes();
    o.symbol(&descriptor, 0, 2, EXTERNAL);
    o.symbol(b".idata$2", 0xC000_0040, 2, SECTION);
    o.symbol(b".idata$6", 0, 3, STATIC);
    o.symbol(b".idata$4", 0xC000_0040, 0, SECTION);
    o.symbol(b".idata$5", 0xC000_0040, 0, SECTION);
    o.symbol(b"__NULL_IMPORT_DESCRIPTOR", 0, 0, EXTERNAL);
    let thunk = format!("\x7f{lib}_NULL_THUNK_DATA").into_bytes();
    o.symbol(&thunk, 0, 0, EXTERNAL);
    Member {
        name: dll.to_owned(),
        data: o.build(),
        symbols: vec![descriptor],
        key: Vec::new(),
    }
}

fn null_descriptor(dll: &str, machine: u16) -> Member {
    let mut o = CoffBuilder::new(machine, dll);
    o.section(b".idata$3", ALIGN_4 | DATA_RW, vec![0; 20], Vec::new());
    o.symbol(b"__NULL_IMPORT_DESCRIPTOR", 0, 2, EXTERNAL);
    Member {
        name: dll.to_owned(),
        data: o.build(),
        symbols: vec![b"__NULL_IMPORT_DESCRIPTOR".to_vec()],
        key: Vec::new(),
    }
}

fn null_thunk(dll: &str, machine: u16, is64: bool) -> Member {
    let lib = stem(dll);
    let width = if is64 { 8 } else { 4 };
    let align = if is64 { ALIGN_8 } else { ALIGN_4 };
    let mut o = CoffBuilder::new(machine, dll);
    o.section(b".idata$5", align | DATA_RW, vec![0; width], Vec::new());
    o.section(b".idata$4", align | DATA_RW, vec![0; width], Vec::new());
    let thunk = format!("\x7f{lib}_NULL_THUNK_DATA").into_bytes();
    o.symbol(&thunk, 0, 2, EXTERNAL);
    Member {
        name: dll.to_owned(),
        data: o.build(),
        symbols: vec![thunk],
        key: Vec::new(),
    }
}

/// A short import object for `e` (exported by `dll` with ordinal
/// `e.ordinal` and hint `hint`), by ordinal if `by_ordinal`.
fn short_import(dll: &str, machine: u16, e: &Export, hint: u16, by_ordinal: bool) -> Member {
    // By ordinal (NONAME, or an ordinal given), or by the name as is
    // (x64's names aren't decorated; x86's are undecorated by link.exe: to
    // do with x86).
    let (name_type, ordinal_or_hint) = if e.noname || by_ordinal {
        (0u16, e.ordinal.unwrap_or(0))
    } else if e.export_as.is_some() {
        (4u16, hint)
    } else {
        (1u16, hint)
    };
    let kind: u16 = if e.data { 1 } else { 0 };
    let mut names = Vec::new();
    // The symbol programs refer to is the exported name.
    names.extend_from_slice(e.name.as_bytes());
    names.push(0);
    names.extend_from_slice(dll.as_bytes());
    names.push(0);
    if let Some(a) = &e.export_as {
        names.extend_from_slice(a.as_bytes());
        names.push(0);
    }
    let mut data = Vec::with_capacity(20 + names.len());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&machine.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&(crate::n32(names.len())).to_le_bytes());
    data.extend_from_slice(&ordinal_or_hint.to_le_bytes());
    data.extend_from_slice(&(kind | (name_type << 2)).to_le_bytes());
    data.extend_from_slice(&names);
    let mut symbols = vec![[b"__imp_".as_slice(), e.name.as_bytes()].concat()];
    if !e.data {
        symbols.push(e.name.as_bytes().to_vec());
    }
    Member {
        name: dll.to_owned(),
        data,
        symbols,
        key: e.name.as_bytes().to_vec(),
    }
}

/// Sets `member`'s time stamp (at `at`) as link.exe does: the first 4
/// bytes of the SHA-256 of the member with it 0xFFFFFFFF, little-endian,
/// with the top two bits set.
fn stamp(member: &mut [u8], at: usize) {
    use sha2::Digest;
    member[at..at + 4].fill(0xFF);
    let hash = sha2::Sha256::digest(&*member);
    let s = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]) | 0xC000_0000;
    member[at..at + 4].copy_from_slice(&s.to_le_bytes());
}

/// The import library for `dll`'s `exports` (from
/// [`crate::exports::finish`]; `hints[i]` is export `i`'s place in the
/// name table; `ordinals_given`, the exported names given an ordinal).
pub fn write(
    dll: &str,
    machine: u16,
    is64: bool,
    exports: &[Export],
    hints: &[u16],
    ordinals_given: &[String],
) -> Vec<u8> {
    let mut members = vec![
        descriptor(dll, machine),
        null_descriptor(dll, machine),
        null_thunk(dll, machine, is64),
    ];
    for m in &mut members {
        stamp(&mut m.data, 4);
    }
    let imports: Vec<(&Export, u16)> = exports
        .iter()
        .zip(hints)
        .filter(|(e, _)| !e.private)
        .map(|(e, &h)| (e, h))
        .collect();
    // (A SHA-256 each: on every core, for a DLL of many exports.)
    let built = turbonop_par::map(&imports, 256, |&(e, h)| {
        let given = ordinals_given
            .iter()
            .any(|n| n == crate::exports::exported_name(e));
        let mut m = short_import(dll, machine, e, h, given);
        stamp(&mut m.data, 8);
        m
    });
    members.extend(built);
    archive(members)
}

fn header(out: &mut Vec<u8>, name: &str, size: usize) {
    let field = |out: &mut Vec<u8>, s: &str, width: usize| {
        let b = s.as_bytes();
        out.extend_from_slice(&b[..b.len().min(width)]);
        out.extend(std::iter::repeat_n(b' ', width.saturating_sub(b.len())));
    };
    field(out, name, 16);
    field(out, "-1", 12);
    field(out, "", 6);
    field(out, "", 6);
    field(out, "0", 8);
    field(out, &size.to_string(), 10);
    out.extend_from_slice(b"`\n");
}

/// A Microsoft archive of `members` (the three descriptors, then the
/// imports in the exports' order): both linker members, the long names,
/// then the members, the imports by name.
fn archive(mut members: Vec<Member>) -> Vec<u8> {
    for m in &mut members {
        m.symbols.sort();
    }
    // The members' order in the file: the descriptors, then the imports
    // by their names.
    let mut order: Vec<usize> = (0..members.len()).collect();
    if order.len() > 3 {
        order[3..].sort_by(|&a, &b| members[a].key.cmp(&members[b].key));
    }
    // Long names (more than 15 bytes) go in `//`, NUL-terminated, each
    // once.
    let mut long_names: Vec<u8> = Vec::new();
    let mut long_at: Vec<(&str, usize)> = Vec::new();
    let mut names = Vec::with_capacity(members.len());
    for m in &members {
        names.push(if m.name.len() <= 15 {
            format!("{}/", m.name)
        } else if let Some(&(_, at)) = long_at.iter().find(|(n, _)| *n == m.name) {
            format!("/{at}")
        } else {
            let at = long_names.len();
            long_at.push((&m.name, at));
            long_names.extend_from_slice(m.name.as_bytes());
            long_names.push(0);
            format!("/{at}")
        });
    }
    let symbol_count: usize = members.iter().map(|m| m.symbols.len()).sum();
    let string_bytes: usize = members
        .iter()
        .flat_map(|m| &m.symbols)
        .map(|s| s.len() + 1)
        .sum();
    let first_size = 4 + 4 * symbol_count + string_bytes;
    let second_size = 4 + 4 * members.len() + 4 + 2 * symbol_count + string_bytes;
    let pad = |n: usize| n + (n & 1);
    let mut at = 8 + 60 + pad(first_size) + 60 + pad(second_size);
    if !long_names.is_empty() {
        at += 60 + pad(long_names.len());
    }
    let mut offsets = vec![0u32; members.len()];
    for &i in &order {
        offsets[i] = crate::n32(at);
        at += 60 + pad(members[i].data.len());
    }

    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(b"!<arch>\n");
    // The first linker member: big-endian offsets, the symbols in the
    // members' first order (the exports').
    header(&mut out, "/", first_size);
    out.extend_from_slice(&(crate::n32(symbol_count)).to_be_bytes());
    for (m, &o) in members.iter().zip(&offsets) {
        for _ in &m.symbols {
            out.extend_from_slice(&o.to_be_bytes());
        }
    }
    for s in members.iter().flat_map(|m| &m.symbols) {
        out.extend_from_slice(s);
        out.push(0);
    }
    if first_size % 2 == 1 {
        out.push(b'\n');
    }
    // The second: little-endian, the members by number (in their first
    // order too, not the file's), the symbols sorted.
    header(&mut out, "/", second_size);
    out.extend_from_slice(&(crate::n32(members.len())).to_le_bytes());
    for o in &offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&(crate::n32(symbol_count)).to_le_bytes());
    let mut sorted: Vec<(&[u8], u16)> = members
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            let n = crate::n16(i + 1);
            m.symbols.iter().map(move |s| (s.as_slice(), n))
        })
        .collect();
    sorted.sort();
    for (_, i) in &sorted {
        out.extend_from_slice(&i.to_le_bytes());
    }
    for (s, _) in &sorted {
        out.extend_from_slice(s);
        out.push(0);
    }
    if second_size % 2 == 1 {
        out.push(b'\n');
    }
    if !long_names.is_empty() {
        header(&mut out, "//", long_names.len());
        out.extend_from_slice(&long_names);
        if long_names.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    for &i in &order {
        let m = &members[i];
        header(&mut out, &names[i], m.data.len());
        out.extend_from_slice(&m.data);
        if m.data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbonop_archive::Archive;
    use turbonop_pe::import_object::{ImportObject, ImportType};

    #[test]
    fn readable_by_the_archive_reader() {
        let mut f = Export::parse("Func").unwrap();
        f.ordinal = Some(1);
        let mut d = Export::parse("Datum,DATA").unwrap();
        d.ordinal = Some(2);
        let mut p = Export::parse("Hidden,PRIVATE").unwrap();
        p.ordinal = Some(3);
        let lib = write(
            "a_long_library_name.dll",
            0x8664,
            true,
            &[f, d, p],
            &[0, 1, 2],
            &[],
        );
        let a = Archive::parse(&lib).unwrap();
        let mut names: Vec<String> = a
            .symbols()
            .iter()
            .map(|s| String::from_utf8_lossy(s.name).into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Func",
                "__IMPORT_DESCRIPTOR_a_long_library_name",
                "__NULL_IMPORT_DESCRIPTOR",
                "__imp_Datum",
                "__imp_Func",
                "\x7fa_long_library_name_NULL_THUNK_DATA",
            ]
        );
        for s in a.symbols() {
            let m = a.member_at_offset(s.member).unwrap();
            assert_eq!(&*m.name, b"a_long_library_name.dll");
            if s.name == b"__imp_Func" {
                let i = ImportObject::parse(m.data).unwrap();
                assert_eq!(i.kind, ImportType::Code);
                assert_eq!(i.dll, b"a_long_library_name.dll");
                assert_eq!(i.import_name(), Some(&b"Func"[..]));
            }
            if s.name == b"__IMPORT_DESCRIPTOR_a_long_library_name" {
                let o = crate::object::Object::parse(Box::leak(m.data.to_vec().into_boxed_slice()))
                    .unwrap();
                assert_eq!(o.sections.len(), 3);
                assert_eq!(o.sections[1].relocations.len(), 3);
            }
        }
    }
}
