//! A COFF object's CodeView (its `.debug$S` and `.debug$T` sections) as a
//! PDB, so that everything that reads a PDB reads an object's debug
//! information too ([`pdb_of`]).
//!
//! The PDB is the one a linker would write for a program of the object
//! alone, at the addresses its sections would have
//! ([`PeFile::section_addresses`]): one module, whose symbols are the
//! object's, with their relocations applied (each `section:offset` is a
//! section of the object and an offset in it), their scopes linked (their
//! parent and end offsets, which an object leaves 0), and procedures
//! named by id (`S_GPROC32_ID`) turned into procedures of a type
//! (`S_GPROC32`), as a linker turns them; its line tables and file
//! checksums; the `/names` table its checksums name files in (the
//! object's string table); its types, as both the type and the id stream
//! (an object's types and ids share one index space); its publics (its
//! external symbols, as a linker's: hashed, and by address); and the
//! object's section headers, at those addresses.
//!
//! An object whose types are in a PDB of their own (`/Zi`: its `.debug$T`
//! a reference, `LF_TYPESERVER2`, to the PDB [`type_server`] names) has
//! that PDB's types and ids, given ([`pdb_of_with`]); else, as an object
//! whose types are a precompiled header's (`LF_PRECOMP`), its symbols and
//! lines, and no types.

use hexcavator_codeview::lines::subsection;
use hexcavator_codeview::symbols::kind;
use hexcavator_core::Reader;
use hexcavator_pe::{FileKind, PeFile};

/// `CV_SIGNATURE_C13`: what `.debug$S` and `.debug$T` start with.
const SIGNATURE: u32 = 4;

/// The leaves a `.debug$T` that holds no types of its own starts with.
const LF_TYPESERVER2: u16 = 0x1515;
const LF_PRECOMP: u16 = 0x1509;
/// The ids of functions, with their types after their scopes.
const LF_FUNC_ID: u16 = 0x1601;
const LF_MFUNC_ID: u16 = 0x1602;

/// The first type index (those below are the basic types).
const FIRST_TYPE: u32 = 0x1000;

/// An MSF block's size.
const BLOCK: usize = 4096;

/// How many buckets the type hash has (as MSVC's linker makes it).
const HASH_BUCKETS: u32 = 0x3FFFF;

/// The PDB of the COFF object `data`'s CodeView, if it is an object with
/// some (a `.debug$S` section that describes something: more than the
/// object and its compiler, which clang records in every object): see the
/// module's documentation.
pub fn pdb_of(data: &[u8]) -> Option<Vec<u8>> {
    pdb_of_with(data, None)
}

/// The PDB whose types an object compiled with `/Zi` uses (its
/// `.debug$T`'s `LF_TYPESERVER2`): its GUID, age, and path, as the
/// compiler recorded them.
pub fn type_server(data: &[u8]) -> Option<([u8; 16], u32, &[u8])> {
    let pe = PeFile::parse(data).ok()?;
    if pe.kind() != FileKind::Object {
        return None;
    }
    let t = pe
        .sections()
        .iter()
        .filter(|s| pe.section_name(s) == b".debug$T")
        .find_map(|s| section_bytes(&pe, s))?;
    let mut r = Reader::new(t.get(4..)?);
    let len = usize::from(r.u16_le().ok()?);
    let body = r.bytes(len).ok()?;
    if u16::from_le_bytes([*body.first()?, *body.get(1)?]) != LF_TYPESERVER2 {
        return None;
    }
    let guid: [u8; 16] = body.get(2..18)?.try_into().ok()?;
    let age = u32::from_le_bytes(body.get(18..22)?.try_into().ok()?);
    let name = body.get(22..)?;
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some((guid, age, &name[..end]))
}

/// [`pdb_of`], the types and ids of an object compiled with `/Zi` from
/// `server` (the PDB [`type_server`] names, its contents), if given and
/// it is one: its type and id streams, as they are (the object's symbols
/// index them).
pub fn pdb_of_with(data: &[u8], server: Option<&[u8]>) -> Option<Vec<u8>> {
    let pe = PeFile::parse(data).ok()?;
    if pe.kind() != FileKind::Object {
        return None;
    }
    let sections = pe.sections();
    let addresses = pe.section_addresses();
    let named = |name: &'static [u8]| {
        sections
            .iter()
            .enumerate()
            .filter(move |(_, s)| pe.section_name(s) == name)
    };
    named(b".debug$S").next()?;
    // The types (and ids), in index order from FIRST_TYPE.
    let types = named(b".debug$T")
        .chain(named(b".debug$P"))
        .find_map(|(_, s)| section_bytes(&pe, s))
        .and_then(type_records);
    // A type server's types (only for an object that names one).
    let server = server
        .filter(|_| types.is_none())
        .and_then(|s| hexcavator_pdb::Pdb::parse(s).ok());
    let types = types.unwrap_or_default();
    // The type of each function id: the object's, or its type server's.
    let function = |id: u32| -> Option<u32> {
        if let Some(s) = &server {
            let r = s.ids().ok()?.record(hexcavator_codeview::TypeIndex(id))?;
            if !matches!(r.kind, LF_FUNC_ID | LF_MFUNC_ID) {
                return None;
            }
            return Some(u32::from_le_bytes(r.data.get(4..8)?.try_into().ok()?));
        }
        function_type(&types, id)
    };
    let mut module = Module::default();
    for (index, s) in named(b".debug$S") {
        let Some(bytes) = section_bytes(&pe, s) else {
            continue;
        };
        let mut bytes = bytes.to_vec();
        relocate(&pe, index, &mut bytes);
        module.add(&bytes, &function);
    }
    if !module.describes {
        return None;
    }
    let object_name = module.object_name.clone().unwrap_or_default();
    // The section headers, each at its address.
    let mut headers = Vec::with_capacity(sections.len() * 40);
    for (s, &at) in sections.iter().zip(&addresses) {
        headers.extend_from_slice(&s.name);
        headers.extend_from_slice(&s.size_of_raw_data.get().to_le_bytes());
        headers.extend_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
        headers.extend_from_slice(&s.size_of_raw_data.get().to_le_bytes());
        headers.extend_from_slice(&[0; 12]);
        headers.extend_from_slice(&[0; 4]);
        headers.extend_from_slice(&s.characteristics.get().to_le_bytes());
    }
    // The module's contributions: each section the image would hold.
    let contributions: Vec<Contribution> = sections
        .iter()
        .zip(&addresses)
        .enumerate()
        .filter(|(_, (s, at))| **at != 0 && s.size_of_raw_data.get() > 0)
        .filter_map(|(i, (s, _))| {
            Some(Contribution {
                section: u16::try_from(i + 1).ok()?,
                size: s.size_of_raw_data.get(),
                characteristics: s.characteristics.get(),
            })
        })
        .collect();
    // The types and ids: the object's, both alike; or its type server's,
    // their streams as they are (their hashes' streams renumbered).
    let (tpi, hash, ipi, ipi_hash) = match &server {
        Some(s) => {
            let stream = |n: u32, hash: u16| -> Option<(Vec<u8>, Vec<u8>)> {
                let mut t = s.msf().stream_or_empty(n).ok()?.into_owned();
                let h = match hexcavator_pdb::tpi::hash_stream_of(&t) {
                    Some(i) => s.msf().stream_or_empty(i).ok()?.into_owned(),
                    None => Vec::new(),
                };
                if let Some(at) = t.get_mut(20..22) {
                    at.copy_from_slice(&hash.to_le_bytes());
                }
                Some((t, h))
            };
            let (tpi, hash) = stream(hexcavator_pdb::stream::TPI, stream::HASH)?;
            let (ipi, ipi_hash) = stream(hexcavator_pdb::stream::IPI, stream::IPI_HASH)?;
            (tpi, hash, ipi, ipi_hash)
        }
        None => {
            let (tpi, hash) = type_stream(&types, stream::HASH);
            (tpi.clone(), hash, tpi, Vec::new())
        }
    };
    let (records, publics) = publics(&pe);
    let streams = [
        Vec::new(),
        info_stream(stream::NAMES),
        tpi,
        dbi_stream(&pe, &module, &object_name, &contributions),
        ipi,
        module.stream(),
        names_stream(&module.strings),
        headers,
        hash,
        records,
        publics,
        symbol_hash(&[]),
        ipi_hash,
    ];
    msf(&streams)
}

/// The symbol records of the object's publics, as a linker makes them
/// (an `S_PUB32` for each external symbol it defines), and the publics
/// stream: their hash, and their address map.
fn publics(pe: &PeFile<'_>) -> (Vec<u8>, Vec<u8>) {
    use hexcavator_pe::consts::storage_class;
    let mut records = Vec::new();
    // (Offset, section, offset in it, name) of each.
    let mut publics: Vec<(u32, u16, u32, Vec<u8>)> = Vec::new();
    let sections = pe.sections();
    if let Ok(Some(symbols)) = pe.symbols() {
        for sym in symbols.iter() {
            if sym.storage_class != storage_class::EXTERNAL || sym.section < 1 {
                continue;
            }
            let (Ok(section), Some(name)) = (u16::try_from(sym.section), symbols.name(&sym)) else {
                continue;
            };
            let code = sections
                .get(usize::from(section) - 1)
                .is_some_and(|h| h.characteristics.get() & 0x20 != 0);
            // cvpsfCode, and cvpsfFunction for a function's.
            let flags = u32::from(code) | if sym.kind & 0x30 == 0x20 { 2 } else { 0 };
            let at = u32::try_from(records.len()).unwrap_or(0);
            let mut data = Vec::new();
            u32s(&mut data, &[flags, sym.value]);
            u16s(&mut data, &[section]);
            data.extend_from_slice(name);
            data.push(0);
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
            let Ok(len) = u16::try_from(data.len() + 2) else {
                continue;
            };
            u16s(&mut records, &[len, kind::S_PUB32]);
            records.extend_from_slice(&data);
            publics.push((at, section, sym.value, name.to_vec()));
        }
    }
    let names: Vec<(u32, &[u8])> = publics.iter().map(|p| (p.0, &p.3[..])).collect();
    let hash = symbol_hash(&names);
    let mut by_address: Vec<&(u32, u16, u32, Vec<u8>)> = publics.iter().collect();
    by_address.sort_by_key(|p| (p.1, p.2));
    let mut out = Vec::new();
    u32s(
        &mut out,
        &[
            u32::try_from(hash.len()).unwrap_or(0),
            u32::try_from(by_address.len() * 4).unwrap_or(0),
            0,
            0,
        ],
    );
    u16s(&mut out, &[0, 0]);
    u32s(&mut out, &[0, u32::try_from(sections.len()).unwrap_or(0)]);
    out.extend_from_slice(&hash);
    for p in by_address {
        u32s(&mut out, &[p.0]);
    }
    (records, out)
}

/// A symbol hash (a GSI's, or a PSI's): each symbol (its offset in the
/// record stream, and its name) in its name's bucket.
fn symbol_hash(symbols: &[(u32, &[u8])]) -> Vec<u8> {
    const BUCKETS: usize = 4096;
    let mut by_bucket: Vec<(usize, u32)> = symbols
        .iter()
        .map(|&(at, name)| (hexcavator_pdb::hash::hash_v1(name) as usize % BUCKETS, at))
        .collect();
    by_bucket.sort_unstable();
    let mut records = Vec::new();
    for &(_, at) in &by_bucket {
        u32s(&mut records, &[at + 1, 1]);
    }
    let mut bitmap = vec![0u32; (BUCKETS + 1).div_ceil(32)];
    let mut starts = Vec::new();
    for (i, &(b, _)) in by_bucket.iter().enumerate() {
        if i == 0 || by_bucket[i - 1].0 != b {
            bitmap[b / 32] |= 1 << (b % 32);
            starts.push(u32::try_from(i * 12).unwrap_or(0));
        }
    }
    let mut buckets = Vec::new();
    u32s(&mut buckets, &bitmap);
    u32s(&mut buckets, &starts);
    let mut out = Vec::new();
    u32s(
        &mut out,
        &[
            u32::MAX,
            0xEFFE_0000 + 19_990_810,
            u32::try_from(records.len()).unwrap_or(0),
            u32::try_from(buckets.len()).unwrap_or(0),
        ],
    );
    out.extend_from_slice(&records);
    out.extend_from_slice(&buckets);
    out
}

/// The streams the PDB has (besides the fixed ones: the info stream, the
/// TPI, the DBI, and the IPI).
mod stream {
    pub const MODULE: u16 = 5;
    pub const NAMES: u32 = 6;
    pub const SECTION_HEADERS: u16 = 7;
    pub const HASH: u16 = 8;
    pub const RECORDS: u16 = 9;
    pub const PUBLICS: u16 = 10;
    pub const GLOBALS: u16 = 11;
    /// The IPI's hash, a type server's (an object's types and ids share
    /// the TPI's).
    pub const IPI_HASH: u16 = 12;
}

/// A section's bytes, if it starts with the CodeView signature (after it).
fn section_bytes<'d>(pe: &PeFile<'d>, s: &hexcavator_pe::raw::SectionHeader) -> Option<&'d [u8]> {
    let at = usize::try_from(s.pointer_to_raw_data.get()).ok()?;
    let len = usize::try_from(s.size_of_raw_data.get()).ok()?;
    let bytes = pe.data().get(at..at.checked_add(len)?)?;
    (bytes.get(..4)? == SIGNATURE.to_le_bytes()).then_some(bytes)
}

/// Applies the relocations of section `index` (0-based) that CodeView
/// uses to `data`, its contents: each `SECREL` (a symbol's offset in its
/// section, added to the bytes it patches) and `SECTION` (its section's
/// number).
fn relocate(pe: &PeFile<'_>, index: usize, data: &mut [u8]) {
    use hexcavator_pe::consts::machine as m;
    let Some(header) = pe.sections().get(index) else {
        return;
    };
    let Ok(Some(symbols)) = pe.symbols() else {
        return;
    };
    // (SECTION, SECREL) by machine.
    let (section, secrel) = match pe.machine() {
        m::I386 | m::AMD64 => (0x0A, 0x0B),
        m::ARM64 | m::ARM64EC | m::ARM64X => (0x0D, 0x08),
        m::ARM | m::ARMNT | m::THUMB => (0x0E, 0x0F),
        _ => return,
    };
    for r in pe.section_relocations(header) {
        let Some(sym) = symbols.get(r.symbol_table_index.get() as usize) else {
            continue;
        };
        let Ok(at) = usize::try_from(r.virtual_address.get()) else {
            continue;
        };
        let kind = r.kind.get();
        if kind == secrel {
            let Some(place) = data.get_mut(at..at.saturating_add(4)) else {
                continue;
            };
            let addend = u32::from_le_bytes([place[0], place[1], place[2], place[3]]);
            place.copy_from_slice(&sym.value.wrapping_add(addend).to_le_bytes());
        } else if kind == section {
            let Some(place) = data.get_mut(at..at.saturating_add(2)) else {
                continue;
            };
            let n = u16::try_from(sym.section).unwrap_or(0);
            place.copy_from_slice(&n.to_le_bytes());
        }
    }
}

/// The type records of a `.debug$T` (after its signature), each its kind
/// and contents, or none if it holds none of its own.
fn type_records(data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut r = Reader::new(data.get(4..)?);
    let mut out = Vec::new();
    while !r.is_empty() {
        let len = usize::from(r.u16_le().ok()?);
        let Ok(body) = r.bytes(len) else { break };
        if body.len() < 2 {
            break;
        }
        out.push(body);
    }
    let first = out.first().map(|b| u16::from_le_bytes([b[0], b[1]]));
    if matches!(first, Some(LF_TYPESERVER2 | LF_PRECOMP)) {
        return None;
    }
    Some(out)
}

/// The type a function id (`LF_FUNC_ID`, `LF_MFUNC_ID`) names.
fn function_type(types: &[&[u8]], id: u32) -> Option<u32> {
    let body = types.get(usize::try_from(id.checked_sub(FIRST_TYPE)?).ok()?)?;
    let kind = u16::from_le_bytes([body[0], body[1]]);
    if !matches!(kind, LF_FUNC_ID | LF_MFUNC_ID) {
        return None;
    }
    let t = body.get(6..10)?;
    Some(u32::from_le_bytes([t[0], t[1], t[2], t[3]]))
}

/// A section's contribution to the module.
struct Contribution {
    section: u16,
    size: u32,
    characteristics: u32,
}

/// The module stream, built from the object's `.debug$S` sections.
#[derive(Default)]
struct Module {
    /// The symbol records (after the stream's signature).
    symbols: Vec<u8>,
    /// The C13 subsections: lines, file checksums, inlinee lines.
    c13: Vec<u8>,
    /// The string table the file checksums name files in.
    strings: Vec<u8>,
    /// Whether a file checksum subsection was kept (the first is: the
    /// object's lines all name its files by their offsets in it).
    checksums: Option<Vec<u8>>,
    /// The open scopes: their records' offsets in the stream.
    scopes: Vec<usize>,
    /// The object's name, from its `S_OBJNAME`.
    object_name: Option<Vec<u8>>,
    /// Whether it describes anything (a symbol other than the object's
    /// and its compiler's, or a line).
    describes: bool,
}

impl Module {
    /// Adds a `.debug$S` section's subsections (its contents, relocated).
    fn add(&mut self, data: &[u8], function: &dyn Fn(u32) -> Option<u32>) {
        let mut r = Reader::new(data.get(4..).unwrap_or_default());
        while !r.is_empty() {
            let (Ok(kind), Ok(len)) = (r.u32_le(), r.u32_le()) else {
                break;
            };
            let Ok(body) = r.bytes(len as usize) else {
                break;
            };
            // (Each subsection is padded to 4 bytes.)
            let _ = r.align_to(4);
            match kind {
                subsection::DEBUG_S_SYMBOLS => self.add_symbols(body, function),
                subsection::DEBUG_S_LINES | subsection::DEBUG_S_INLINEELINES => {
                    self.describes = true;
                    push_subsection(&mut self.c13, kind, body);
                }
                subsection::DEBUG_S_FILECHKSMS if self.checksums.is_none() => {
                    self.checksums = Some(body.to_vec());
                }
                subsection::DEBUG_S_STRINGTABLE if self.strings.is_empty() => {
                    self.strings = body.to_vec();
                }
                _ => {}
            }
        }
    }

    /// Adds a symbols subsection's records.
    fn add_symbols(&mut self, body: &[u8], function: &dyn Fn(u32) -> Option<u32>) {
        let mut r = Reader::new(body);
        while !r.is_empty() {
            let Ok(len) = r.u16_le() else { break };
            let Ok(record) = r.bytes(usize::from(len)) else {
                break;
            };
            if record.len() < 2 {
                break;
            }
            let mut k = u16::from_le_bytes([record[0], record[1]]);
            let mut data = record[2..].to_vec();
            // Procedures by id, as a linker writes them: by type.
            let by_type = match k {
                kind::S_GPROC32_ID => Some(kind::S_GPROC32),
                kind::S_LPROC32_ID => Some(kind::S_LPROC32),
                kind::S_LPROC32_DPC_ID => Some(kind::S_LPROC32_DPC),
                _ => None,
            };
            if let Some(to) = by_type {
                k = to;
                if let Some(t) = data.get_mut(24..28) {
                    let id = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
                    let ty = function(id).unwrap_or(0);
                    t.copy_from_slice(&ty.to_le_bytes());
                }
            }
            if k == kind::S_PROC_ID_END {
                k = kind::S_END;
            }
            self.describes |= !matches!(
                k,
                kind::S_OBJNAME
                    | kind::S_COMPILE2
                    | kind::S_COMPILE3
                    | kind::S_ENVBLOCK
                    | kind::S_BUILDINFO
            );
            if k == kind::S_OBJNAME && self.object_name.is_none() {
                let name = data.get(4..).unwrap_or_default();
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                self.object_name = Some(name[..end].to_vec());
            }
            // Where it goes in the stream (after the signature).
            let at = 4 + self.symbols.len();
            if kind::closes_scope(k)
                && let Some(open) = self.scopes.pop()
            {
                // The opening record's end: this one.
                let end = open - 4 + 8;
                if let Some(e) = self.symbols.get_mut(end..end + 4) {
                    e.copy_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
                }
            }
            if kind::opens_scope(k) {
                let parent = self.scopes.last().map_or(0, |&p| p);
                if let Some(p) = data.get_mut(..4) {
                    p.copy_from_slice(&u32::try_from(parent).unwrap_or(0).to_le_bytes());
                }
                if data.len() >= 8 {
                    self.scopes.push(at);
                }
            }
            // (Records are aligned to 4 bytes in a PDB.)
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
            let Ok(len) = u16::try_from(data.len() + 2) else {
                continue;
            };
            self.symbols.extend_from_slice(&len.to_le_bytes());
            self.symbols.extend_from_slice(&k.to_le_bytes());
            self.symbols.extend_from_slice(&data);
        }
    }

    /// The module's files, from its checksums: their names' offsets in
    /// the string table.
    fn files(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let Some(c) = &self.checksums else {
            return out;
        };
        let mut r = Reader::new(c);
        while !r.is_empty() {
            let Ok(name) = r.u32_le() else { break };
            let (Ok(size), Ok(_)) = (r.u8(), r.u8()) else {
                break;
            };
            if r.skip(usize::from(size)).is_err() {
                break;
            }
            let _ = r.align_to(4);
            out.push(name);
        }
        out
    }

    /// The stream: its signature, symbols, C13 subsections, and global
    /// references (none).
    fn stream(&self) -> Vec<u8> {
        let mut out = SIGNATURE.to_le_bytes().to_vec();
        out.extend_from_slice(&self.symbols);
        out.extend_from_slice(&self.c13_bytes());
        u32s(&mut out, &[0]);
        out
    }

    fn c13_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(c) = &self.checksums {
            push_subsection(&mut out, subsection::DEBUG_S_FILECHKSMS, c);
        }
        out.extend_from_slice(&self.c13);
        out
    }
}

fn push_subsection(out: &mut Vec<u8>, kind: u32, body: &[u8]) {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(body);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn u32s(out: &mut Vec<u8>, values: &[u32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn u16s(out: &mut Vec<u8>, values: &[u16]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// The info stream: its header, and the named stream map's `/names`.
fn info_stream(names: u32) -> Vec<u8> {
    let mut out = Vec::new();
    // Version (VC70), signature, age; the GUID (none: no image names it).
    u32s(&mut out, &[20_000_404, 0, 1]);
    out.extend_from_slice(&[0; 16]);
    let strings = b"/names\0";
    u32s(&mut out, &[u32::try_from(strings.len()).unwrap_or(0)]);
    out.extend_from_slice(strings);
    // The hash table: one entry (size, capacity, its present bit, no
    // deleted bits), "/names" (at 0) to its stream.
    u32s(&mut out, &[1, 1, 1, 1, 0, 0, names]);
    // (Its features: an IPI, as VC140's.)
    u32s(&mut out, &[hexcavator_pdb::info::feature::VC140]);
    out
}

/// The type stream (the IPI's too) and its hash stream.
fn type_stream(types: &[&[u8]], hash_stream: u16) -> (Vec<u8>, Vec<u8>) {
    let mut records = Vec::new();
    let mut hashes = Vec::with_capacity(types.len() * 4);
    let mut index = Vec::new();
    let mut next_index = 0;
    for (i, body) in types.iter().enumerate() {
        let ti = FIRST_TYPE + u32::try_from(i).unwrap_or(u32::MAX - FIRST_TYPE);
        // (An index entry every 8 KB, as MSVC's linker writes them.)
        if records.len() >= next_index {
            u32s(&mut index, &[ti, u32::try_from(records.len()).unwrap_or(0)]);
            next_index = records.len() + 8192;
        }
        let Ok(len) = u16::try_from(body.len()) else {
            break;
        };
        records.extend_from_slice(&len.to_le_bytes());
        records.extend_from_slice(body);
        // A user-defined type by its name; the rest in bucket 0 (looked
        // up by index alone).
        let mut rr = Reader::new(&records[records.len() - body.len() - 2..]);
        let hash = hexcavator_codeview::types::TypeRecord::read(&mut rr)
            .ok()
            .and_then(|t| t.udt())
            .map_or(0, |u| hexcavator_pdb::hash::hash_v1(u.name) % HASH_BUCKETS);
        u32s(&mut hashes, &[hash]);
    }
    let count = u32::try_from(hashes.len() / 4).unwrap_or(0);
    let mut tpi = Vec::new();
    u32s(
        &mut tpi,
        &[
            20_040_203,
            56,
            FIRST_TYPE,
            FIRST_TYPE + count,
            u32::try_from(records.len()).unwrap_or(0),
        ],
    );
    u16s(&mut tpi, &[hash_stream, 0xFFFF]);
    let hashes_len = u32::try_from(hashes.len()).unwrap_or(0);
    let index_len = u32::try_from(index.len()).unwrap_or(0);
    u32s(
        &mut tpi,
        &[
            4,
            HASH_BUCKETS,
            0,
            hashes_len,
            hashes_len,
            index_len,
            hashes_len + index_len,
            0,
        ],
    );
    tpi.extend_from_slice(&records);
    let mut hash = hashes;
    hash.extend_from_slice(&index);
    (tpi, hash)
}

/// The DBI stream: its header, the one module, its contributions, its
/// files, and the optional debug header (the section headers' stream).
fn dbi_stream(
    pe: &PeFile<'_>,
    module: &Module,
    object_name: &[u8],
    contributions: &[Contribution],
) -> Vec<u8> {
    let contribution = |out: &mut Vec<u8>, c: Option<&Contribution>| {
        let (section, size, characteristics) =
            c.map_or((0, 0, 0), |c| (c.section, c.size, c.characteristics));
        u16s(out, &[section, 0]);
        u32s(out, &[0, size, characteristics]);
        u16s(out, &[0, 0]);
        u32s(out, &[0, 0]);
    };
    // The module.
    let mut modules = Vec::new();
    u32s(&mut modules, &[0]);
    contribution(&mut modules, contributions.first());
    let symbols = u32::try_from(module.symbols.len() + 4).unwrap_or(0);
    let c13 = u32::try_from(module.c13_bytes().len()).unwrap_or(0);
    let files = module.files();
    u16s(&mut modules, &[0, stream::MODULE]);
    u32s(&mut modules, &[symbols, 0, c13]);
    u16s(&mut modules, &[u16::try_from(files.len()).unwrap_or(0), 0]);
    u32s(&mut modules, &[0, 0, 0]);
    for name in [object_name, object_name] {
        modules.extend_from_slice(name);
        modules.push(0);
    }
    while !modules.len().is_multiple_of(4) {
        modules.push(0);
    }
    // Its contributions (version 6.0's).
    let mut contribs = Vec::new();
    u32s(&mut contribs, &[0xEFFE_0000 + 19_970_605]);
    for c in contributions {
        contribution(&mut contribs, Some(c));
    }
    // Its files: their names, from the string table.
    let mut info = Vec::new();
    let count = u16::try_from(files.len()).unwrap_or(0);
    u16s(&mut info, &[1, 0, 0, count]);
    let mut names = Vec::new();
    for &f in files.iter().take(usize::from(count)) {
        u32s(&mut info, &[u32::try_from(names.len()).unwrap_or(0)]);
        let s = module.strings.get(f as usize..).unwrap_or_default();
        let end = hexcavator_core::find_nul(s).unwrap_or(s.len());
        names.extend_from_slice(&s[..end]);
        names.push(0);
    }
    info.extend_from_slice(&names);
    while !info.len().is_multiple_of(4) {
        info.push(0);
    }
    // The optional debug header: the section headers' stream alone.
    let mut debug = Vec::new();
    for i in 0..11 {
        let s = if i == hexcavator_pdb::dbi::dbg_stream::SECTION_HEADERS {
            stream::SECTION_HEADERS
        } else {
            0xFFFF
        };
        u16s(&mut debug, &[s]);
    }
    let size = |v: &[u8]| u32::try_from(v.len()).unwrap_or(0);
    let mut out = Vec::new();
    // Its signature (-1), version (V70), and age.
    u32s(&mut out, &[u32::MAX, 19_990_903, 1]);
    // The global and public hashes, the build (14.0's format), and the
    // symbol records.
    u16s(
        &mut out,
        &[
            stream::GLOBALS,
            0x8E00,
            stream::PUBLICS,
            0,
            stream::RECORDS,
            0,
        ],
    );
    u32s(
        &mut out,
        &[
            size(&modules),
            size(&contribs),
            0,
            size(&info),
            0,
            0,
            size(&debug),
            0,
        ],
    );
    u16s(&mut out, &[0, pe.machine()]);
    u32s(&mut out, &[0]);
    out.extend_from_slice(&modules);
    out.extend_from_slice(&contribs);
    out.extend_from_slice(&info);
    out.extend_from_slice(&debug);
    out
}

/// The `/names` stream: the object's string table, and its hash (each
/// string's offset in the bucket its hash names, the next free one).
fn names_stream(strings: &[u8]) -> Vec<u8> {
    let mut offsets = Vec::new();
    let mut at = 0;
    while at < strings.len() {
        let end = strings[at..]
            .iter()
            .position(|&b| b == 0)
            .map_or(strings.len(), |e| at + e);
        if end > at {
            offsets.push(at);
        }
        at = end + 1;
    }
    let count = offsets.len() * 2 + 1;
    let mut buckets = vec![0u32; count];
    for &o in &offsets {
        let end = strings[o..]
            .iter()
            .position(|&b| b == 0)
            .map_or(strings.len(), |e| o + e);
        let start = hexcavator_pdb::hash::hash_v1(&strings[o..end]) as usize % count;
        for probe in 0..count {
            let b = &mut buckets[(start + probe) % count];
            if *b == 0 {
                *b = u32::try_from(o).unwrap_or(0);
                break;
            }
        }
    }
    let mut out = Vec::new();
    u32s(
        &mut out,
        &[0xEFFE_EFFE, 1, u32::try_from(strings.len()).unwrap_or(0)],
    );
    out.extend_from_slice(strings);
    u32s(&mut out, &[u32::try_from(count).unwrap_or(0)]);
    u32s(&mut out, &buckets);
    u32s(&mut out, &[u32::try_from(offsets.len()).unwrap_or(0)]);
    out
}

/// An MSF 7.0 file of `streams`: the superblock, the free block map, each
/// stream's blocks in turn, the directory's, and the block that lists
/// the directory's. None if the directory needs more blocks than one
/// block can list.
fn msf(streams: &[Vec<u8>]) -> Option<Vec<u8>> {
    let blocks_of = |len: usize| len.div_ceil(BLOCK);
    let mut next = 3u32;
    let mut lists = Vec::with_capacity(streams.len());
    for s in streams {
        let n = u32::try_from(blocks_of(s.len())).ok()?;
        lists.push(next..next.checked_add(n)?);
        next = next.checked_add(n)?;
    }
    let mut dir = Vec::new();
    u32s(&mut dir, &[u32::try_from(streams.len()).ok()?]);
    for s in streams {
        u32s(&mut dir, &[u32::try_from(s.len()).ok()?]);
    }
    for l in &lists {
        for b in l.clone() {
            u32s(&mut dir, &[b]);
        }
    }
    let dir_blocks = u32::try_from(blocks_of(dir.len())).ok()?;
    if dir_blocks as usize > BLOCK / 4 {
        return None;
    }
    let dir_start = next;
    let map = dir_start.checked_add(dir_blocks)?;
    let total = map.checked_add(1)?;
    let mut out = Vec::with_capacity(total as usize * BLOCK);
    out.extend_from_slice(hexcavator_pdb::msf::MAGIC);
    u32s(
        &mut out,
        &[
            u32::try_from(BLOCK).ok()?,
            1,
            total,
            u32::try_from(dir.len()).ok()?,
            0,
            map,
        ],
    );
    out.resize(BLOCK, 0);
    // The free block map (every block in use) and its alternate.
    out.resize(3 * BLOCK, 0);
    let pad = |out: &mut Vec<u8>| out.resize(out.len().next_multiple_of(BLOCK), 0);
    for s in streams {
        out.extend_from_slice(s);
        pad(&mut out);
    }
    out.extend_from_slice(&dir);
    pad(&mut out);
    for b in dir_start..map {
        u32s(&mut out, &[b]);
    }
    pad(&mut out);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_an_object() {
        assert!(pdb_of(b"").is_none());
        assert!(pdb_of(b"MZ").is_none());
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata")
            .join(name);
        std::fs::read(path).unwrap()
    }

    /// MSVC's object (`/Z7`): one module of its symbols, each scope
    /// linked to its parent and its end, procedures by type; its types
    /// and ids; its publics; and its sections at their laid-out
    /// addresses.
    #[test]
    fn an_msvc_object() {
        let object = fixture("coff/objects.obj");
        let data = pdb_of(&object).unwrap();
        let pdb = hexcavator_pdb::Pdb::parse(&data).unwrap();
        let dbi = pdb.dbi().unwrap();
        assert_eq!(dbi.machine(), hexcavator_pe::consts::machine::AMD64);
        let modules = dbi.modules();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, br"R:\testdata\coff\objects.obj");
        let stream = pdb.module_stream(&modules[0]).unwrap().unwrap();
        let mut open: Vec<u32> = Vec::new();
        let mut procedures = 0;
        for r in stream.symbols() {
            assert!(r.kind != kind::S_GPROC32_ID && r.kind != kind::S_PROC_ID_END);
            if kind::closes_scope(r.kind) {
                let start = open.pop().unwrap();
                let opener = stream.symbol_at(start).unwrap();
                let end = u32::from_le_bytes(opener.data[4..8].try_into().unwrap());
                assert_eq!(end, r.offset, "{opener:?}");
            }
            if kind::opens_scope(r.kind) {
                let parent = u32::from_le_bytes(r.data[..4].try_into().unwrap());
                assert_eq!(parent, open.last().copied().unwrap_or(0));
                open.push(r.offset);
            }
            if r.kind == kind::S_GPROC32 {
                procedures += 1;
                // (By type: a procedure's, not an id.)
                let ty = u32::from_le_bytes(r.data[24..28].try_into().unwrap());
                let types = pdb.types().unwrap();
                let rec = types.record(hexcavator_codeview::TypeIndex(ty)).unwrap();
                assert!(matches!(rec.kind, 0x1008 | 0x1009), "{rec:?}");
            }
        }
        assert!(open.is_empty());
        assert_eq!(procedures, 11);
        assert_eq!(pdb.types().unwrap().len(), 75);
        assert_eq!(pdb.ids().unwrap().len(), 75);
        assert!(pdb.find_public("?use_statics@@YAHH@Z").unwrap().is_some());
        assert!(pdb.find_public("??_7Square@@6B@").unwrap().is_some());
        assert!(
            pdb.find_public("?calls@?1??use_statics@@YAHH@Z@4HA")
                .unwrap()
                .is_none()
        );
        assert_eq!(pdb.publics().unwrap().address_map().len(), 25);
        // Lines, and the files they're in.
        let line = pdb.line_at(26, 0).unwrap().unwrap();
        assert_eq!(
            (line.file.as_str(), line.line),
            (r"R:\tools\coff\objects.cpp", 26)
        );
        // (The headers it includes, too: stdint.h and the rest.)
        let files = dbi.module_files(0).unwrap();
        assert_eq!(files.len(), 6);
        assert_eq!(files[0], br"R:\tools\coff\objects.cpp");
        assert!(files[1].ends_with(br"\include\stdint.h"), "{files:?}");
        // The sections where the object's are laid out.
        let pe = PeFile::parse(&object).unwrap();
        let addresses: Vec<u32> = pdb.sections().unwrap().iter().map(|s| s.0).collect();
        let expected: Vec<u32> = pe
            .section_addresses()
            .iter()
            .map(|&a| u32::try_from(a).unwrap())
            .collect();
        assert_eq!(addresses, expected);
        // Every function's address is its COFF symbol's.
        let info = crate::DebugInfo::parse(&data).unwrap();
        let symbols = pe.symbols().unwrap().unwrap();
        let mut functions = 0;
        for s in info.symbols().unwrap() {
            if s.kind != crate::SymbolKind::Public {
                continue;
            }
            let sym = symbols
                .iter()
                .find(|c| symbols.name(c) == Some(s.name.as_bytes()))
                .unwrap();
            let n = usize::try_from(sym.section).unwrap() - 1;
            assert_eq!(
                s.addr,
                u64::from(expected[n]) + u64::from(sym.value),
                "{}",
                s.name
            );
            functions += 1;
        }
        assert_eq!(functions, 25);
    }

    /// A /Zi object's types and ids are its type server's: without it, its
    /// procedures have no types; with it, each is a procedure's.
    #[test]
    fn a_type_server() {
        let object = fixture("coff/objects-zi.obj");
        let (_, _, name) = type_server(&object).unwrap();
        assert_eq!(name, br"R:\testdata\coff\objects-zi.pdb");
        let server = fixture("coff/objects-zi.pdb");
        for (with, typed) in [(None, false), (Some(&server[..]), true)] {
            let data = pdb_of_with(&object, with).unwrap();
            let pdb = hexcavator_pdb::Pdb::parse(&data).unwrap();
            let types = pdb.types().unwrap();
            assert_eq!(!types.is_empty(), typed);
            let dbi = pdb.dbi().unwrap();
            let stream = pdb.module_stream(&dbi.modules()[0]).unwrap().unwrap();
            let mut procedures = 0;
            for r in stream.symbols().filter(|r| r.kind == kind::S_GPROC32) {
                procedures += 1;
                let ty = u32::from_le_bytes(r.data[24..28].try_into().unwrap());
                assert_eq!(ty != 0, typed);
                if typed {
                    let rec = types.record(hexcavator_codeview::TypeIndex(ty)).unwrap();
                    assert!(matches!(rec.kind, 0x1008 | 0x1009), "{rec:?}");
                }
            }
            assert_eq!(procedures, 11);
        }
        // (An object with its types has no type server.)
        assert!(type_server(&fixture("coff/objects.obj")).is_none());
    }

    /// An object without CodeView, or a damaged one, has none (or what of
    /// it can be read).
    #[test]
    fn objects_without() {
        assert!(pdb_of(&fixture("mingw/program.o")).is_none());
        let object = fixture("coff/objects.obj");
        for n in (0..object.len()).step_by(97) {
            let mut d = object.clone();
            d[n] ^= 0x5A;
            if let Some(p) = pdb_of(&d) {
                let _ = crate::DebugInfo::parse(&p).map(|i| {
                    (
                        i.symbols().map(|s| s.len()),
                        i.source_lines().map(|l| l.spans.len()),
                    )
                });
            }
            let _ = pdb_of(&object[..n]);
        }
    }

    #[test]
    fn names_are_found() {
        let strings = b"\0a.c\0dir\\b.h\0";
        let s = names_stream(strings);
        let t = hexcavator_pdb::StringTable::parse(&s).unwrap();
        assert_eq!(t.find(b"a.c"), Some(1));
        assert_eq!(t.find(b"dir\\b.h"), Some(5));
        assert_eq!(t.find(b"c.c"), None);
    }
}
