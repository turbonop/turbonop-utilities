//! `/symbols`: a COFF symbol table (an object's, or an image's: MinGW's
//! linker keeps one), as dumpbin shows it. Each symbol in a line (its
//! index, value, section, type, storage class, and name, undecorated if
//! it's a C++ name), and its auxiliary records under it: a section's
//! definition, a weak external's default, a function's size, a file's
//! name.

use std::io::{self, Write};

use hexcavator_pe::consts::section_characteristics as scn;
use hexcavator_pe::{CoffSymbol, PeFile, SymbolTable};

/// `IMAGE_SYM_CLASS_*`.
mod class {
    pub const END_OF_FUNCTION: u8 = 0xFF;
    pub const EXTERNAL: u8 = 2;
    pub const STATIC: u8 = 3;
    pub const FAR_EXTERNAL: u8 = 68;
    pub const BLOCK: u8 = 100;
    pub const FUNCTION: u8 = 101;
    pub const FILE: u8 = 103;
    pub const WEAK_EXTERNAL: u8 = 105;
    pub const CLR_TOKEN: u8 = 107;
}

/// The symbol table, if the file has one (nothing, if it hasn't).
pub fn dump(out: &mut dyn Write, file: &PeFile<'_>) -> io::Result<()> {
    let table = match file.symbols() {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(()),
        Err(e) => {
            writeln!(out, "\nCOFF SYMBOL TABLE\n  (error: {e})")?;
            return Ok(());
        }
    };
    writeln!(out, "\nCOFF SYMBOL TABLE")?;
    for symbol in table.iter() {
        line(out, file, &table, &symbol)?;
    }
    writeln!(
        out,
        "\nString Table Size = 0x{:X} bytes",
        table.strings().size()
    )
}

fn line(
    out: &mut dyn Write,
    file: &PeFile<'_>,
    table: &SymbolTable<'_>,
    s: &CoffSymbol<'_>,
) -> io::Result<()> {
    let section = match s.section {
        0 => "UNDEF".to_string(),
        -1 => "ABS".to_string(),
        -2 => "DEBUG".to_string(),
        n => format!("SECT{n:X}"),
    };
    let name = table.name(s).unwrap_or_default();
    let name = String::from_utf8_lossy(name);
    let shown = match super::undecorate(&name) {
        Some(u) => format!("{name} ({u})"),
        None => name.to_string(),
    };
    writeln!(
        out,
        "{:03X} {:08X} {section:<6} {:<12} {:<12} | {shown}",
        s.index,
        s.value,
        type_name(s.kind),
        class_name(s.storage_class, &name),
    )?;
    aux(out, file, s, &name)
}

/// A symbol's type: its base type, and what's derived from it (`()`, a
/// function; `*`, a pointer; `[]`, an array).
fn type_name(kind: u16) -> String {
    const BASE: [&str; 16] = [
        "notype", "void", "char", "short", "int", "long", "float", "double", "struct", "union",
        "enum", "moe", "byte", "word", "uint", "dword",
    ];
    let mut out = BASE[usize::from(kind & 0xF)].to_string();
    match (kind >> 4) & 0x3 {
        1 => out.push_str(" *"),
        2 => out.push_str(" ()"),
        3 => out.push_str(" []"),
        _ => {}
    }
    out
}

fn class_name(c: u8, name: &str) -> &'static str {
    match c {
        class::END_OF_FUNCTION => "EndOfFunction",
        0 => "NoClass",
        1 => "AutoVar",
        class::EXTERNAL => "External",
        class::STATIC => "Static",
        4 => "RegisterVar",
        5 => "ExternalDef",
        6 => "Label",
        7 => "UndefinedLabel",
        8 => "MemberOfStruct",
        9 => "FunctionArg",
        10 => "StructTag",
        11 => "MemberOfUnion",
        12 => "UnionTag",
        13 => "TypeDefinition",
        14 => "UndefinedStatic",
        15 => "EnumTag",
        16 => "MemberOfEnum",
        17 => "RegisterParam",
        18 => "BitField",
        class::FAR_EXTERNAL => "Far External",
        class::BLOCK => match name {
            ".bb" => "BeginBlock",
            ".eb" => "EndBlock",
            _ => ".bb or.eb",
        },
        class::FUNCTION => match name {
            ".bf" => "BeginFunction",
            ".ef" => "EndFunction",
            _ => ".bf or.ef",
        },
        102 => "EndOfStruct",
        class::FILE => "Filename",
        104 => "Section",
        class::WEAK_EXTERNAL => "WeakExternal",
        class::CLR_TOKEN => "clr Token",
        _ => "UNKNOWN SYMBOL CLASS",
    }
}

fn u16_at(a: &[u8], o: usize) -> u16 {
    a.get(o..o + 2)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(a: &[u8], o: usize) -> u32 {
    a.get(o..o + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The symbol's auxiliary records, as their kind is.
fn aux(out: &mut dyn Write, file: &PeFile<'_>, s: &CoffSymbol<'_>, name: &str) -> io::Result<()> {
    if s.aux_count() == 0 {
        return Ok(());
    }
    // A file's name, across its records (to its NUL).
    if s.storage_class == class::FILE {
        let bytes: Vec<u8> = s.aux().flatten().copied().collect();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        return writeln!(out, "    {}", String::from_utf8_lossy(&bytes[..end]));
    }
    if let Some(d) = s.section_definition() {
        write!(
            out,
            "    Section length {:4X}, #relocs {:4X}, #linenums {:4X}, checksum {:8X}",
            d.length, d.relocations, d.line_numbers, d.checksum
        )?;
        let comdat = usize::try_from(s.section - 1)
            .ok()
            .and_then(|i| file.sections().get(i))
            .is_some_and(|h| h.characteristics.get() & scn::LNK_COMDAT != 0);
        if comdat {
            write!(out, ", selection {:4X}", d.selection)?;
            match d.selection {
                5 => write!(out, " (pick associative Section 0x{:X})", d.number)?,
                n => {
                    let how = match n {
                        1 => "no duplicates",
                        2 => "any",
                        3 => "same size",
                        4 => "exact match",
                        6 => "largest",
                        7 => "newest",
                        _ => "unknown",
                    };
                    write!(out, " (pick {how})")?;
                }
            }
        }
        return writeln!(out);
    }
    for a in s.aux() {
        match s.storage_class {
            class::WEAK_EXTERNAL => {
                let how = match u32_at(a, 4) {
                    1 => "No library search",
                    2 => "Library search",
                    3 => "Alias record",
                    4 => "Anti dependency",
                    _ => "Unknown",
                };
                writeln!(out, "    Default index {:8X} {how}", u32_at(a, 0))?;
            }
            class::FUNCTION => {
                // (.bf's: its line, and the next function's .bf; .ef's: its
                // line.)
                write!(out, "    line# {:04x}", u16_at(a, 4))?;
                if name == ".bf" {
                    write!(out, " end {:08X}", u32_at(a, 12))?;
                }
                writeln!(out)?;
            }
            class::CLR_TOKEN => writeln!(out, "    Symbol index {:8X}", u32_at(a, 4))?,
            // A function's definition: its .bf's index, and its size.
            class::EXTERNAL | class::STATIC if (s.kind >> 4) & 0x3 == 2 => {
                writeln!(
                    out,
                    "    tag index {:08X} size {:08X}",
                    u32_at(a, 0),
                    u32_at(a, 4)
                )?;
            }
            _ => {
                let hex: Vec<String> = a.iter().map(|b| format!("{b:02X}")).collect();
                writeln!(out, "    {}", hex.join(" "))?;
            }
        }
    }
    Ok(())
}
