//! Writing PDBs, as a linker does: the MSF container ([`msf`]), the info,
//! TPI and IPI streams and the `/names` table ([`streams`]), the DBI
//! stream ([`dbi`]), the globals and publics ([`gsi`]), module streams
//! ([`module`]), and merging CodeView types from many objects on every
//! core ([`merge`]). [`write`] puts it all in a file.
//!
//! The PDB's GUID is a hash of its contents (with the GUID zeroed), so
//! the same inputs give the same file; the image's debug directory is
//! given the same GUID.

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod dbi;
pub mod gsi;
pub mod merge;
pub mod module;
pub mod msf;
pub mod streams;

pub use streams::StringTable;

/// Everything a PDB holds, from the linker.
pub struct Input {
    /// The machine (`IMAGE_FILE_MACHINE_*`).
    pub machine: u16,
    /// The age (1 for a new PDB).
    pub age: u32,
    /// The signature (the image's time stamp).
    pub signature: u32,
    /// The types (TPI) and ids (IPI).
    pub merged: merge::Merged,
    /// The modules, each with its stream (if it has one).
    pub modules: Vec<(dbi::ModuleInfo, Option<Vec<u8>>)>,
    /// The section contributions, by section and offset.
    pub contribs: Vec<dbi::Contrib>,
    /// Each image section's characteristics and size.
    pub sections: Vec<(u32, u32)>,
    /// The image's section headers (40 bytes each).
    pub section_headers: Vec<u8>,
    /// The public symbols.
    pub publics: Vec<gsi::Public>,
    /// The global symbols (whole records).
    pub globals: Vec<Vec<u8>>,
    /// The `/names` table.
    pub names: StringTable,
    /// The DBI's EC names.
    pub ec_names: StringTable,
    /// `FRAMEDATA` records (x86's new FPO), if any.
    pub new_fpo: Vec<u8>,
}

/// A written PDB.
pub struct Written {
    /// The file.
    pub file: Vec<u8>,
    /// Its GUID.
    pub guid: [u8; 16],
    /// Its age.
    pub age: u32,
    /// Where the GUID's bytes are in the file.
    pub guid_at: [usize; 16],
}

impl Written {
    /// Gives the PDB another GUID (`/Brepro`'s, from the image).
    pub fn set_guid(&mut self, guid: [u8; 16]) {
        for (&at, b) in self.guid_at.iter().zip(guid) {
            self.file[at] = b;
        }
        self.guid = guid;
    }
}

/// Fixed streams' numbers.
mod stream {
    pub const INFO: usize = 1;
    pub const NAMES: usize = 5;
    pub const TPI_HASH: usize = 6;
    pub const IPI_HASH: usize = 7;
    pub const GLOBALS: usize = 8;
    pub const PUBLICS: usize = 9;
    pub const RECORDS: usize = 10;
    pub const SECTION_HEADERS: usize = 11;
    pub const NEW_FPO: usize = 12;
    pub const FIRST_MODULE: usize = 13;
}

fn n16(n: usize) -> u16 {
    u16::try_from(n).expect("fewer than 65536 streams")
}

/// Writes the PDB (`block_size` 4096, unless the file needs larger).
pub fn write(input: Input, block_size: u32) -> Result<Written, msf::TooLarge> {
    let Input {
        machine,
        age,
        signature,
        merged,
        modules,
        contribs,
        sections,
        section_headers,
        publics,
        globals,
        names,
        ec_names,
        new_fpo,
    } = input;
    let (tpi, ipi) = turbonop_par::join(
        || {
            streams::tpi_streams(
                &merged.types.records,
                &merged.types.offsets,
                &merged.types.hashes,
                n16(stream::TPI_HASH),
            )
        },
        || {
            streams::tpi_streams(
                &merged.ids.records,
                &merged.ids.offsets,
                &merged.ids.hashes,
                n16(stream::IPI_HASH),
            )
        },
    );
    let timing = std::env::var_os("TURBOLINK_PDB_TIME").is_some();
    let mut last = std::time::Instant::now();
    let mut lap = |what: &str| {
        if timing {
            eprintln!(
                "  write: {what}: {:.1} ms",
                last.elapsed().as_secs_f64() * 1000.0
            );
        }
        last = std::time::Instant::now();
    };
    lap("tpi");
    let gsi = gsi::build(publics, globals);
    lap("gsi");
    let mut infos = Vec::with_capacity(modules.len());
    let mut module_streams = Vec::new();
    for (mut info, s) in modules {
        if let Some(s) = s {
            info.stream = Some(n16(stream::FIRST_MODULE + module_streams.len()));
            module_streams.push(s);
        }
        infos.push(info);
    }
    let mut debug = [0xFFFFu16; dbi::debug_stream::COUNT];
    debug[dbi::debug_stream::SECTION_HEADERS] = n16(stream::SECTION_HEADERS);
    if !new_fpo.is_empty() {
        debug[dbi::debug_stream::NEW_FPO] = n16(stream::NEW_FPO);
    }
    let dbi = dbi::dbi_stream(
        dbi::Header {
            age,
            machine,
            globals: n16(stream::GLOBALS),
            publics: n16(stream::PUBLICS),
            records: n16(stream::RECORDS),
        },
        &infos,
        &contribs,
        &sections,
        &ec_names,
        &debug,
    );
    let names = names.serialize();
    let zero = [0u8; 16];
    let info = streams::info_stream(
        signature,
        age,
        &zero,
        &[("/names", u32::from(n16(stream::NAMES)))],
    );
    let mut list: Vec<&[u8]> = vec![
        b"",
        &info,
        &tpi.0,
        &dbi,
        &ipi.0,
        &names,
        &tpi.1,
        &ipi.1,
        &gsi.globals,
        &gsi.publics,
        &gsi.records,
        &section_headers,
        &new_fpo,
    ];
    list.extend(module_streams.iter().map(Vec::as_slice));
    lap("dbi, names, info");
    let (mut file, layout) = msf::write(&list, block_size)?;
    lap("msf");
    // The GUID: a hash of the file with the GUID zeroed, in the info
    // stream at offset 12.
    let guid = content_guid(&file);
    let guid_at: [usize; 16] = std::array::from_fn(|i| layout.file_offset(stream::INFO, 12 + i));
    for (&at, b) in guid_at.iter().zip(guid) {
        file[at] = b;
    }
    lap("guid");
    Ok(Written {
        file,
        guid,
        age,
        guid_at,
    })
}

/// A GUID from `bytes` (a 128-bit hash of them, in parallel chunks, with
/// the version and variant bits of a version 4 GUID).
pub fn content_guid(bytes: &[u8]) -> [u8; 16] {
    const CHUNK: usize = 1 << 20;
    let parts: Vec<u64> = turbonop_par::map_range(bytes.len().div_ceil(CHUNK), 1, |i| {
        let c = &bytes[i * CHUNK..bytes.len().min((i + 1) * CHUNK)];
        let mut h = 0x243F_6A88_85A3_08D3u64 ^ i as u64;
        let (words, rest) = c.as_chunks::<8>();
        for w in words {
            let p = u128::from(h ^ u64::from_le_bytes(*w)) * 0x9E37_79B9_7F4A_7C15u128;
            h = lo(p) ^ hi(p);
        }
        for &b in rest {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3);
        }
        h
    });
    let mut a = 0x1319_8A2E_0370_7344u64;
    let mut b = 0xA409_3822_299F_31D0u64;
    for p in parts {
        let x = u128::from(a ^ p) * 0xC2B2_AE3D_27D4_EB4Fu128;
        a = lo(x) ^ hi(x);
        let y = u128::from(b ^ p.rotate_left(31)) * 0x1656_67B1_9E37_79F9u128;
        b = lo(y) ^ hi(y);
    }
    let mut g = [0u8; 16];
    g[..8].copy_from_slice(&a.to_le_bytes());
    g[8..].copy_from_slice(&b.to_le_bytes());
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

/// `n` as a `u32` (a stream's records are under 4 GB).
pub(crate) fn n32(n: usize) -> u32 {
    u32::try_from(n).expect("under 4 GB")
}

/// The low 64 bits of `x` (a product's half, folded).
pub(crate) fn lo(x: u128) -> u64 {
    u64::try_from(x & u128::from(u64::MAX)).expect("masked")
}

/// The high 64 bits of `x`.
pub(crate) fn hi(x: u128) -> u64 {
    u64::try_from(x >> 64).expect("shifted")
}
