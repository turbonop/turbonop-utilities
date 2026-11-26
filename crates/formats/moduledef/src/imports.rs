//! The import library `lib /DEF` makes of a module definition (with no
//! objects on its command line), as measured with MSVC 14.52's `lib.exe`
//! on x64, x86, ARM64, and ARM64EC.
//!
//! # Its members
//!
//! The archive holds three import descriptor objects, then one short
//! import object for each export that isn't `PRIVATE` (each name once:
//! the first entry's), sorted by their symbols' bytes. Every member is
//! named for the DLL (`foo.dll`). The second linker member lists the
//! members in another order, and its symbols name them by their places in
//! it: the three descriptor objects, then the imports in the `.def`
//! file's order. The first linker member lists the symbols in that order
//! too, each member's sorted (see `turbonop_archive::write`).
//!
//! Each import ([`Import`]):
//!
//! - Its symbol is the export's name. On x86, `_` is put before a name
//!   that doesn't start with `?` or `@` (`_Under` becomes `__Under`). On
//!   ARM64EC, code's symbol is mangled (`#f`, or `$$h` after a C++ name's
//!   qualified name: `?f@@$$hYAXXZ`).
//! - It is code, data (`DATA`), or a constant (`CONSTANT`).
//! - An export with an ordinal (`NONAME` or not) is imported by ordinal,
//!   with the ordinal as its hint. Another is imported by name, its hint
//!   its place among the exports that have names (all but `NONAME` ones,
//!   `PRIVATE` ones included) sorted by the names they are exported as
//!   (the `EXPORTAS` name, if any). On ARM64EC the hints of imports by
//!   name are not that: they are 0, or bytes of memory `lib.exe` didn't
//!   set (values up to 22639 were seen, and one `.def` file gave 26982
//!   for an import in one run and 0 in another). They are 0 here.
//! - The name type: by name; on x86 `NOPREFIX` when `_` was put before
//!   it; `EXPORTAS` with the `EXPORTAS` name; on ARM64EC, code by name is
//!   `EXPORTAS` with the plain name, and code by ordinal is by ordinal
//!   but still carries the plain name.
//! - Its time stamp: the time `lib.exe` ran; with `/Brepro`, a hash of the
//!   object ([`crate::descriptors::brepro_stamp`]).
//!
//! (`lib /DEF` given objects too, or `link /DEF`, finds each export's
//! symbol in them: an x86 `Func` defined as `_Func@8` is imported as
//! `_Func@8`, `UNDECORATE`; `Ren=Func` as `_Ren@8`, `UNDECORATE`;
//! `Ren2=_Func@8` as `Ren2`, by name; `@Fa@4` (fastcall) `UNDECORATE`;
//! `Cd` (cdecl) as `_Cd`, `NOPREFIX`. That is not done here.)
//!
//! # The import descriptor objects
//!
//! `lib.exe` writes three COFF objects, for the DLL `<dll>` with `<stem>`
//! its name without its last extension (`foo.bar.baz` gives `foo.bar`):
//!
//! **1. The import descriptor** defines `__IMPORT_DESCRIPTOR_<stem>`.
//! Sections:
//!
//! | # | Name | Size | Characteristics | Contents |
//! |---|---|---|---|---|
//! | 1 | `.debug$S` | 54 and the DLL name's length | `0x42100040` (initialized data, 1-byte aligned, discardable, read) | the CodeView records below |
//! | 2 | `.idata$2` | 20 | `0xC0300040` (initialized data, 4-byte aligned, read, write) | zeros: the `IMAGE_IMPORT_DESCRIPTOR` |
//! | 3 | `.idata$6` | the name, a NUL, and another to an even size | `0xC0200040` (2-byte aligned) | the DLL's name |
//!
//! `.idata$2`'s three relocations, in this order: at 0x0C to symbol 3
//! (`.idata$6`: the name), at 0x00 to symbol 4 (`.idata$4`: the lookup
//! table), at 0x10 to symbol 5 (`.idata$5`: the address table); each
//! `IMAGE_REL_AMD64_ADDR32NB` (3) on x64, `IMAGE_REL_I386_DIR32NB` (7) on
//! x86, and `IMAGE_REL_ARM64_ADDR32NB` (2) on ARM64 and ARM64EC.
//!
//! Symbols (none with auxiliary records):
//!
//! | # | Name | Value | Section | Class |
//! |---|---|---|---|---|
//! | 0 | `@comp.id` | `0x010190A5` (the linker's id: product 0x0101, build 37029) | -1 | static |
//! | 1 | `__IMPORT_DESCRIPTOR_<stem>` | 0 | 2 | external |
//! | 2 | `.idata$2` | `0xC0000040` | 2 | section (104) |
//! | 3 | `.idata$6` | 0 | 3 | static |
//! | 4 | `.idata$4` | `0xC0000040` | 0 | section |
//! | 5 | `.idata$5` | `0xC0000040` | 0 | section |
//! | 6 | `__NULL_IMPORT_DESCRIPTOR` | 0 | 0 | external |
//! | 7 | `\x7f<stem>_NULL_THUNK_DATA` | 0 | 0 | external |
//!
//! **2. The null import descriptor** defines `__NULL_IMPORT_DESCRIPTOR`:
//! `.debug$S` as above, then `.idata$3`, 20 zero bytes, `0xC0300040`.
//! Symbols: `@comp.id` as above; `__NULL_IMPORT_DESCRIPTOR`, 0, section 2,
//! external.
//!
//! **3. The null thunk** defines `\x7f<stem>_NULL_THUNK_DATA`: `.debug$S`
//! as above, then `.idata$5` and `.idata$4`, each a zero pointer (8 bytes,
//! `0xC0400040`, 8-byte aligned, on x64, ARM64, and ARM64EC; 4 bytes,
//! `0xC0300040`, on x86). Symbols: `@comp.id`; the thunk's symbol, 0,
//! section 2 (`.idata$5`), external.
//!
//! **In every one**:
//!
//! - The file header: the machine (x64 `0x8664`, x86 `0x14C`, ARM64
//!   `0xAA64`; ARM64EC's are ARM64's), the section count, the time stamp
//!   (as the import objects'), the symbol table's offset, the symbol
//!   count, no optional header, and characteristics 0 (x86:
//!   `IMAGE_FILE_32BIT_MACHINE`, 0x100).
//! - The section headers have 0 for their virtual size and address and
//!   line numbers. The data follow the headers, each section's at once
//!   after the one before (no alignment), a section's relocations at once
//!   after its data; then the symbol table, then the string table (every
//!   name over 8 bytes, in symbol order). A section without relocations
//!   has 0 for their offset, but for the descriptor's `.idata$6`, which
//!   has `.idata$2`'s (0xE7 for a name of 17 bytes, 0xDC for 6), with a
//!   count of 0.
//! - `.debug$S` holds signature 2 (C13), then `S_OBJNAME_ST` (0x0009:
//!   signature 0, the DLL's name as a length-prefixed string), then
//!   `S_COMPILE2_ST` (0x1013: flags 7 (language "link"), the machine
//!   (`CV_CFL_AMD64` 0xD0, `CV_CFL_80386` 0x03, `CV_CFL_ARM64` 0xF6, and
//!   0xF8 for ARM64EC), front-end version 0.0.0, back-end version
//!   14.52.37029, and the length-prefixed string `Microsoft (R) LINK`),
//!   neither record padded.
//!
//! `llvm-lib` writes the same sections and relocations without the
//! `.debug$S` and `@comp.id`, with time stamp 0, section symbols' values
//! 0, and `.idata$6`'s relocation offset 0.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use turbonop_pe::consts::machine;
use turbonop_pe::import_object::{ImportNameType, ImportObject, ImportType};

use turbonop_archive::write::Tool;

use crate::ModuleDefinition;
use crate::descriptors::{Descriptors, Stamp, set_stamp};
use crate::exports::{Origin, Resolved, hints, resolve};
use crate::library::Members;

/// One import object of the library.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Import<'a> {
    /// Its symbol (`__imp_` and it are what it defines).
    pub symbol: Cow<'a, [u8]>,
    /// Code, data, or a constant.
    pub kind: ImportType,
    /// How the DLL's export is found.
    pub name_type: ImportNameType,
    /// The ordinal, or the hint.
    pub ordinal_or_hint: u16,
    /// The name after the DLL's (`EXPORTAS`, and ARM64EC's plain names).
    pub export_name: Option<Cow<'a, [u8]>>,
}

impl Import<'_> {
    /// The short import object, for `machine`, of `dll`, with `time_date_stamp`.
    pub fn object<'s>(
        &'s self,
        machine: u16,
        dll: &'s [u8],
        time_date_stamp: u32,
    ) -> ImportObject<'s> {
        let mut o = ImportObject::new(
            machine,
            self.kind,
            self.name_type,
            self.ordinal_or_hint,
            &self.symbol,
            dll,
            self.export_name.as_deref(),
        );
        o.time_date_stamp = time_date_stamp;
        o
    }
}

/// What `lib /DEF` puts in an import library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportLibrary<'a> {
    /// The machine.
    pub machine: u16,
    /// The DLL's file name.
    pub dll: Vec<u8>,
    /// The imports, in the `.def` file's order (the second linker
    /// member's, after the descriptor objects).
    pub imports: Vec<Import<'a>>,
    /// An ARM64X library's ARM64 imports (`/DEFARM64NATIVE`), in their
    /// `.def` file's order: after the ARM64EC ones (`imports`) in both
    /// orders.
    pub native: Vec<Import<'a>>,
}

impl<'a> ImportLibrary<'a> {
    /// The import library `lib /DEF:<def_stem>.def /MACHINE:<machine>`
    /// makes of `def` (the DLL is named for the `.def` file when `def`
    /// names none).
    pub fn lib_exe(def: &ModuleDefinition<'a>, machine: u16, def_stem: &[u8]) -> Self {
        Self {
            machine,
            dll: def.file_name(def_stem),
            imports: Self::lib_exe_imports(&def.exports(), machine),
            native: Vec::new(),
        }
    }

    /// The ARM64X import library `lib /DEF:<def_stem>.def
    /// /DEFARM64NATIVE:<native>.def /MACHINE:ARM64X` makes: `def`'s
    /// exports as ARM64EC imports, then `native`'s as ARM64 ones, for the
    /// DLL `def` names.
    pub fn lib_exe_arm64x(
        def: &ModuleDefinition<'a>,
        native: &ModuleDefinition<'a>,
        def_stem: &[u8],
    ) -> Self {
        Self {
            machine: machine::ARM64X,
            dll: def.file_name(def_stem),
            imports: Self::lib_exe_imports(&def.exports(), machine::ARM64EC),
            native: Self::lib_exe_imports(&native.exports(), machine::ARM64),
        }
    }

    /// `lib.exe`'s import of each of `exports` but the `PRIVATE` ones, for
    /// `machine`.
    pub fn lib_exe_imports(exports: &[crate::Export<'a>], machine: u16) -> Vec<Import<'a>> {
        let given: Vec<_> = exports.iter().map(|&e| (e, Origin::Definition)).collect();
        // Without objects, every export resolves.
        let resolved = resolve(&given, machine, None).unwrap_or_default();
        Self::imports_of(&resolved, machine)
    }

    /// The import of each of `resolved` but the `PRIVATE` ones, for
    /// `machine`.
    pub fn imports_of(resolved: &[Resolved<'a>], machine: u16) -> Vec<Import<'a>> {
        let hints = hints(resolved);
        let ec = machine == machine::ARM64EC;
        resolved
            .iter()
            .zip(hints)
            .filter(|(r, _)| !r.private)
            .map(|(r, hint)| Import {
                symbol: r.symbol.clone(),
                kind: r.import_type(),
                name_type: if r.ordinal.is_some() {
                    ImportNameType::Ordinal
                } else {
                    r.name_type
                },
                ordinal_or_hint: r.ordinal.unwrap_or(if ec { 0 } else { hint }),
                export_name: r.import_name.clone(),
            })
            .collect()
    }

    /// The DLL's name without its last extension: what the descriptor
    /// objects' symbols are named for.
    pub fn stem(&self) -> &[u8] {
        let dot = self.dll.iter().rposition(|&b| b == b'.');
        &self.dll[..dot.unwrap_or(self.dll.len())]
    }

    /// The descriptor objects' symbols: `__IMPORT_DESCRIPTOR_<stem>`,
    /// `__NULL_IMPORT_DESCRIPTOR`, and `\x7f<stem>_NULL_THUNK_DATA`.
    pub fn descriptor_symbols(&self) -> [Vec<u8>; 3] {
        let mut descriptor = b"__IMPORT_DESCRIPTOR_".to_vec();
        descriptor.extend_from_slice(self.stem());
        let mut thunk = b"\x7f".to_vec();
        thunk.extend_from_slice(self.stem());
        thunk.extend_from_slice(b"_NULL_THUNK_DATA");
        [descriptor, b"__NULL_IMPORT_DESCRIPTOR".to_vec(), thunk]
    }

    /// The imports' places in the archive: sorted by symbol (after the
    /// three descriptor objects).
    pub fn archive_order(&self) -> Vec<usize> {
        sorted(&self.imports)
    }

    /// The members `lib.exe` writes, each import object and descriptor
    /// with `stamp`: the descriptor objects, then the imports sorted by
    /// symbol (an ARM64X library's ARM64EC imports, then its ARM64 ones),
    /// listed in the index in the `.def` files' order.
    pub fn members(&self, stamp: Stamp) -> Members {
        let stem = self.stem();
        let descriptors = Descriptors {
            machine: self.machine,
            dll: &self.dll,
            stem,
            tool: Tool::LibExe,
        };
        let mut data: Vec<Vec<u8>> = descriptors.all(stamp).into();
        let mut listed: Vec<u32> = (0..3).collect();
        let groups = [
            (
                &self.imports,
                match self.machine {
                    machine::ARM64X => machine::ARM64EC,
                    m => m,
                },
            ),
            (&self.native, machine::ARM64),
        ];
        for (imports, m) in groups {
            let first = data.len();
            let order = sorted(imports);
            let mut place = alloc::vec![0u32; imports.len()];
            for (at, &k) in order.iter().enumerate() {
                place[k] = crate::descriptors::count(first + at);
                data.push(import_bytes(&imports[k], m, &self.dll, stamp));
            }
            listed.extend(place);
        }
        Members {
            dll: self.dll.clone(),
            data,
            listed,
            machine: self.machine,
        }
    }
}

/// The positions of `imports` sorted by symbol.
fn sorted(imports: &[Import<'_>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..imports.len()).collect();
    order.sort_by(|&a, &b| imports[a].symbol.cmp(&imports[b].symbol));
    order
}

/// An import object's bytes, with `stamp`.
pub fn import_bytes(import: &Import<'_>, machine: u16, dll: &[u8], stamp: Stamp) -> Vec<u8> {
    let mut bytes = import.object(machine, dll, 0).to_vec().unwrap_or_default();
    set_stamp(&mut bytes, 8, stamp);
    bytes
}

#[cfg(test)]
mod tests;
