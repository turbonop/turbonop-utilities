//! Bitcode where it's kept: a bitcode file itself (bare or wrapped),
//! objects' sections (ELF's `.llvmbc` and fat LTO objects' `.llvm.lto`,
//! Mach-O's `__LLVM,__bitcode`, COFF's `.llvmbc`, WebAssembly's `.llvmbc`
//! custom section), each with the command line beside it (`.llvmcmd`,
//! `__LLVM,__cmdline`), universal Mach-O files' slices, and archives'
//! members (LTO objects, or objects with bitcode in them).
//!
//! A linked image's section can hold several modules' bitcode back to
//! back: each is found.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use turbonop_archive::{Archive, MemberKind};
use turbonop_coff::{Kind, Object, SectionData};
use turbonop_elf::ElfFile;
use turbonop_macho::MachO;

use crate::bitstream::{Stream, WRAPPER_MAGIC, idx, is_bitcode};

/// What held the bitcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    /// The file (or member) is bitcode.
    File,
    /// An ELF object or image's section.
    Elf,
    /// A Mach-O file's section.
    MachO,
    /// A COFF object's section.
    Coff,
    /// A WebAssembly module's custom section.
    Wasm,
}

/// Bitcode found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Embedded<'d> {
    /// What held it.
    pub container: Container,
    /// The archive member it's in (its name), if any.
    pub member: Option<Cow<'d, [u8]>>,
    /// The universal Mach-O slice it's in (CPU type and subtype), if any.
    pub slice: Option<(u32, u32)>,
    /// The section it's in, if any.
    pub section: Option<Cow<'d, [u8]>>,
    /// Where it starts in the file.
    pub offset: u64,
    /// The bitcode (from its magic, or its wrapper).
    pub data: &'d [u8],
    /// The compiler's command line, from the section beside it (NUL
    /// separated), if any.
    pub command_line: Option<&'d [u8]>,
}

/// Sections that hold bitcode (ELF, COFF, WebAssembly).
const BITCODE_SECTIONS: &[&[u8]] = &[b".llvmbc", b".llvm.lto"];
/// The section that holds the command line.
const COMMAND_SECTION: &[u8] = b".llvmcmd";

/// COFF machines: objects of others aren't taken for COFF.
const COFF_MACHINES: &[u16] = &[
    0x014C, 0x8664, 0xAA64, 0x01C4, 0xA641, 0xA64E, 0x01C0, 0x0200, 0x5032, 0x5064, 0x5128,
];

fn le16(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The bitcode files in `data` (a section's bytes): each from its magic
/// (or wrapper) to its last top-level block, then the next one after
/// that (at a 4-byte boundary).
pub fn split(data: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let rest = &data[at..];
        if !is_bitcode(rest) {
            break;
        }
        let len = if le32(rest, 0) == Some(WRAPPER_MAGIC) {
            // A wrapper says where its bitcode ends.
            match (le32(rest, 8), le32(rest, 12)) {
                (Some(o), Some(s)) => (o as usize).saturating_add(s as usize).min(rest.len()),
                _ => rest.len(),
            }
        } else {
            match Stream::new(rest) {
                Ok(s) => s
                    .top_blocks()
                    .last()
                    .map_or(rest.len(), |b| idx(b.end.div_ceil(8))),
                Err(_) => rest.len(),
            }
        };
        let len = len.clamp(4, rest.len());
        out.push((at, &rest[..len]));
        at += len.div_ceil(4) * 4;
    }
    out
}

struct Finder<'d> {
    out: Vec<Embedded<'d>>,
}

impl<'d> Finder<'d> {
    #[allow(clippy::too_many_arguments)]
    fn section(
        &mut self,
        container: Container,
        member: Option<&Cow<'d, [u8]>>,
        slice: Option<(u32, u32)>,
        name: Cow<'d, [u8]>,
        offset: u64,
        data: &'d [u8],
        command_line: Option<&'d [u8]>,
    ) {
        for (at, bc) in split(data) {
            self.out.push(Embedded {
                container,
                member: member.cloned(),
                slice,
                section: Some(name.clone()),
                offset: offset + at as u64,
                data: bc,
                command_line,
            });
        }
    }

    fn file(
        &mut self,
        data: &'d [u8],
        base: u64,
        member: Option<Cow<'d, [u8]>>,
        slice: Option<(u32, u32)>,
        depth: u32,
    ) {
        if is_bitcode(data) {
            self.out.push(Embedded {
                container: Container::File,
                member,
                slice,
                section: None,
                offset: base,
                data,
                command_line: None,
            });
            return;
        }
        if ElfFile::is_elf(data) {
            let Ok(elf) = ElfFile::parse(data) else {
                return;
            };
            let cmd = elf
                .section_by_name(COMMAND_SECTION)
                .map(|s| elf.section_data(&s));
            for s in elf
                .sections()
                .filter(|s| BITCODE_SECTIONS.contains(&s.name))
            {
                let bytes = elf.section_data(&s);
                self.section(
                    Container::Elf,
                    member.as_ref(),
                    slice,
                    Cow::Borrowed(s.name),
                    base + s.offset,
                    bytes,
                    cmd,
                );
            }
            return;
        }
        if turbonop_macho::is_fat(data) {
            if depth > 0 {
                return;
            }
            for a in turbonop_macho::fat_archs(data).unwrap_or_default() {
                let (Ok(start), Ok(size)) = (usize::try_from(a.offset), usize::try_from(a.size))
                else {
                    continue;
                };
                let Some(bytes) = data.get(start..start.saturating_add(size)) else {
                    continue;
                };
                self.file(
                    bytes,
                    base + a.offset,
                    member.clone(),
                    Some((a.cputype, a.cpusubtype)),
                    depth + 1,
                );
            }
            return;
        }
        if let Ok(m) = MachO::parse(data) {
            let sections = m.sections();
            let get = |s: &turbonop_macho::Section<'_>| {
                let start = s.offset as usize;
                data.get(start..start.saturating_add(idx(s.file_size())))
            };
            let cmd = sections
                .iter()
                .find(|s| s.segname == b"__LLVM" && s.sectname == b"__cmdline")
                .and_then(get);
            for s in sections
                .iter()
                .filter(|s| s.segname == b"__LLVM" && s.sectname == b"__bitcode")
            {
                let Some(bytes) = get(s) else { continue };
                self.section(
                    Container::MachO,
                    member.as_ref(),
                    slice,
                    Cow::Borrowed(b"__LLVM,__bitcode"),
                    base + u64::from(s.offset),
                    bytes,
                    cmd,
                );
            }
            return;
        }
        if turbonop_wasm::is_wasm(data) {
            let Ok(m) = turbonop_wasm::Module::parse(data) else {
                return;
            };
            let body = |s: &turbonop_wasm::Section<'_>| {
                let start = idx(s.data_offset);
                let end = idx(s.contents_offset + s.contents_size);
                data.get(start..end)
            };
            let cmd = m
                .sections()
                .iter()
                .find(|s| s.id == turbonop_wasm::SECTION_CUSTOM && s.name == COMMAND_SECTION)
                .and_then(body);
            for s in m.sections().iter().filter(|s| {
                s.id == turbonop_wasm::SECTION_CUSTOM && BITCODE_SECTIONS.contains(&s.name)
            }) {
                let Some(bytes) = body(s) else { continue };
                self.section(
                    Container::Wasm,
                    member.as_ref(),
                    slice,
                    Cow::Borrowed(s.name),
                    base + s.data_offset,
                    bytes,
                    cmd,
                );
            }
            return;
        }
        if Archive::is_archive(data) {
            if depth > 0 {
                return;
            }
            let Ok(a) = Archive::parse(data) else { return };
            for m in a.members().filter(|m| m.kind == MemberKind::File) {
                self.file(
                    m.data,
                    base + m.data_offset,
                    Some(m.name.clone()),
                    slice,
                    depth + 1,
                );
            }
            return;
        }
        let coff = match turbonop_coff::kind(data) {
            Some(Kind::BigObj) => true,
            Some(Kind::Regular) => {
                le16(data, 0).is_some_and(|m| COFF_MACHINES.contains(&m))
                    && le16(data, 16) == Some(0)
            }
            _ => false,
        };
        if coff && let Ok(obj) = Object::parse(data) {
            // Section data borrowed from the file: where it is.
            let place = |d: &SectionData<'d>| -> Option<(u64, &'d [u8])> {
                let SectionData::Bytes(Cow::Borrowed(bytes)) = d else {
                    return None;
                };
                let at = (bytes.as_ptr() as usize).checked_sub(data.as_ptr() as usize)?;
                (data.get(at..at + bytes.len())?.as_ptr() == bytes.as_ptr())
                    .then_some((at as u64, *bytes))
            };
            let cmd = obj
                .sections
                .iter()
                .find(|s| s.name.as_bytes() == COMMAND_SECTION)
                .and_then(|s| place(&s.data))
                .map(|p| p.1);
            for s in obj
                .sections
                .iter()
                .filter(|s| BITCODE_SECTIONS.contains(&s.name.as_bytes()))
            {
                let Some((at, bytes)) = place(&s.data) else {
                    continue;
                };
                let name = Cow::Owned(s.name.as_bytes().to_vec());
                self.section(
                    Container::Coff,
                    member.as_ref(),
                    slice,
                    name,
                    base + at,
                    bytes,
                    cmd,
                );
            }
        }
    }
}

/// The bitcode in `data`: the file itself, its sections, its slices'
/// sections, or its members' (one level of archive or universal file).
pub fn find(data: &[u8]) -> Vec<Embedded<'_>> {
    let mut f = Finder { out: Vec::new() };
    f.file(data, 0, None, None, 0);
    f.out
}
