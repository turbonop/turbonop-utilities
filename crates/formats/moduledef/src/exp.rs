//! The export file (`x.exp`) `lib /DEF` writes beside the import library:
//! a COFF object that a link of the DLL takes in place of the `.def` file,
//! holding its export table and its `.def` file's directives.
//!
//! Measured with `lib.exe` 14.52 on x64, x86, and ARM64 (one object):
//!
//! **Sections**: `.edata` (`0x40000040`: initialized data, read), then
//! `.debug$S` (`0x42100040`), then, if there are any, `.drectve`
//! (`0x200`), laid out as `cl` lays objects out.
//!
//! **`.edata`** is the `IMAGE_EXPORT_DIRECTORY` (40 bytes: its time stamp,
//! then the DLL's name, the ordinal base, the counts, and the three tables,
//! relocated), then the address table (4 bytes for each ordinal from the
//! base to the last), the name table and the ordinal table (one entry for
//! each export with a name, sorted by name), then the DLL's name, then each
//! export's name and, for a forwarder, its target (`other.Func`), each with
//! a NUL.
//!
//! The exports are in their import symbols' order ([`Resolved::symbol`]),
//! for everything but the name table: which ordinal one without an ordinal
//! gets, the strings, and the symbols below. The base is the least ordinal
//! given (1 if none was), or less if the exports wouldn't fit below 65536
//! (`A @65535` and `B` give base 65534, and `B` 65534); then each export
//! without an ordinal gets the least one from the base up not yet taken.
//!
//! **Symbols**: `@comp.id` (`0x010090A5`, absolute), `@feat.00` (`0x810`),
//! the two sections' symbols (static, no auxiliary records), `szName`,
//! `rgpv` (if any ordinals), `rgszName` and `rgwOrd` (if any names), the
//! names' symbols `$N<ordinal>` (five digits) in the name table's order,
//! then each export's address: its internal symbol, external and
//! undefined, or for a forwarder a static symbol, named the export's
//! symbol, at the target's string. The relocations (`ADDR32NB`): the
//! directory's four, then each export's address and (if it has a name)
//! the next name table entry.
//!
//! **`.debug$S`** (C13): `S_OBJNAME` (the export file as named), the
//! linker's `S_COMPILE3` (14.52.37029), `S_ENVBLOCK` (`cwd` and `exe`, the
//! linker's path), then an `S_EXPORT` for each export (its ordinal, flags
//! for `CONSTANT`, `DATA`, `PRIVATE`, `NONAME`, an ordinal given, and a
//! forwarder, and its internal symbol); then, if there was a `.def` file,
//! its SHA-256 (zeros for an empty file) and its full path as a file
//! checksum and the string table.
//!
//! **`.drectve`**: a UTF-8 byte order mark, then each directive after a
//! space ([`directives`]).
//!
//! The time stamps (the file header's and the directory's) are the time
//! `lib.exe` ran, or `0xFFFFFFFF` with `/Brepro`.

use alloc::borrow::Cow;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use sha2::{Digest, Sha256};
use turbonop_coff::{
    Format, Object, Relocation, Section, SectionData, SectionId, SectionNumber, Symbol, SymbolId,
    storage_class,
};
use turbonop_pe::consts::machine;

use crate::descriptors::count;
use crate::exports::Resolved;
use crate::{ExportKind, ModuleDefinition, ModuleKind, ModuleOption, SectionAttribute, Statement};

/// The `.def` file, as the export file records it.
#[derive(Clone, Copy, Debug)]
pub struct DefFile<'a> {
    /// Its full path.
    pub path: &'a [u8],
    /// Its contents.
    pub contents: &'a [u8],
}

/// What an export file holds.
#[derive(Clone, Copy, Debug)]
pub struct ExportFile<'a> {
    /// The machine.
    pub machine: u16,
    /// The DLL's file name.
    pub dll: &'a [u8],
    /// The exports, `PRIVATE` ones too.
    pub exports: &'a [Resolved<'a>],
    /// The directives (without the byte order mark; empty for none).
    pub directives: &'a [u8],
    /// The export file's name, as the output was named (`sub\x.exp`).
    pub name: &'a [u8],
    /// The current directory.
    pub cwd: &'a [u8],
    /// The linker's path (`lib.exe` is `link.exe /LIB`).
    pub exe: &'a [u8],
    /// The `.def` file, if there was one.
    pub def: Option<DefFile<'a>>,
    /// The time stamp: the time it ran, or `None` for `/Brepro`
    /// (`0xFFFFFFFF`).
    pub time: Option<u32>,
    /// The least ordinal the base may be, and the greatest ordinal the
    /// address table must span. An ARM64EC library's native export file
    /// (`x.a64n.exp`) takes the ARM64EC one's base and greatest given
    /// ordinal; an ARM64X library's ARM64EC file's base is at most its
    /// native exports' least given ordinal (measured).
    pub reserved: Option<(u16, Option<u16>)>,
    /// The least number of address table entries, beyond what the
    /// directory counts (measured: in an ARM64EC or ARM64X library's two
    /// export files, room for every export of both, and the native one's
    /// for the ARM64EC one's `NumberOfFunctions`).
    pub slots: u32,
}

/// `@comp.id`: product 0x0100, build 37029.
const COMP_ID: u32 = 0x0100_90A5;
/// `@feat.00`.
const FEAT: u32 = 0x810;

/// `S_EXPORT` flags.
mod flag {
    pub const CONSTANT: u16 = 0x01;
    pub const DATA: u16 = 0x02;
    pub const PRIVATE: u16 = 0x04;
    pub const NONAME: u16 = 0x08;
    pub const ORDINAL: u16 = 0x10;
    pub const FORWARDER: u16 = 0x20;
}

/// The ordinal base, and each export's ordinal, for `exports` in their
/// order (see the module's documentation); with `reserved`, the base is at
/// most its least.
pub fn ordinals(
    exports: &[Resolved<'_>],
    order: &[usize],
    reserved: Option<(u16, Option<u16>)>,
) -> (u16, Vec<u16>) {
    let mut taken = vec![false; 65536];
    let mut least: Option<u16> = reserved.map(|r| r.0);
    for e in exports {
        if let Some(o) = e.ordinal {
            taken[usize::from(o)] = true;
            least = Some(least.map_or(o, |l| l.min(o)));
        }
    }
    let n = exports.len();
    let mut base = u32::from(least.unwrap_or(1));
    if base + count(n) > 65536 {
        base = 65536u32.saturating_sub(count(n)).max(1);
    }
    let mut out: Vec<u16> = exports.iter().map(|e| e.ordinal.unwrap_or(0)).collect();
    let mut next = base;
    for &k in order {
        if exports[k].ordinal.is_some() {
            continue;
        }
        while next < 65536 && taken[next as usize] {
            next += 1;
        }
        let o = u16::try_from(next).unwrap_or(u16::MAX);
        out[k] = o;
        if next < 65536 {
            taken[next as usize] = true;
        }
    }
    (u16::try_from(base).unwrap_or(u16::MAX), out)
}

/// The greatest ordinal given in `exports`, if any.
pub fn greatest_given(exports: &[Resolved<'_>]) -> Option<u16> {
    exports.iter().filter_map(|e| e.ordinal).max()
}

/// The positions of `exports` sorted by their symbols.
pub fn symbol_order(exports: &[Resolved<'_>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..exports.len()).collect();
    order.sort_by(|&a, &b| exports[a].symbol.cmp(&exports[b].symbol));
    order
}

/// The export file's directives for `def` (each after a space, without the
/// byte order mark): `/DLL` (for `LIBRARY`) and `/OUT:` (quoted if the name
/// was) for the module, with its `/BASE:` (hexadecimal) and `/SUBSYSTEM:`
/// (`WINDOWS` for `WINDOWAPI`, `CONSOLE` for `WINDOWCOMPAT`); `/HEAP:` and
/// `/STACK:` (decimal, either half may be missing); `/VERSION:`; and a
/// `/SECTION:name,letters` for each section with attributes (of `D`, `E`,
/// `R`, `S`, and `W`, in that order), in the file's order. A module without
/// a name gives none.
pub fn directives(def: &ModuleDefinition<'_>, def_stem: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut put = |s: &[u8]| {
        out.push(b' ');
        out.extend_from_slice(s);
    };
    let sizes = |what: &str, s: crate::Sizes| {
        let r = s.reserve.map(|v| format!("{v}")).unwrap_or_default();
        match s.commit {
            Some(c) => format!("/{what}:{r},{c}"),
            None => format!("/{what}:{r}"),
        }
    };
    for st in &def.statements {
        match *st {
            Statement::Module => {
                let Some(m) = &def.module else { continue };
                if m.name.is_none() {
                    continue;
                }
                if m.kind == ModuleKind::Library {
                    put(b"/DLL");
                }
                let file = m.file_name(def_stem);
                let mut o = b"/OUT:".to_vec();
                if m.quoted {
                    o.push(b'"');
                }
                o.extend_from_slice(&file);
                if m.quoted {
                    o.push(b'"');
                }
                put(&o);
                for opt in &m.options {
                    match opt {
                        ModuleOption::Base(b) => put(format!("/BASE:0x{b:x}").as_bytes()),
                        ModuleOption::WindowApi => put(b"/SUBSYSTEM:WINDOWS"),
                        ModuleOption::WindowCompat => put(b"/SUBSYSTEM:CONSOLE"),
                    }
                }
            }
            Statement::Heap(s) => put(sizes("HEAP", s).as_bytes()),
            Statement::Stack(s) => put(sizes("STACK", s).as_bytes()),
            Statement::Version(v) => put(format!("/VERSION:{}.{}", v.major, v.minor).as_bytes()),
            Statement::Section(i) => {
                let s = &def.sections[i];
                let (mut d, mut e, mut r, mut sh, mut w) = (false, false, false, false, false);
                for a in &s.attributes {
                    match a {
                        SectionAttribute::Read | SectionAttribute::ReadOnly => r = true,
                        SectionAttribute::Write => w = true,
                        SectionAttribute::ReadWrite => (r, w) = (true, true),
                        SectionAttribute::Execute | SectionAttribute::ExecuteOnly => e = true,
                        SectionAttribute::ExecuteRead => (e, r) = (true, true),
                        SectionAttribute::Shared => sh = true,
                        SectionAttribute::Discardable => d = true,
                        SectionAttribute::NonShared => {}
                    }
                }
                let letters: Vec<u8> = [(d, b'D'), (e, b'E'), (r, b'R'), (sh, b'S'), (w, b'W')]
                    .iter()
                    .filter(|(on, _)| *on)
                    .map(|&(_, l)| l)
                    .collect();
                if !letters.is_empty() {
                    let mut o = b"/SECTION:".to_vec();
                    o.extend_from_slice(s.name);
                    o.push(b',');
                    o.extend_from_slice(&letters);
                    put(&o);
                }
            }
        }
    }
    out
}

impl ExportFile<'_> {
    fn addr32nb(&self) -> u16 {
        match self.machine {
            machine::I386 => 7,
            machine::ARM64 | machine::ARM64EC | machine::ARM64X => 2,
            _ => 3,
        }
    }

    /// The ordinal base.
    pub fn base(&self) -> u16 {
        ordinals(self.exports, &symbol_order(self.exports), self.reserved).0
    }

    /// The directory's `NumberOfFunctions`: from the base to the greatest
    /// ordinal (an export's, or the reserved range's).
    pub fn functions(&self) -> u32 {
        let order = symbol_order(self.exports);
        let (base, ords) = ordinals(self.exports, &order, self.reserved);
        ords.iter()
            .copied()
            .chain(self.reserved.and_then(|r| r.1))
            .map(|o| u32::from(o) + 1 - u32::from(base))
            .max()
            .unwrap_or(0)
    }

    /// The export file's bytes.
    pub fn write(&self) -> Vec<u8> {
        let stamp = self.time.unwrap_or(u32::MAX);
        let exports = self.exports;
        let order = symbol_order(exports);
        let (base, ords) = ordinals(exports, &order, self.reserved);
        let functions = self.functions();
        let mut names: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&k| !exports[k].noname)
            .collect();
        names.sort_by(|&a, &b| exports[a].name.cmp(&exports[b].name));
        let nn = count(names.len());

        // .edata's layout.
        let eat = 0x28u32;
        let slots = functions.max(self.slots);
        let name_table = eat + 4 * slots;
        let ordinal_table = name_table + 4 * nn;
        let sz_name = ordinal_table + 2 * nn;
        let mut data = vec![0u8; sz_name as usize];
        let put32 = |d: &mut [u8], at: u32, v: u32| {
            d[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        };
        put32(&mut data, 4, stamp);
        put32(&mut data, 0x10, u32::from(base));
        put32(&mut data, 0x14, functions);
        put32(&mut data, 0x18, nn);
        data.extend_from_slice(self.dll);
        data.push(0);
        // Each export's strings, in order.
        let mut name_at = vec![0u32; exports.len()];
        let mut forwarder_at = vec![0u32; exports.len()];
        for &k in &order {
            let e = &exports[k];
            if !e.noname {
                name_at[k] = count(data.len());
                data.extend_from_slice(&e.name);
                data.push(0);
            }
            if let Some(f) = e.forwarder {
                forwarder_at[k] = count(data.len());
                data.extend_from_slice(f);
                data.push(0);
            }
        }
        for (slot, &k) in names.iter().enumerate() {
            let at = ordinal_table + 2 * count(slot);
            let v = ords[k] - base;
            data[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
        }

        // Symbols.
        let mut o = Object::new(Format::Regular, self.machine);
        if self.machine == machine::I386 {
            o.characteristics = turbonop_pe::consts::file_characteristics::MACHINE_32BIT;
        }
        o.time_date_stamp = stamp;
        let edata = SectionNumber::Section(SectionId::from_number(1).unwrap_or(SectionId(0)));
        let debug = SectionNumber::Section(SectionId::from_number(2).unwrap_or(SectionId(0)));
        let stat = |name: Cow<'static, [u8]>, value: u32, section| {
            Symbol::new(
                turbonop_coff::Name::new(name),
                value,
                section,
                0,
                storage_class::STATIC,
            )
        };
        o.symbols.push(stat(
            Cow::Borrowed(b"@comp.id"),
            COMP_ID,
            SectionNumber::Absolute,
        ));
        o.symbols.push(stat(
            Cow::Borrowed(b"@feat.00"),
            FEAT,
            SectionNumber::Absolute,
        ));
        o.symbols.push(stat(Cow::Borrowed(b".edata"), 0, edata));
        o.symbols.push(stat(Cow::Borrowed(b".debug$S"), 0, debug));
        let sym_sz_name = SymbolId(count(o.symbols.len()));
        o.symbols
            .push(stat(Cow::Borrowed(b"szName"), sz_name, edata));
        let sym_rgpv = (slots > 0).then(|| {
            o.symbols.push(stat(Cow::Borrowed(b"rgpv"), eat, edata));
            SymbolId(count(o.symbols.len() - 1))
        });
        let tables = (nn > 0).then(|| {
            o.symbols
                .push(stat(Cow::Borrowed(b"rgszName"), name_table, edata));
            o.symbols
                .push(stat(Cow::Borrowed(b"rgwOrd"), ordinal_table, edata));
            (
                SymbolId(count(o.symbols.len() - 2)),
                SymbolId(count(o.symbols.len() - 1)),
            )
        });
        let first_name = count(o.symbols.len());
        for &k in &names {
            let n = format!("$N{:05}", ords[k]);
            o.symbols
                .push(stat(Cow::Owned(n.into_bytes()), name_at[k], edata));
        }
        // ARM64EC: each code export's address is its x64 entry thunk's.
        let ec = self.machine == machine::ARM64EC;
        let thunked: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&k| {
                ec && exports[k].kind == ExportKind::Code && exports[k].forwarder.is_none()
            })
            .collect();
        let mut internal_symbol = vec![SymbolId(0); exports.len()];
        let mut address = vec![SymbolId(0); exports.len()];
        for &k in &order {
            let e = &exports[k];
            address[k] = SymbolId(count(o.symbols.len()));
            internal_symbol[k] = address[k];
            o.symbols.push(match e.forwarder {
                Some(_) => stat(Cow::Owned(e.symbol.to_vec()), forwarder_at[k], edata),
                None => Symbol::new(
                    e.internal.to_vec(),
                    0,
                    SectionNumber::Undefined,
                    0,
                    storage_class::EXTERNAL,
                ),
            });
        }

        let thunk_section = if self.directives.is_empty() { 3 } else { 4 };
        let thunk_section =
            SectionNumber::Section(SectionId::from_number(thunk_section).unwrap_or(SectionId(0)));
        let mut thunk_symbols = Vec::with_capacity(thunked.len());
        for (i, &k) in thunked.iter().enumerate() {
            let mut name = b"EXP+".to_vec();
            name.extend_from_slice(&exports[k].symbol);
            address[k] = SymbolId(count(o.symbols.len()));
            thunk_symbols.push(address[k]);
            o.symbols.push(Symbol::new(
                name,
                16 * count(i),
                thunk_section,
                turbonop_coff::symbol_type::FUNCTION,
                storage_class::EXTERNAL,
            ));
        }

        // Relocations.
        let kind = self.addr32nb();
        let mut relocs = vec![Relocation {
            offset: 0x0C,
            symbol: sym_sz_name,
            kind,
        }];
        if let Some(s) = sym_rgpv {
            relocs.push(Relocation {
                offset: 0x1C,
                symbol: s,
                kind,
            });
        }
        if let Some((n, w)) = tables {
            relocs.push(Relocation {
                offset: 0x20,
                symbol: n,
                kind,
            });
            relocs.push(Relocation {
                offset: 0x24,
                symbol: w,
                kind,
            });
        }
        let mut slot = 0u32;
        for &k in &order {
            relocs.push(Relocation {
                offset: eat + 4 * (u32::from(ords[k]) - u32::from(base)),
                symbol: address[k],
                kind,
            });
            if !exports[k].noname {
                relocs.push(Relocation {
                    offset: name_table + 4 * slot,
                    symbol: SymbolId(first_name + slot),
                    kind,
                });
                slot += 1;
            }
        }

        let mut edata_section =
            Section::new(&b".edata"[..], 0x4000_0040, SectionData::Bytes(data.into()));
        edata_section.relocations = relocs;
        o.sections.push(edata_section);
        let cv = self.debug_s(&order, &ords);
        o.sections.push(Section::new(
            &b".debug$S"[..],
            0x4210_0040,
            SectionData::Bytes(cv.into()),
        ));
        if !self.directives.is_empty() {
            let mut d = b"\xEF\xBB\xBF".to_vec();
            d.extend_from_slice(self.directives);
            o.sections.push(Section::new(
                &b".drectve"[..],
                0x200,
                SectionData::Bytes(d.into()),
            ));
        }
        if !thunked.is_empty() {
            self.add_thunks(&mut o, &thunked, &internal_symbol, &thunk_symbols);
        }
        pad_edata_relocations(thunk_names_in_table(o.write().unwrap_or_default()))
    }

    /// An ARM64EC export file's x64 entry thunks (`.hexpthk`: each
    /// `mov rax, rsp; mov [rax+20h], rbx; push rbp; pop rbp; jmp` to the
    /// export, then `int 3` twice), their symbols' indices (`.gfids$y`),
    /// and an `S_THUNK32` and an `S_END` for each (a second `.debug$S`).
    fn add_thunks(
        &self,
        o: &mut Object<'_>,
        thunked: &[usize],
        internal: &[SymbolId],
        thunks: &[SymbolId],
    ) {
        const THUNK: [u8; 16] = [
            0x48, 0x8B, 0xC4, 0x48, 0x89, 0x58, 0x20, 0x55, 0x5D, 0xE9, 0, 0, 0, 0, 0xCC, 0xCC,
        ];
        let mut code = Vec::with_capacity(16 * thunked.len());
        let mut relocs = Vec::with_capacity(thunked.len());
        for (i, &k) in thunked.iter().enumerate() {
            code.extend_from_slice(&THUNK);
            relocs.push(Relocation {
                offset: 16 * count(i) + 10,
                symbol: internal[k],
                // IMAGE_REL_AMD64_REL32: the thunks are x64 code.
                kind: 4,
            });
        }
        let mut text = Section::new(
            &b".hexpthk"[..],
            0x6050_8020,
            SectionData::Bytes(code.into()),
        );
        text.relocations = relocs;
        o.sections.push(text);
        let mut gfids = Vec::with_capacity(4 * thunks.len());
        for s in thunks {
            let index = o.symbol_table_index(*s).unwrap_or(0);
            gfids.extend_from_slice(&index.to_le_bytes());
        }
        o.sections.push(Section::new(
            &b".gfids$y"[..],
            0x4030_0040,
            SectionData::Bytes(gfids.into()),
        ));
        let mut records = Vec::new();
        let mut relocs = Vec::new();
        for (i, &k) in thunked.iter().enumerate() {
            let mut name = b"EXP+".to_vec();
            name.extend_from_slice(&self.exports[k].symbol);
            name.push(0);
            let len = u16::try_from(2 + 21 + name.len()).unwrap_or(u16::MAX);
            let at = 12 + count(records.len());
            records.extend_from_slice(&len.to_le_bytes());
            records.extend_from_slice(&0x1102u16.to_le_bytes());
            records.extend_from_slice(&[0; 16]);
            records.extend_from_slice(&0u16.to_le_bytes());
            records.extend_from_slice(&16u16.to_le_bytes());
            records.push(0);
            records.extend_from_slice(&name);
            records.extend_from_slice(&[2, 0, 6, 0]);
            // IMAGE_REL_ARM64_SECREL and IMAGE_REL_ARM64_SECTION.
            relocs.push(Relocation {
                offset: at + 16,
                symbol: thunks[i],
                kind: 8,
            });
            relocs.push(Relocation {
                offset: at + 20,
                symbol: thunks[i],
                kind: 13,
            });
        }
        let mut cv = 4u32.to_le_bytes().to_vec();
        cv.extend_from_slice(&0xF1u32.to_le_bytes());
        cv.extend_from_slice(&count(records.len()).to_le_bytes());
        cv.extend_from_slice(&records);
        // Measured: one byte after an odd size (0x121 and 0x5F).
        if cv.len() % 2 == 1 {
            cv.push(0);
        }
        let mut debug = Section::new(&b".debug$S"[..], 0x4210_0040, SectionData::Bytes(cv.into()));
        debug.relocations = relocs;
        o.sections.push(debug);
    }

    /// `.debug$S`.
    fn debug_s(&self, order: &[usize], ords: &[u16]) -> Vec<u8> {
        let mut records = Vec::new();
        let mut record = |kind: u16, body: &[u8]| {
            let len = u16::try_from(body.len() + 2).unwrap_or(u16::MAX);
            records.extend_from_slice(&len.to_le_bytes());
            records.extend_from_slice(&kind.to_le_bytes());
            records.extend_from_slice(body);
        };
        // S_OBJNAME.
        let mut b = 0u32.to_le_bytes().to_vec();
        b.extend_from_slice(self.name);
        b.push(0);
        record(0x1101, &b);
        // S_COMPILE3.
        let cv_machine: u16 = match self.machine {
            machine::I386 => 0x03,
            // A native export file beside an ARM64EC one says ARM64EC.
            machine::ARM64 if self.slots > 0 || self.reserved.is_some() => 0xF8,
            machine::ARM64 => 0xF6,
            machine::ARM64EC | machine::ARM64X => 0xF8,
            _ => 0xD0,
        };
        let mut b = 0x0008_0007u32.to_le_bytes().to_vec();
        b.extend_from_slice(&cv_machine.to_le_bytes());
        for v in [0u16, 0, 0, 0, 14, 52, 37029, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(b"Microsoft (R) LINK\0");
        record(0x113C, &b);
        // S_ENVBLOCK.
        let mut b = vec![0u8];
        for s in [&b"cwd"[..], self.cwd, b"exe", self.exe] {
            b.extend_from_slice(s);
            b.push(0);
        }
        b.push(0);
        record(0x113D, &b);
        // S_EXPORT.
        for &k in order {
            let e = &self.exports[k];
            let mut flags = 0;
            match e.kind {
                ExportKind::Constant => flags |= flag::CONSTANT,
                ExportKind::Data => flags |= flag::DATA,
                ExportKind::Code => {}
            }
            if e.private {
                flags |= flag::PRIVATE;
            }
            if e.noname {
                flags |= flag::NONAME;
            }
            if e.ordinal.is_some() {
                flags |= flag::ORDINAL;
            }
            if e.forwarder.is_some() {
                flags |= flag::FORWARDER;
            }
            let mut b = ords[k].to_le_bytes().to_vec();
            b.extend_from_slice(&flags.to_le_bytes());
            b.extend_from_slice(&e.internal);
            b.push(0);
            record(0x1138, &b);
        }
        let mut cv = 4u32.to_le_bytes().to_vec();
        let subsection = |cv: &mut Vec<u8>, kind: u32, body: &[u8], pad: bool| {
            cv.extend_from_slice(&kind.to_le_bytes());
            cv.extend_from_slice(&count(body.len()).to_le_bytes());
            cv.extend_from_slice(body);
            if pad {
                while !cv.len().is_multiple_of(4) {
                    cv.push(0);
                }
            }
        };
        let def = self.def;
        subsection(&mut cv, 0xF1, &records, true);
        if let Some(def) = def {
            let mut b = 1u32.to_le_bytes().to_vec();
            b.extend_from_slice(&[0x20, 3]);
            if def.contents.is_empty() {
                b.extend_from_slice(&[0; 32]);
            } else {
                b.extend_from_slice(&Sha256::digest(def.contents));
            }
            b.extend_from_slice(&[0, 0]);
            subsection(&mut cv, 0xF4, &b, true);
            let mut b = vec![0u8];
            b.extend_from_slice(def.path);
            b.push(0);
            subsection(&mut cv, 0xF3, &b, false);
        }
        cv
    }
}

/// `lib.exe` stores a thunk's name (`EXP+...`) in the string table even
/// when it fits the symbol's 8 bytes (measured: `EXP+#Ren`), which the
/// writer stores in place: the string table is made again, each symbol's
/// long name in order.
fn thunk_names_in_table(mut b: Vec<u8>) -> Vec<u8> {
    let get = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (table, n) = (get(&b, 8) as usize, get(&b, 12) as usize);
    let strings = table + 18 * n;
    if b.len() < strings + 4 {
        return b;
    }
    let forced = |name: &[u8; 8]| name.starts_with(b"EXP+") && !name.contains(&0);
    let mut any = false;
    let mut i = 0;
    while i < n {
        let at = table + 18 * i;
        any |= forced(&b[at..at + 8].try_into().unwrap_or_default());
        i += 1 + usize::from(b[at + 17]);
    }
    if !any {
        return b;
    }
    let mut new = Vec::new();
    let mut i = 0;
    while i < n {
        let at = table + 18 * i;
        let field: [u8; 8] = b[at..at + 8].try_into().unwrap_or_default();
        let name: Option<Vec<u8>> = if field[..4] == [0; 4] {
            let off = strings + get(&b, at + 4) as usize;
            let end = b[off..]
                .iter()
                .position(|&c| c == 0)
                .map_or(b.len(), |e| off + e);
            Some(b[off..end].to_vec())
        } else if forced(&field) {
            Some(field.to_vec())
        } else {
            None
        };
        if let Some(name) = name {
            let offset = 4 + count(new.len());
            b[at..at + 4].copy_from_slice(&[0; 4]);
            b[at + 4..at + 8].copy_from_slice(&offset.to_le_bytes());
            new.extend_from_slice(&name);
            new.push(0);
        }
        i += 1 + usize::from(b[at + 17]);
    }
    b.truncate(strings);
    b.extend_from_slice(&(4 + count(new.len())).to_le_bytes());
    b.extend_from_slice(&new);
    b
}

/// `lib.exe` starts `.edata`'s relocations at an even offset (measured: a
/// byte after 0x41 bytes of data); the writer packs them, so a byte is put
/// in and every later part moves.
fn pad_edata_relocations(mut b: Vec<u8>) -> Vec<u8> {
    let get = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    if b.len() < 60 {
        return b;
    }
    let (size, data, relocs) = (get(&b, 36), get(&b, 40), get(&b, 44));
    let end = data + size;
    if relocs == 0 || relocs != end || end % 2 == 0 {
        return b;
    }
    let sections = usize::from(u16::from_le_bytes([b[2], b[3]]));
    b.insert(end as usize, 0);
    let bump = |b: &mut Vec<u8>, at: usize| {
        let v = get(b, at);
        if v >= end && v != 0 {
            b[at..at + 4].copy_from_slice(&(v + 1).to_le_bytes());
        }
    };
    bump(&mut b, 8);
    for i in 0..sections {
        for field in [20, 24, 28] {
            bump(&mut b, 20 + 40 * i + field);
        }
    }
    b
}

#[cfg(test)]
mod tests;
