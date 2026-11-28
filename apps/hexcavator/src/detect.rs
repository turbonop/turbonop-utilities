//! Identifies a file's format from its leading bytes.
//!
//! This is a stopgap: it reads just enough of each header to name the format
//! and will move into the format crates as they are written.

use turbonop_core::{Endian, U16E, U16Le, U32E, read_at};
use turbonop_mz::NewHeaderKind;

const MSF7_MAGIC: &[u8] = b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0";

pub fn describe(data: &[u8]) -> String {
    if data.starts_with(b"MZ") {
        return describe_mz(data);
    }
    if data.starts_with(b"\x7fELF") {
        return describe_elf(data);
    }
    if data.starts_with(MSF7_MAGIC) {
        return "PDB (MSF 7.00)".to_string();
    }
    if data.starts_with(b"!<arch>\n") {
        return "ar archive".to_string();
    }
    if turbonop_java::is_class(data) {
        return describe_class(data);
    }
    if let Some(desc) = describe_macho(data) {
        return desc;
    }
    if let Some(desc) = describe_bitstream(data) {
        return desc;
    }
    if turbonop_wasm::is_wasm(data) {
        let version = data
            .get(4..8)
            .map_or(0, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        let object = turbonop_wasm::Module::parse(data).is_ok_and(|m| m.is_object());
        return format!(
            "WebAssembly {} (version {version})",
            if object { "object" } else { "module" }
        );
    }
    if turbonop_zip::is_zip(data) {
        return describe_zip(data);
    }
    // (A COFF object's header has no magic: last, so that nothing else
    // reads as one.)
    if let Some(desc) = describe_coff(data) {
        return desc;
    }
    "unknown format".to_string()
}

/// A Java class file: its version.
fn describe_class(data: &[u8]) -> String {
    let major = u16::from_be_bytes([data[6], data[7]]);
    let minor = u16::from_be_bytes([data[4], data[5]]);
    let java = turbonop_java::java_version_name(major).map_or_else(
        || {
            format!(
                "newer than Java {}",
                turbonop_java::LATEST_MAJOR_VERSION - 44
            )
        },
        |v| format!("Java {v}"),
    );
    let preview = if minor == turbonop_java::PREVIEW_MINOR_VERSION && major >= 56 {
        ", preview features"
    } else {
        ""
    };
    format!("Java class file, version {major}.{minor} ({java}{preview})")
}

/// A ZIP archive: a JAR if it has a manifest or classes.
fn describe_zip(data: &[u8]) -> String {
    match turbonop_zip::Archive::parse(data) {
        Ok(a) => {
            let jar = a.entries().iter().any(|e| {
                let n = e.name();
                n.eq_ignore_ascii_case(turbonop_java::jar::MANIFEST_NAME) || n.ends_with(".class")
            });
            let kind = if jar {
                "JAR (ZIP archive)"
            } else {
                "ZIP archive"
            };
            format!("{kind}, {} entries", a.len())
        }
        Err(e) => format!("ZIP archive (unreadable: {e})"),
    }
}

/// LLVM bitcode (bare or in Darwin's wrapper), or another LLVM
/// bitstream with a magic of its own, if `data` is.
fn describe_bitstream(data: &[u8]) -> Option<String> {
    use turbonop_bitcode::StreamKind;
    if !turbonop_bitcode::is_bitcode(data)
        && ![b"DIAG", b"RMRK", b"CPCH"]
            .iter()
            .any(|m| data.starts_with(*m))
    {
        return None;
    }
    let s = turbonop_bitcode::Stream::new(data).ok()?;
    Some(match s.kind {
        StreamKind::LlvmIr if s.wrapper.is_some() => "LLVM bitcode (wrapped)".to_string(),
        StreamKind::LlvmIr => "LLVM bitcode".to_string(),
        StreamKind::ClangDiagnostics => "clang serialized diagnostics (bitstream)".to_string(),
        StreamKind::Remarks => "LLVM optimization remarks (bitstream)".to_string(),
        StreamKind::ClangAst => "clang precompiled header or module (bitstream)".to_string(),
        StreamKind::Unknown => "LLVM bitstream".to_string(),
    })
}

/// A COFF object (or a /bigobj one, or an import object), if `data` is.
fn describe_coff(data: &[u8]) -> Option<String> {
    if let Ok(object) = turbonop_pe::PeFile::parse_object(data) {
        let kind = if object.is_bigobj() {
            "COFF object (bigobj)"
        } else {
            "COFF object"
        };
        return Some(format!("{kind}, {}", pe_machine(object.machine())));
    }
    if turbonop_pe::ltcg::LtcgObject::is_ltcg(data) {
        let machine = u16::from_le_bytes([data[6], data[7]]);
        return Some(format!(
            "Visual C++ LTCG object (/GL: the compiler's intermediate language), {}",
            pe_machine(machine)
        ));
    }
    // An import object: Sig1 0, Sig2 0xFFFF (as a /bigobj object's), then
    // its version (0) and machine.
    if data.len() >= 20 && data[0..6] == [0, 0, 0xFF, 0xFF, 0, 0] {
        let machine = u16::from_le_bytes([data[6], data[7]]);
        return Some(format!("COFF import object, {}", pe_machine(machine)));
    }
    None
}

fn describe_mz(data: &[u8]) -> String {
    let new_header = turbonop_mz::MzFile::parse(data)
        .ok()
        .and_then(|mz| mz.new_header());
    let e_lfanew = match new_header {
        Some(h) if h.kind == NewHeaderKind::Pe => h.offset,
        Some(h) => return format!("{} executable", h.kind.name()),
        None => return "MS-DOS executable".to_string(),
    };
    let pe = (|| {
        let machine = read_at::<U16Le>(data, e_lfanew + 4).ok()?.get();
        // The optional header follows the 4-byte signature and 20-byte file header.
        let kind = match read_at::<U16Le>(data, e_lfanew + 24).ok().map(|m| m.get()) {
            Some(0x10B) => "PE32",
            Some(0x20B) => "PE32+",
            _ => "PE (no optional header)",
        };
        Some(format!("{kind} image, {}", pe_machine(machine)))
    })();
    pe.unwrap_or_else(|| "PE (truncated)".to_string())
}

fn pe_machine(machine: u16) -> String {
    match turbonop_pe::consts::machine::name(machine) {
        Some(name) => name.to_string(),
        None => format!("machine {machine:#06x}"),
    }
}

fn describe_elf(data: &[u8]) -> String {
    let class = match data.get(4) {
        Some(1) => "ELF32",
        Some(2) => "ELF64",
        _ => return "ELF (bad class)".to_string(),
    };
    let endian = match data.get(5) {
        Some(1) => Endian::Little,
        Some(2) => Endian::Big,
        _ => return format!("{class} (bad byte order)"),
    };
    let order = match endian {
        Endian::Little => "little-endian",
        Endian::Big => "big-endian",
    };
    let Ok(machine) = read_at::<U16E>(data, 18) else {
        return format!("{class} {order} (truncated)");
    };
    let machine = match machine.get(endian) {
        3 => "x86".to_string(),
        40 => "ARM".to_string(),
        62 => "x86-64".to_string(),
        183 => "AArch64".to_string(),
        243 => "RISC-V".to_string(),
        other => format!("machine {other}"),
    };
    format!("{class} {order}, {machine}")
}

fn describe_macho(data: &[u8]) -> Option<String> {
    let raw = read_at::<U32E>(data, 0).ok()?;
    let (bits, endian) = match raw.get(Endian::Big) {
        0xFEED_FACE => ("32-bit", Endian::Big),
        0xFEED_FACF => ("64-bit", Endian::Big),
        0xCEFA_EDFE => ("32-bit", Endian::Little),
        0xCFFA_EDFE => ("64-bit", Endian::Little),
        // 0xCAFEBABE is shared with Java class files (read first: a major
        // version of 45 or more), whose architecture count is small.
        0xCAFE_BABE => {
            let count = read_at::<U32E>(data, 4).ok()?.get(Endian::Big);
            return (count < 45).then(|| format!("Mach-O universal binary, {count} architectures"));
        }
        _ => return None,
    };
    let cpu = read_at::<U32E>(data, 4).ok()?.get(endian);
    let cpu = match cpu {
        7 => "x86".to_string(),
        0x0100_0007 => "x86-64".to_string(),
        12 => "ARM".to_string(),
        0x0100_000C => "ARM64".to_string(),
        0x0200_000C => "ARM64_32".to_string(),
        other => format!("cpu type {other:#x}"),
    };
    Some(format!("Mach-O {bits}, {cpu}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_headers() {
        let mut pe = vec![0u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C] = 0x80;
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
        assert_eq!(describe(&pe), "PE32+ image, x64");
        assert_eq!(describe(&pe[..0x90]), "PE (no optional header) image, x64");
        assert_eq!(describe(&pe[..0x40]), "MS-DOS executable");

        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 2;
        elf[18..20].copy_from_slice(&183u16.to_be_bytes());
        assert_eq!(describe(&elf), "ELF64 big-endian, AArch64");

        let mut macho = vec![0u8; 32];
        macho[..4].copy_from_slice(&0xFEED_FACFu32.to_le_bytes());
        macho[4..8].copy_from_slice(&0x0100_000Cu32.to_le_bytes());
        assert_eq!(describe(&macho), "Mach-O 64-bit, ARM64");

        assert_eq!(describe(MSF7_MAGIC), "PDB (MSF 7.00)");
        assert_eq!(describe(b""), "unknown format");
    }
}
