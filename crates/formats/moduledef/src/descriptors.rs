//! The three import descriptor objects at the head of an import library
//! (laid out in [`crate::imports`]'s documentation), written with
//! `turbonop-coff` as `lib.exe` and `llvm-lib` write them, and the time
//! stamps `lib.exe /Brepro` gives the members it makes.
//!
//! **`/Brepro`'s time stamps** (measured on every member of every `/DEF`
//! fixture: 3 descriptor objects and 1 to 41 import objects each, for x64,
//! x86, ARM64, ARM64EC, and ARM64X): the member is hashed with SHA-256
//! with its time stamp set to `0xFFFFFFFF`, and the stamp is the digest's
//! first four bytes, little-endian, with the top two bits set
//! (`| 0xC000_0000`).

use alloc::vec;
use alloc::vec::Vec;

use sha2::{Digest, Sha256};
use turbonop_archive::write::Tool;
use turbonop_coff::{Format, Object, Relocation, Section, SectionData, SectionNumber, Symbol};
use turbonop_coff::{SectionId, SymbolId, storage_class};
use turbonop_pe::consts::machine;

/// A member's time stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stamp {
    /// This value (`lib.exe`: the time it ran; `llvm-lib`: 0).
    Time(u32),
    /// `lib.exe /Brepro`'s hash of the member ([`brepro_stamp`]).
    Brepro,
}

/// The time stamp `lib.exe /Brepro` gives `member`, whose own stamp is the
/// four bytes at `at` (4 in a COFF object, 8 in an import object): the
/// first four bytes of the SHA-256 of the member with those bytes
/// `0xFFFFFFFF`, little-endian, with the top two bits set.
pub fn brepro_stamp(member: &[u8], at: usize) -> u32 {
    let mut h = Sha256::new();
    h.update(&member[..at]);
    h.update([0xFF; 4]);
    h.update(&member[at + 4..]);
    let d = h.finalize();
    u32::from_le_bytes([d[0], d[1], d[2], d[3]]) | 0xC000_0000
}

/// Sets the time stamp at `at` in `member` to `stamp`.
pub fn set_stamp(member: &mut [u8], at: usize, stamp: Stamp) {
    let v = match stamp {
        Stamp::Time(t) => t,
        Stamp::Brepro => brepro_stamp(member, at),
    };
    member[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// `IMAGE_SCN_*` of the sections.
mod scn {
    /// `.debug$S`: initialized data, 1-byte aligned, discardable, read.
    pub const DEBUG: u32 = 0x4210_0040;
    /// Initialized data, read, write, aligned to 2, 4, or 8 bytes.
    pub const DATA_2: u32 = 0xC020_0040;
    pub const DATA_4: u32 = 0xC030_0040;
    pub const DATA_8: u32 = 0xC040_0040;
}

/// `lib.exe`'s `@comp.id`: product 0x0101 (the linker's import objects),
/// build 37029.
const COMP_ID: u32 = 0x0101_90A5;

/// The value `lib.exe` gives the section symbols of the descriptor.
const SECTION_SYMBOL_VALUE: u32 = 0xC000_0040;

/// What the descriptor objects depend on: the machine, the DLL, and the
/// tool that writes them.
#[derive(Clone, Copy, Debug)]
pub struct Descriptors<'a> {
    /// The library's machine: x64, x86, ARM64, ARM64EC, or ARM64X (whose
    /// descriptors are ARM64 objects).
    pub machine: u16,
    /// The DLL's file name.
    pub dll: &'a [u8],
    /// What the symbols are named for (the DLL's name without its last
    /// extension).
    pub stem: &'a [u8],
    /// Whose objects these are.
    pub tool: Tool,
}

impl Descriptors<'_> {
    /// The objects' machine: ARM64 for ARM64EC and ARM64X.
    fn object_machine(&self) -> u16 {
        match self.machine {
            machine::ARM64EC | machine::ARM64X => machine::ARM64,
            m => m,
        }
    }

    fn is_x86(&self) -> bool {
        self.machine == machine::I386
    }

    /// The `ADDR32NB` relocation type of the machine.
    fn addr32nb(&self) -> u16 {
        match self.object_machine() {
            machine::I386 => 7,
            machine::ARM64 => 2,
            // x64.
            _ => 3,
        }
    }

    /// The symbols: `__IMPORT_DESCRIPTOR_<stem>`, `__NULL_IMPORT_DESCRIPTOR`,
    /// and `\x7f<stem>_NULL_THUNK_DATA`.
    pub fn symbols(&self) -> [Vec<u8>; 3] {
        let mut descriptor = b"__IMPORT_DESCRIPTOR_".to_vec();
        descriptor.extend_from_slice(self.stem);
        let mut thunk = b"\x7f".to_vec();
        thunk.extend_from_slice(self.stem);
        thunk.extend_from_slice(b"_NULL_THUNK_DATA");
        [descriptor, b"__NULL_IMPORT_DESCRIPTOR".to_vec(), thunk]
    }

    /// `lib.exe`'s `.debug$S`: signature 2, `S_OBJNAME_ST` (the DLL's name),
    /// and `S_COMPILE2_ST` (the linker, 14.52.37029).
    fn debug_s(&self) -> Vec<u8> {
        let mut d = 2u32.to_le_bytes().to_vec();
        // S_OBJNAME_ST: signature 0, then the name, length-prefixed.
        let name_len = u8::try_from(self.dll.len()).unwrap_or(u8::MAX);
        let name = &self.dll[..usize::from(name_len)];
        let len = u16::from(name_len) + 7;
        d.extend_from_slice(&len.to_le_bytes());
        d.extend_from_slice(&0x0009u16.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.push(name_len);
        d.extend_from_slice(name);
        // S_COMPILE2_ST.
        let cv_machine: u16 = match self.machine {
            machine::I386 => 0x03,
            machine::ARM64 => 0xF6,
            machine::ARM64EC | machine::ARM64X => 0xF8,
            _ => 0xD0,
        };
        const LINKER: &[u8; 18] = b"Microsoft (R) LINK";
        let len: u16 = 2 + 4 + 2 + 12 + 1 + 18;
        d.extend_from_slice(&len.to_le_bytes());
        d.extend_from_slice(&0x1013u16.to_le_bytes());
        d.extend_from_slice(&7u32.to_le_bytes());
        d.extend_from_slice(&cv_machine.to_le_bytes());
        for v in [0u16, 0, 0, 14, 52, 37029] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.push(18);
        d.extend_from_slice(LINKER);
        d
    }

    fn object(&self) -> Object<'static> {
        let mut o = Object::new(Format::Regular, self.object_machine());
        if self.is_x86() {
            o.characteristics = turbonop_pe::consts::file_characteristics::MACHINE_32BIT;
        }
        o
    }

    fn lib_exe(&self) -> bool {
        self.tool == Tool::LibExe
    }

    /// The prologue `lib.exe` gives every descriptor object: `.debug$S` (as
    /// section 1) and `@comp.id`.
    fn add_lib_exe_head(&self, o: &mut Object<'_>) {
        if self.lib_exe() {
            o.sections.push(Section::new(
                &b".debug$S"[..],
                scn::DEBUG,
                SectionData::Bytes(self.debug_s().into()),
            ));
            o.symbols.push(Symbol::new(
                &b"@comp.id"[..],
                COMP_ID,
                SectionNumber::Absolute,
                0,
                storage_class::STATIC,
            ));
        }
    }

    /// The import descriptor: `.idata$2` (the `IMAGE_IMPORT_DESCRIPTOR`,
    /// relocated to the name, the lookup table, and the address table) and
    /// `.idata$6` (the name), defining `__IMPORT_DESCRIPTOR_<stem>` and
    /// referring to the other two objects' symbols.
    pub fn import_descriptor(&self, stamp: Stamp) -> Vec<u8> {
        let [descriptor, null, thunk] = self.symbols();
        let mut o = self.object();
        self.add_lib_exe_head(&mut o);
        let lib = self.lib_exe();
        // Section numbers and symbol indices after the prologue.
        let first_section = count(o.sections.len());
        let first_symbol = count(o.symbols.len());
        let section = |i: u32| SectionId::from_number(first_section + i).unwrap_or(SectionId(0));
        let symbol = |i: u32| SymbolId(first_symbol + i);
        let mut idata2 = Section::new(
            &b".idata$2"[..],
            scn::DATA_4,
            SectionData::Bytes(vec![0u8; 20].into()),
        );
        let kind = self.addr32nb();
        for (offset, target) in [(0x0C, 2), (0x00, 3), (0x10, 4)] {
            idata2.relocations.push(Relocation {
                offset,
                symbol: symbol(target),
                kind,
            });
        }
        o.sections.push(idata2);
        // The name: lib.exe pads it to an even size, llvm-lib doesn't.
        let mut name = self.dll.to_vec();
        name.push(0);
        if lib && name.len() % 2 == 1 {
            name.push(0);
        }
        o.sections.push(Section::new(
            &b".idata$6"[..],
            scn::DATA_2,
            SectionData::Bytes(name.into()),
        ));
        let section_value = if lib { SECTION_SYMBOL_VALUE } else { 0 };
        let defined = |id| SectionNumber::Section(section(id));
        o.symbols.extend([
            Symbol::new(descriptor, 0, defined(1), 0, storage_class::EXTERNAL),
            Symbol::new(
                &b".idata$2"[..],
                section_value,
                defined(1),
                0,
                storage_class::SECTION,
            ),
            Symbol::new(&b".idata$6"[..], 0, defined(2), 0, storage_class::STATIC),
            Symbol::new(
                &b".idata$4"[..],
                section_value,
                SectionNumber::Undefined,
                0,
                storage_class::SECTION,
            ),
            Symbol::new(
                &b".idata$5"[..],
                section_value,
                SectionNumber::Undefined,
                0,
                storage_class::SECTION,
            ),
            Symbol::new(
                null,
                0,
                SectionNumber::Undefined,
                0,
                storage_class::EXTERNAL,
            ),
            Symbol::new(
                thunk,
                0,
                SectionNumber::Undefined,
                0,
                storage_class::EXTERNAL,
            ),
        ]);
        let mut bytes = write(&o);
        if lib {
            // lib.exe gives .idata$6 (no relocations) .idata$2's pointer
            // to its relocations; the writer's fresh layout gives it 0.
            let header = |n: usize| 20 + 40 * n;
            let at = header(first_section as usize) + 24;
            let pointer: [u8; 4] = bytes[at..at + 4].try_into().unwrap_or_default();
            let to = header(first_section as usize + 1) + 24;
            bytes[to..to + 4].copy_from_slice(&pointer);
        }
        self.stamped(bytes, stamp)
    }

    /// The null import descriptor: `.idata$3`, 20 zero bytes, defining
    /// `__NULL_IMPORT_DESCRIPTOR`.
    pub fn null_import_descriptor(&self, stamp: Stamp) -> Vec<u8> {
        let [_, null, _] = self.symbols();
        let mut o = self.object();
        self.add_lib_exe_head(&mut o);
        o.sections.push(Section::new(
            &b".idata$3"[..],
            scn::DATA_4,
            SectionData::Bytes(vec![0u8; 20].into()),
        ));
        let id = SectionId::from_number(count(o.sections.len())).unwrap_or(SectionId(0));
        o.symbols.push(Symbol::new(
            null,
            0,
            SectionNumber::Section(id),
            0,
            storage_class::EXTERNAL,
        ));
        self.stamped(write(&o), stamp)
    }

    /// The null thunk: `.idata$5` and `.idata$4`, each a zero pointer,
    /// defining `\x7f<stem>_NULL_THUNK_DATA` (in `.idata$5`).
    pub fn null_thunk(&self, stamp: Stamp) -> Vec<u8> {
        let [_, _, thunk] = self.symbols();
        let mut o = self.object();
        self.add_lib_exe_head(&mut o);
        let (size, align) = if self.is_x86() {
            (4, scn::DATA_4)
        } else {
            (8, scn::DATA_8)
        };
        for name in [&b".idata$5"[..], &b".idata$4"[..]] {
            o.sections.push(Section::new(
                name,
                align,
                SectionData::Bytes(vec![0u8; size].into()),
            ));
        }
        let id = SectionId::from_number(count(o.sections.len()) - 1).unwrap_or(SectionId(0));
        o.symbols.push(Symbol::new(
            thunk,
            0,
            SectionNumber::Section(id),
            0,
            storage_class::EXTERNAL,
        ));
        self.stamped(write(&o), stamp)
    }

    /// The three, in the library's order.
    pub fn all(&self, stamp: Stamp) -> [Vec<u8>; 3] {
        [
            self.import_descriptor(stamp),
            self.null_import_descriptor(stamp),
            self.null_thunk(stamp),
        ]
    }

    fn stamped(&self, mut bytes: Vec<u8>, stamp: Stamp) -> Vec<u8> {
        set_stamp(&mut bytes, 4, stamp);
        bytes
    }
}

/// A count of sections or symbols (a few) as a number.
pub(crate) fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The object, written. These objects are small and well formed, so
/// writing them can't fail.
fn write(o: &Object<'_>) -> Vec<u8> {
    o.write().unwrap_or_default()
}

#[cfg(test)]
mod tests;
