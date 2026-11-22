//! The size model of an image of any format: its loaded sections, and the
//! compilands that contributed to them, as its debug information says.
//!
//! PE images have their own model ([`crate::pe`]): COFF groups, the
//! tables the headers locate, `.pdata` charged to the code it describes,
//! all as SizeBench has them. Other formats' models have no groups: their
//! pieces are the debug information's contributions (DWARF's compile
//! units' ranges), in the sections that hold them.

use std::collections::HashMap;

use hexcavator_core::Result;
use hexcavator_debuginfo::DebugInfo;
use hexcavator_image::{Image, Kind, SectionFlags};

use crate::pe::Unsupported;
use crate::range::Range;
use crate::{Compiland, Library, Model, NO_GROUP, Piece, Section, Size};

/// The model of `image` with `debug`: a PE image's with its PDB as
/// SizeBench has it ([`crate::pe`]); every other's (a PE image's with
/// DWARF, as MinGW builds one, and a COFF object's, its CodeView read as
/// a PDB, too) from its sections and its debug information's
/// contributions.
pub fn model(image: &Image<'_>, debug: &DebugInfo<'_>) -> Result<Model> {
    if image.pe().is_some() && image.kind() != Kind::Object && debug.pdb().is_some() {
        return crate::pe::model(image, debug);
    }
    build(image, debug)
}

/// Checks that `image` and `debug` can be analyzed together.
pub fn check(image: &Image<'_>, debug: &DebugInfo<'_>) -> std::result::Result<(), Unsupported> {
    if let Some(pe) = image.pe() {
        if debug.pdb().is_some() && image.kind() != Kind::Object {
            return crate::pe::check(image, debug);
        }
        if pe
            .data_directory(hexcavator_pe::consts::directory::COM_DESCRIPTOR)
            .is_some()
        {
            return Err(Unsupported::Managed);
        }
    }
    let ids = image.debug_ids();
    // (An image with its own DWARF names no other file; nor need it.)
    if !ids.is_empty() && !ids.iter().any(|id| debug.matches(id)) {
        return Err(Unsupported::DebugMismatch);
    }
    Ok(())
}

/// The sections' names: a Mach-O section's with its segment's
/// (`__TEXT,__text`, as bloaty and Apple's tools name them), others' as
/// they are; duplicates numbered.
fn section_names(image: &Image<'_>) -> Vec<String> {
    let mut names: Vec<String> = match image.macho() {
        Some(m) => m
            .sections()
            .iter()
            .map(|s| {
                format!(
                    "{},{}",
                    String::from_utf8_lossy(s.segname),
                    String::from_utf8_lossy(s.sectname)
                )
            })
            .collect(),
        None => image
            .sections()
            .iter()
            .map(|s| String::from_utf8_lossy(s.name).into_owned())
            .collect(),
    };
    let mut seen: HashMap<String, u32> = HashMap::new();
    for n in &mut names {
        let count = seen.entry(n.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            *n = format!("{n} ({count})");
        }
    }
    names
}

/// A WebAssembly module's `.bss`: from the first variable in its memory
/// (at [`hexcavator_image::WASM_MEMORY`]) that is in no section to past
/// the last, if there are any, and none of `sections` overlaps them.
fn wasm_bss(sections: &[Section], debug: &DebugInfo<'_>) -> Result<Option<Section>> {
    use hexcavator_debuginfo::SymbolKind as DebugKind;
    let within = |a: u64| {
        sections
            .iter()
            .any(|s| a >= s.addr && a - s.addr < s.size.vm)
    };
    let mut range: Option<(u64, u64)> = None;
    for s in debug.symbols()? {
        if s.kind != DebugKind::Data
            || s.thread_local
            || s.addr < hexcavator_image::WASM_MEMORY
            || within(s.addr)
        {
            continue;
        }
        let end = s.addr.saturating_add(s.size.unwrap_or(0).max(1));
        range = Some(range.map_or((s.addr, end), |(a, b)| (a.min(s.addr), b.max(end))));
    }
    let Some((start, end)) = range else {
        return Ok(None);
    };
    if sections
        .iter()
        .any(|s| s.addr < end && start < s.addr.saturating_add(s.size.vm))
    {
        return Ok(None);
    }
    Ok(Some(Section {
        name: ".bss".to_string(),
        addr: start,
        size: Size::new(0, end - start),
        vm_padding: 0,
        characteristics: 11,
        flags: SectionFlags(
            SectionFlags::READ | SectionFlags::WRITE | SectionFlags::DATA | SectionFlags::ZERO_FILL,
        ),
    }))
}

/// Builds the model of a non-PE image from its sections and `debug`.
pub fn build(image: &Image<'_>, debug: &DebugInfo<'_>) -> Result<Model> {
    let names = section_names(image);
    let linked = image.kind() != Kind::Object;
    // (An object's sections at 0 aren't laid out, but a WebAssembly
    // object's code is at its offsets, from 0, as a module's is.)
    let wasm = image.wasm().is_some();
    let mut sections: Vec<Section> = image
        .sections()
        .iter()
        .zip(names)
        .filter(|(s, _)| {
            s.vm_size > 0
                && !s.flags.contains(SectionFlags::NOT_LOADED)
                && (linked || s.addr != 0 || (wasm && s.flags.contains(SectionFlags::CODE)))
        })
        .map(|(s, name)| Section {
            name,
            addr: s.addr,
            size: Size::new(s.file_size.min(s.vm_size), s.vm_size),
            vm_padding: 0,
            characteristics: s.raw_flags,
            flags: s.flags,
        })
        .collect();
    // A WebAssembly module's zeroed data (`.bss`), without shared memory,
    // is in no segment (memory starts zeroed), and only its debug
    // information's variables say where.
    if wasm
        && linked
        && !sections.iter().any(|s| s.name.starts_with(".bss"))
        && let Some(bss) = wasm_bss(&sections, debug)?
    {
        sections.push(bss);
    }
    sections.sort_by_key(|s| s.addr);

    // One library (formats without archives' names in their debug
    // information), and the compilands as the debug information lists them.
    let libraries = vec![Library {
        name: String::new(),
    }];
    let compilands: Vec<Compiland> = debug
        .compilands()?
        .into_iter()
        .map(|c| Compiland {
            name: c.name,
            library: 0,
        })
        .collect();

    let mut pieces = Vec::new();
    for c in debug.contributions()? {
        if c.size == 0 || c.compiland as usize >= compilands.len() {
            continue;
        }
        // The section holding it.
        let i = sections.partition_point(|s| s.addr <= c.addr);
        let Some(si) = i.checked_sub(1) else { continue };
        let s = &sections[si];
        if c.addr - s.addr >= s.size.vm {
            continue;
        }
        let zero_fill = s.flags.contains(SectionFlags::ZERO_FILL);
        pieces.push(Piece {
            range: Range::new(c.addr, c.size.min(s.addr + s.size.vm - c.addr), zero_fill),
            compiland: c.compiland,
            section: u32::try_from(si).unwrap_or(u32::MAX),
            group: NO_GROUP,
        });
    }
    pieces.sort_by_key(|p| (p.range.addr, p.range.len, p.compiland));
    Ok(Model {
        arch: Some(image.arch()),
        sections,
        groups: Vec::new(),
        libraries,
        compilands,
        pieces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../../testdata/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn elf_sections_and_compile_units() {
        let data = testdata("elf/dwarf5-x86_64-linux-gnu");
        let image = Image::parse(&data).unwrap();
        let debug = DebugInfo::parse(&data).unwrap();
        check(&image, &debug).unwrap();
        let m = model(&image, &debug).unwrap();
        let names: Vec<&str> = m.sections.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&".text") && names.contains(&".bss"),
            "{names:?}"
        );
        assert!(!names.iter().any(|n| n.starts_with(".debug")));
        assert!(m.sections.windows(2).all(|w| w[0].addr <= w[1].addr));
        assert!(
            m.sections
                .iter()
                .find(|s| s.name == ".text")
                .unwrap()
                .executable()
        );
        let bss = m.sections.iter().find(|s| s.name == ".bss").unwrap();
        assert_eq!(bss.size.file, 0);
        assert!(bss.writable());
        let cus: Vec<&str> = m.compilands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cus, ["program.c", "second.c"]);
        // Every compile unit's code is in .text, charged to it; its data
        // in the sections that hold it.
        assert!(!m.pieces.is_empty());
        assert!(m.pieces.iter().all(|p| p.group == NO_GROUP));
        let text = names.iter().position(|n| *n == ".text").unwrap() as u32;
        let code: u64 = m
            .pieces
            .iter()
            .filter(|p| p.section == text)
            .map(|p| p.range.len)
            .sum();
        assert_eq!(code, 0x70 + 3 + 0x32 + 0x30 + 0x21);
        let bss_index = names.iter().position(|n| *n == ".bss").unwrap() as u32;
        assert!(m.pieces.iter().any(|p| p.section == bss_index));
    }

    #[test]
    fn macho_sections_are_named_by_segment() {
        let data = testdata("macho/program-arm64");
        let image = Image::parse(&data).unwrap();
        let dsym = testdata("macho/program-arm64.dwarf");
        let debug = DebugInfo::parse(&dsym).unwrap();
        check(&image, &debug).unwrap();
        let m = model(&image, &debug).unwrap();
        let names: Vec<&str> = m.sections.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"__TEXT,__text"), "{names:?}");
        assert!(names.contains(&"__DATA,__data"), "{names:?}");
        assert!(!m.pieces.is_empty());
        // Another file's debug information doesn't go with it.
        let other = testdata("macho/program-x86_64.dwarf");
        let other = DebugInfo::parse(&other).unwrap();
        assert_eq!(check(&image, &other), Err(Unsupported::DebugMismatch));
    }
}
