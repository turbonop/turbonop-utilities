//! The `.exp` file: the export object link.exe writes beside an import
//! library (and links into the image): a COFF object of two or three
//! sections, as link.exe 14.52 writes it:
//!
//! - `.edata`: the export directory as an object holds it, its addresses
//!   relocations (ADDR32NB) to the tables in it and to each export's
//!   symbol: the directory, the address table (`rgpv`), the name pointers
//!   (`rgszName`), the ordinals (`rgwOrd`), the DLL's name (`szName`), and
//!   each name (`$N` and its ordinal);
//! - `.debug$S`: CodeView symbols (`S_OBJNAME`, `S_COMPILE3` for LINK,
//!   `S_ENVBLOCK` with the directory and the linker's path, an `S_EXPORT`
//!   per export) and, with a `.def` file, its SHA-256 and its path;
//! - `.drectve`, with a `.def` file that names the image (`LIBRARY` or
//!   `NAME`): ` /DLL /OUT:<its name>` (no `/DLL` for `NAME`; a UTF-8 byte
//!   order mark before it).
//!
//! The time stamps are 0xFFFFFFFF, as link.exe's. What can't be link.exe's
//! bytes: the linker's path in `S_ENVBLOCK` (turbolink's own).

use crate::config::Export;
use crate::exports::Directory;

/// link.exe 14.52.37029's export object's `@comp.id` (product 0x100).
const COMP_ID: u32 = crate::rich::EXPORT_COMP_ID;

/// What the `.exp` needs besides the directory.
pub struct Inputs<'a> {
    /// The `.exp` file's path (its name goes in `S_OBJNAME` as written).
    pub path: &'a std::path::Path,
    /// The DLL's name (the directory's).
    pub dll_name: &'a str,
    /// What the `.def` file says to link with (` /DLL /OUT:x.dll` for
    /// `LIBRARY x`, ` /OUT:x.exe` for `NAME x`): `.drectve`'s.
    pub directives: Option<&'a str>,
    /// The machine (its `IMAGE_FILE_MACHINE_` number) and CodeView's.
    pub machine: u16,
    /// The `.def` file and its contents, if the exports came from one.
    pub def: Option<(&'a std::path::Path, &'a [u8])>,
    /// The directory the link ran in, and the linker's path.
    pub cwd: &'a str,
    pub exe: &'a str,
    /// The exports given an ordinal (`@n`), by exported name.
    pub ordinals_given: &'a [String],
}

/// The `.exp` object for `dir`.
pub fn write(dir: &Directory, x: &Inputs<'_>) -> Vec<u8> {
    let exports = &dir.exports;
    // Exports by their exported name (the order of the relocations and the
    // S_EXPORT records).
    let mut by_name: Vec<usize> = (0..exports.len()).collect();
    by_name.sort_by(|&a, &b| {
        crate::exports::exported_name(&exports[a]).cmp(crate::exports::exported_name(&exports[b]))
    });
    let internal = |e: &Export| -> String {
        e.internal
            .clone()
            .unwrap_or_else(|| crate::exports::exported_name(e).to_owned())
    };

    // .edata: the directory, at 0, with no addresses yet.
    let mut edata = vec![0u8; dir.size as usize];
    dir.write(&mut edata, 0, x.dll_name, u32::MAX, |_| 0, &vec![None; exports.len()]);
    // The addresses the relocations fill are 0 in the object.
    let functions_at = 40u32;
    let names_at = functions_at + dir.functions * 4;
    let ordinals_at = names_at + crate::n32(dir.names.len()) * 4;
    for at in [12usize, 28, 32, 36] {
        edata[at..at + 4].fill(0);
    }
    let eat = |k: u32| functions_at as usize + 4 * k as usize;
    for slot in 0..dir.functions {
        edata[eat(slot)..eat(slot) + 4].fill(0);
    }
    for k in 0..dir.names.len() {
        let at = names_at as usize + 4 * k;
        edata[at..at + 4].fill(0);
    }

    // The symbols: @comp.id, @feat.00, the two sections', the tables', the
    // names', then each export's symbol (undefined), by name.
    let statics = dir.symbols(x.dll_name);
    let mut externals: Vec<String> = exports.iter().map(&internal).collect();
    externals.sort();
    externals.dedup();
    // Symbol indices: 0 @comp.id, 1 @feat.00, 2 .edata, 3 .debug$S, then
    // szName, rgpv, rgszName, rgwOrd, the names, then the externals.
    let local_names = ["szName", "rgpv", "rgszName", "rgwOrd"];
    let local_index = |n: &str| 4 + crate::n32(local_names.iter().position(|&x| x == n).unwrap_or(0));
    let name_symbol = |ordinal: u16| {
        let n = format!("$N{ordinal:05}");
        4 + 4 + crate::n32(statics.iter().skip(5).position(|(s, _)| *s == n).unwrap_or(0))
    };
    let first_external = 8 + crate::n32(statics.len() - 5);
    let external_index = |n: &str| first_external + crate::n32(externals.iter().position(|x| x == n).unwrap_or(0));

    // .edata's relocations: the directory's four addresses, then each export
    // (by name): its address, then its name's.
    let mut relocations: Vec<(u32, u32)> = vec![
        (12, local_index("szName")),
        (28, local_index("rgpv")),
        (32, local_index("rgszName")),
        (36, local_index("rgwOrd")),
    ];
    for &i in &by_name {
        let e = &exports[i];
        let slot = u32::from(e.ordinal.unwrap_or(dir.base) - dir.base);
        relocations.push((crate::n32(eat(slot)), external_index(&internal(e))));
        if let Some(k) = dir.names.iter().position(|&(_, j)| j == i) {
            relocations.push((names_at + 4 * crate::n32(k), name_symbol(e.ordinal.unwrap_or(0))));
        }
    }
    let _ = ordinals_at;

    // .debug$S.
    let debug = debug_s(dir, x, &by_name, &internal);
    // .drectve.
    let drectve = x.directives.map(|d| [&[0xEF, 0xBB, 0xBF][..], d.as_bytes()].concat());
    let sections: u16 = if drectve.is_some() { 3 } else { 2 };

    // The file: header, three section headers, .edata and its relocations,
    // .debug$S, .drectve, the symbols (on 4 bytes), the strings.
    let edata_at = 20 + 40 * usize::from(sections);
    let relocations_at = edata_at + edata.len();
    let debug_at = relocations_at + 10 * relocations.len();
    let drectve_at = debug_at + debug.len();
    // (A NUL after .drectve that its size doesn't count, as link.exe's.)
    let symbols_at = drectve.as_ref().map_or(drectve_at, |d| drectve_at + d.len() + 1);
    let mut strings: Vec<u8> = Vec::new();
    let mut name8 = |n: &str| -> [u8; 8] {
        let mut r = [0u8; 8];
        if n.len() <= 8 {
            r[..n.len()].copy_from_slice(n.as_bytes());
        } else {
            r[4..].copy_from_slice(&(4 + crate::n32(strings.len())).to_le_bytes());
            strings.extend_from_slice(n.as_bytes());
            strings.push(0);
        }
        r
    };
    let mut symbols: Vec<u8> = Vec::new();
    let symbol = |symbols: &mut Vec<u8>, n: [u8; 8], value: u32, section: i16, class: u8| {
        symbols.extend_from_slice(&n);
        symbols.extend_from_slice(&value.to_le_bytes());
        symbols.extend_from_slice(&section.to_le_bytes());
        symbols.extend_from_slice(&0u16.to_le_bytes());
        symbols.push(class);
        symbols.push(0);
    };
    symbol(&mut symbols, name8("@comp.id"), COMP_ID, -1, 3);
    symbol(&mut symbols, name8("@feat.00"), 0x810, -1, 3);
    symbol(&mut symbols, name8(".edata"), 0, 1, 3);
    symbol(&mut symbols, name8(".debug$S"), 0, 2, 3);
    for n in local_names {
        let offset = statics.iter().find(|(s, _)| s == n).map_or(0, |&(_, o)| o);
        symbol(&mut symbols, name8(n), offset, 1, 3);
    }
    for (n, offset) in statics.iter().skip(5) {
        symbol(&mut symbols, name8(n), *offset, 1, 3);
    }
    for n in &externals {
        symbol(&mut symbols, name8(n), 0, 0, 2);
    }
    let count = crate::n32(symbols.len() / 18);

    let mut out = Vec::new();
    out.extend_from_slice(&x.machine.to_le_bytes());
    out.extend_from_slice(&sections.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&crate::n32(symbols_at).to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    let header = |out: &mut Vec<u8>, name: &[u8; 8], size: usize, at: usize, rel: usize, nrel: usize, chars: u32| {
        out.extend_from_slice(name);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&crate::n32(size).to_le_bytes());
        out.extend_from_slice(&crate::n32(at).to_le_bytes());
        out.extend_from_slice(&crate::n32(rel).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&u16::try_from(nrel).unwrap_or(u16::MAX).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&chars.to_le_bytes());
    };
    header(&mut out, b".edata\0\0", edata.len(), edata_at, relocations_at, relocations.len(), 0x4000_0040);
    header(&mut out, b".debug$S", debug.len(), debug_at, 0, 0, 0x4210_0040);
    if let Some(d) = &drectve {
        header(&mut out, b".drectve", d.len(), drectve_at, 0, 0, 0x0000_0200);
    }
    out.extend_from_slice(&edata);
    let addr32nb: u16 = match x.machine {
        0x8664 => 3,
        0x014C => 7,
        0xAA64 => 2,
        _ => 3,
    };
    for (at, sym) in relocations {
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&sym.to_le_bytes());
        out.extend_from_slice(&addr32nb.to_le_bytes());
    }
    out.extend_from_slice(&debug);
    if let Some(d) = &drectve {
        out.extend_from_slice(d);
    }
    out.resize(symbols_at, 0);
    out.extend_from_slice(&symbols);
    out.extend_from_slice(&(4 + crate::n32(strings.len())).to_le_bytes());
    out.extend_from_slice(&strings);
    out
}

/// The `.debug$S` section: the symbols subsection, and with a `.def` file
/// its checksum and its name.
fn debug_s(dir: &Directory, x: &Inputs<'_>, by_name: &[usize], internal: &dyn Fn(&Export) -> String) -> Vec<u8> {
    let mut symbols = Vec::new();
    let mut record = |kind: u16, body: &[u8]| {
        symbols.extend_from_slice(&u16::try_from(body.len() + 2).unwrap_or(u16::MAX).to_le_bytes());
        symbols.extend_from_slice(&kind.to_le_bytes());
        symbols.extend_from_slice(body);
    };
    // S_OBJNAME: signature 0, the .exp's name.
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend_from_slice(x.path.display().to_string().as_bytes());
    b.push(0);
    record(0x1101, &b);
    // S_COMPILE3: LINK, the machine, version 0.0.0.0 / 14.52.37029.0.
    let mut b = vec![0x07, 0x00, 0x08, 0x00];
    let cv_machine: u16 = match x.machine {
        0x014C => 0x03,
        0xAA64 => 0xF6,
        _ => 0xD0,
    };
    b.extend_from_slice(&cv_machine.to_le_bytes());
    for v in [0u16, 0, 0, 0, 14, 52, 37029, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"Microsoft (R) LINK\0");
    record(0x113C, &b);
    // S_ENVBLOCK: the directory and the linker.
    let mut b = vec![0u8];
    for (k, v) in [("cwd", x.cwd), ("exe", x.exe)] {
        b.extend_from_slice(k.as_bytes());
        b.push(0);
        b.extend_from_slice(v.as_bytes());
        b.push(0);
    }
    b.push(0);
    record(0x113D, &b);
    // S_EXPORT per export, by exported name: ordinal, flags, the symbol.
    for &i in by_name {
        let e = &dir.exports[i];
        let mut flags = 0u16;
        if e.data {
            flags |= 2;
        }
        if e.private {
            flags |= 4;
        }
        if e.noname {
            flags |= 8;
        }
        if x.ordinals_given.iter().any(|n| n == crate::exports::exported_name(e)) {
            flags |= 0x10;
        }
        let mut b = e.ordinal.unwrap_or(0).to_le_bytes().to_vec();
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(internal(e).as_bytes());
        b.push(0);
        record(0x1138, &b);
    }
    let mut out = 4u32.to_le_bytes().to_vec();
    // (Each subsection on 4 bytes; nothing after the last.)
    let subsection = |out: &mut Vec<u8>, kind: u32, body: &[u8]| {
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&crate::n32(body.len()).to_le_bytes());
        out.extend_from_slice(body);
    };
    subsection(&mut out, 0xF1, &symbols);
    if let Some((path, contents)) = x.def {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(contents);
        let mut b = 1u32.to_le_bytes().to_vec(); // the name's offset in the strings
        b.push(32);
        b.push(3); // SHA-256
        b.extend_from_slice(&hash);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        subsection(&mut out, 0xF4, &b);
        let mut b = vec![0u8];
        b.extend_from_slice(path.display().to_string().as_bytes());
        b.push(0);
        subsection(&mut out, 0xF3, &b);
    }
    out
}
