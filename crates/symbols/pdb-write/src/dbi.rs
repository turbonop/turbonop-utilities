//! The DBI stream: its header, the modules, the section contributions,
//! the section map, the source files of each module, the EC names, and
//! the optional debug streams' numbers.

use crate::streams::StringTable;

/// A section contribution (`SectionContrib`, 28 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contrib {
    /// The section (1-based).
    pub section: u16,
    /// The offset in it.
    pub offset: u32,
    /// The size.
    pub size: u32,
    /// The input section's characteristics.
    pub characteristics: u32,
    /// The module (0-based; `0xFFFF` for none).
    pub module: u16,
    /// The contents' CRC (0 if not computed).
    pub data_crc: u32,
    /// The relocations' CRC.
    pub reloc_crc: u32,
}

impl Contrib {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.section.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.characteristics.to_le_bytes());
        out.extend_from_slice(&self.module.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.data_crc.to_le_bytes());
        out.extend_from_slice(&self.reloc_crc.to_le_bytes());
    }
}

/// One module (an object, or `* Linker *`).
#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    /// The module's name (the object's path, or a library member's).
    pub name: Vec<u8>,
    /// The object file's name (the library, for a member).
    pub obj: Vec<u8>,
    /// Its stream, if it has symbols or lines.
    pub stream: Option<u16>,
    /// Its symbols' size, the 4-byte signature included.
    pub sym_bytes: u32,
    /// Its C13 subsections' size.
    pub c13_bytes: u32,
    /// Its source files.
    pub files: Vec<Vec<u8>>,
    /// Its first section contribution.
    pub first: Contrib,
    /// The module's PDB path's EC name index (0: none).
    pub pdb_path_ni: u32,
}

/// The optional debug header's stream slots.
pub mod debug_stream {
    /// FPO data.
    pub const FPO: usize = 0;
    /// The section headers.
    pub const SECTION_HEADERS: usize = 5;
    /// New FPO data (`FRAMEDATA`).
    pub const NEW_FPO: usize = 9;
    /// How many slots there are.
    pub const COUNT: usize = 11;
}

/// What the DBI stream's header names.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    /// The PDB's age.
    pub age: u32,
    /// The machine.
    pub machine: u16,
    /// The globals' stream.
    pub globals: u16,
    /// The publics' stream.
    pub publics: u16,
    /// The symbol records' stream.
    pub records: u16,
}

fn u32_of(n: usize) -> u32 {
    u32::try_from(n).expect("a DBI stream under 4 GB")
}

/// The DBI stream. `sections` is each image section's characteristics and
/// size; `contribs` sorted by section and offset.
pub fn dbi_stream(
    h: Header,
    modules: &[ModuleInfo],
    contribs: &[Contrib],
    sections: &[(u32, u32)],
    ec_names: &StringTable,
    debug_streams: &[u16; debug_stream::COUNT],
) -> Vec<u8> {
    let mut modi = Vec::new();
    for m in modules {
        modi.extend_from_slice(&0u32.to_le_bytes());
        m.first.write(&mut modi);
        modi.extend_from_slice(&0u16.to_le_bytes()); // flags
        modi.extend_from_slice(&m.stream.unwrap_or(0xFFFF).to_le_bytes());
        modi.extend_from_slice(&m.sym_bytes.to_le_bytes());
        modi.extend_from_slice(&0u32.to_le_bytes()); // C11
        modi.extend_from_slice(&m.c13_bytes.to_le_bytes());
        let files = u16::try_from(m.files.len()).unwrap_or(u16::MAX);
        modi.extend_from_slice(&files.to_le_bytes());
        modi.extend_from_slice(&0u16.to_le_bytes());
        modi.extend_from_slice(&0u32.to_le_bytes());
        modi.extend_from_slice(&0u32.to_le_bytes()); // source file name index
        modi.extend_from_slice(&m.pdb_path_ni.to_le_bytes());
        modi.extend_from_slice(&m.name);
        modi.push(0);
        modi.extend_from_slice(&m.obj);
        modi.push(0);
        while modi.len() % 4 != 0 {
            modi.push(0);
        }
    }
    let mut sc = Vec::with_capacity(4 + 28 * contribs.len());
    sc.extend_from_slice(&(0xEFFE_0000u32 + 19_970_605).to_le_bytes()); // V60
    for c in contribs {
        c.write(&mut sc);
    }
    // The section map: each section, then the absolute "section".
    let mut map = Vec::new();
    let n = u16::try_from(sections.len() + 1).unwrap_or(u16::MAX);
    map.extend_from_slice(&n.to_le_bytes());
    map.extend_from_slice(&n.to_le_bytes());
    let mut entry = |flags: u16, frame: u16, len: u32| {
        map.extend_from_slice(&flags.to_le_bytes());
        map.extend_from_slice(&0u16.to_le_bytes()); // ovl
        map.extend_from_slice(&0u16.to_le_bytes()); // group
        map.extend_from_slice(&frame.to_le_bytes());
        map.extend_from_slice(&u16::MAX.to_le_bytes()); // section name
        map.extend_from_slice(&u16::MAX.to_le_bytes()); // class name
        map.extend_from_slice(&0u32.to_le_bytes());
        map.extend_from_slice(&len.to_le_bytes());
    };
    for (i, &(chars, size)) in sections.iter().enumerate() {
        let mut f = 0x0008 | 0x0100; // 32-bit address, selector
        if chars & 0x4000_0000 != 0 {
            f |= 1;
        }
        if chars & 0x8000_0000 != 0 {
            f |= 2;
        }
        if chars & 0x2000_0000 != 0 {
            f |= 4;
        }
        entry(f, u16::try_from(i + 1).unwrap_or(u16::MAX), size);
    }
    entry(0x0208, n, u32::MAX);
    // Source files: per module, offsets of its files' names.
    let mut names = Vec::new();
    let mut name_at: turbonop_hash::Map<&[u8], u32> = turbonop_hash::Map::default();
    let mut offsets = Vec::new();
    for m in modules {
        for f in &m.files {
            let o = *name_at.entry(f.as_slice()).or_insert_with(|| {
                let o = u32_of(names.len());
                names.extend_from_slice(f);
                names.push(0);
                o
            });
            offsets.push(o);
        }
    }
    let mut fi = Vec::new();
    let nmods = u16::try_from(modules.len()).unwrap_or(u16::MAX);
    fi.extend_from_slice(&nmods.to_le_bytes());
    fi.extend_from_slice(&(u16::try_from(offsets.len()).unwrap_or(u16::MAX)).to_le_bytes());
    let mut start = 0usize;
    for m in modules {
        // (Truncated, as the reference implementation's field is.)
        fi.extend_from_slice(&u16::try_from(start & 0xFFFF).unwrap_or(0).to_le_bytes());
        start += m.files.len();
    }
    for m in modules {
        fi.extend_from_slice(
            &u16::try_from(m.files.len())
                .unwrap_or(u16::MAX)
                .to_le_bytes(),
        );
    }
    for o in &offsets {
        fi.extend_from_slice(&o.to_le_bytes());
    }
    fi.extend_from_slice(&names);
    while fi.len() % 4 != 0 {
        fi.push(0);
    }
    let ec = ec_names.serialize();
    let dbg: Vec<u8> = debug_streams.iter().flat_map(|s| s.to_le_bytes()).collect();

    let mut out = Vec::with_capacity(
        64 + modi.len() + sc.len() + map.len() + fi.len() + ec.len() + dbg.len(),
    );
    out.extend_from_slice(&(-1i32).to_le_bytes());
    out.extend_from_slice(&19_990_903u32.to_le_bytes()); // V70
    out.extend_from_slice(&h.age.to_le_bytes());
    out.extend_from_slice(&h.globals.to_le_bytes());
    out.extend_from_slice(&(0x8000u16 | (14 << 8) | 52).to_le_bytes()); // link 14.52's format
    out.extend_from_slice(&h.publics.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // PDB DLL version
    out.extend_from_slice(&h.records.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // PDB DLL rebuild
    for v in [modi.len(), sc.len(), map.len(), fi.len(), 0] {
        out.extend_from_slice(&u32_of(v).to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // MFC type server
    out.extend_from_slice(&u32_of(dbg.len()).to_le_bytes());
    out.extend_from_slice(&u32_of(ec.len()).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&h.machine.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&modi);
    out.extend_from_slice(&sc);
    out.extend_from_slice(&map);
    out.extend_from_slice(&fi);
    out.extend_from_slice(&ec);
    out.extend_from_slice(&dbg);
    out
}
