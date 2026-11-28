//! `/MAP`: the map file, in link.exe's format, line for line: its header
//! (the image's name, time stamp, and preferred load address), the groups
//! of each section ("Start Length Name Class"), the public symbols by
//! address ("Publics by Value", with each one's address relative to the
//! image base and where it came from), the entry point, the static
//! symbols, and with `/MAPINFO:EXPORTS` the exports. Lines end in CRLF.
//!
//! Only the time stamp's date differs from link.exe's: link.exe prints it
//! in local time, turbolink in UTC.

use std::fmt::Write as _;

use super::Image;
use crate::layout::ChunkKind;
use crate::object::{Selection, SymbolKind};
use crate::resolve::{Def, FileKind, IdataPiece, NONE, Synthetic};

/// The absolute symbols link.exe defines in every x64 image (listed whether
/// or not anything refers to them), each 0 unless the link gives it a
/// value.
const ABSOLUTES: &[&[u8]] = &[
    b"__AbsoluteZero",
    b"___safe_se_handler_count",
    b"___safe_se_handler_table",
    b"__arm64x_extra_rfe_table",
    b"__arm64x_extra_rfe_table_size",
    b"__arm64x_native_entrypoint",
    b"__arm64x_redirection_metadata",
    b"__arm64x_redirection_metadata_count",
    b"__dynamic_value_reloc_table",
    b"__enclave_config",
    b"__guard_check_icall_a64n_fptr",
    b"__guard_eh_cont_count",
    b"__guard_eh_cont_table",
    b"__guard_fids_count",
    b"__guard_fids_table",
    b"__guard_flags",
    b"__guard_iat_count",
    b"__guard_iat_table",
    b"__guard_longjmp_count",
    b"__guard_longjmp_table",
    b"__hybrid_auxiliary_delayload_iat",
    b"__hybrid_auxiliary_delayload_iat_copy",
    b"__hybrid_auxiliary_iat",
    b"__hybrid_auxiliary_iat_copy",
    b"__hybrid_code_map",
    b"__hybrid_code_map_count",
    b"__hybrid_image_info_bitfield",
    b"__x64_code_ranges_to_entry_points",
    b"__x64_code_ranges_to_entry_points_count",
];

/// The sections `/GUARD:CF` reads its tables from (and drops), whose
/// static symbols link.exe lists at their values.
const GUARD_INPUTS: &[&[u8]] = &[b".gfids$y", b".giats$y", b".gljmp$y", b".gehcont$y"];

/// One line of the publics or the statics.
struct Entry {
    /// The address (VA), what the lines are sorted by.
    va: u64,
    /// "section:offset" (section 0 for an absolute symbol).
    section: u16,
    offset: u32,
    name: Vec<u8>,
    function: bool,
    inline: bool,
    origin: String,
}

/// What a file is called in a map: `lib:member` for a library's member (the
/// library as the link named it, without its extension; the member without
/// its directories), else the object's name without its directories.
pub(crate) fn origin(name: &str, library: bool) -> String {
    let base = |s: &str| s.rsplit(['\\', '/']).next().unwrap_or(s).to_owned();
    if library
        && let Some(open) = name.rfind('(')
        && let Some(member) = name[open + 1..].strip_suffix(')')
    {
        let lib = std::path::Path::new(&name[..open])
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        return format!("{lib}:{}", base(member));
    }
    base(name)
}

impl Image<'_> {
    /// The map of the image written as `bytes`, named `path`.
    pub fn map(&self, bytes: &[u8], path: &std::path::Path) -> Vec<u8> {
        let r = self.r;
        let l = self.l;
        let mut out = Vec::new();
        let line = |out: &mut Vec<u8>, s: &[u8]| {
            out.extend_from_slice(s);
            out.extend_from_slice(b"\r\n");
        };
        // The header.
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let pe = bytes
            .get(0x3C..0x40)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) as usize;
        let stamp = bytes
            .get(pe + 8..pe + 12)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        line(&mut out, format!(" {stem}").as_bytes());
        line(&mut out, b"");
        line(
            &mut out,
            format!(" Timestamp is {stamp:08x} ({})", ctime(stamp)).as_bytes(),
        );
        line(&mut out, b"");
        line(
            &mut out,
            format!(" Preferred load address is {:016x}", self.base).as_bytes(),
        );
        line(&mut out, b"");
        line(
            &mut out,
            b" Start         Length     Name                   Class",
        );
        // The groups, empty ones too.
        let mut sections: Vec<(u32, u32)> = Vec::new(); // (rva, end) of each listed section
        for (i, s) in l.sections.iter().enumerate() {
            if s.name == b".reloc" {
                continue;
            }
            sections.push((s.rva, s.rva + s.virtual_size));
            let class = if s.characteristics & crate::object::scn::CNT_CODE != 0 {
                "CODE"
            } else {
                "DATA"
            };
            // A group starts at its first chunk; one with no chunks (a
            // table /GUARD's sections leave empty) where what's before it
            // ends. A group with contents runs to the next group with a
            // chunk (its padding with it); an empty one has no length.
            let starts: Vec<Option<u32>> = s
                .groups
                .iter()
                .map(|(_, chunks)| chunks.iter().map(|&c| l.chunks[c as usize].rva).min())
                .collect();
            let ends: Vec<Option<u32>> = s
                .groups
                .iter()
                .map(|(_, chunks)| {
                    chunks
                        .iter()
                        .map(|&c| {
                            let chunk = &l.chunks[c as usize];
                            // (link.exe's room after the bound tables is
                            // 0x103 bytes, and the alignment after it the
                            // 0x104th: an empty group after it starts there.)
                            let short = u32::from(chunk.kind == ChunkKind::DelayBoundRoom);
                            chunk.rva + chunk.size - short
                        })
                        .max()
                })
                .collect();
            let end = s.rva + s.virtual_size;
            for (g, (name, chunks)) in s.groups.iter().enumerate() {
                let next = starts[g + 1..]
                    .iter()
                    .flatten()
                    .next()
                    .copied()
                    .unwrap_or(end);
                let start = starts[g]
                    .unwrap_or_else(|| ends[..g].iter().flatten().max().copied().unwrap_or(s.rva));
                let full = chunks.iter().any(|&c| l.chunks[c as usize].size > 0);
                let length = if full { next.saturating_sub(start) } else { 0 };
                let mut text =
                    format!(" {:04x}:{:08x} {:08x}H ", i + 1, start - s.rva, length).into_bytes();
                text.extend_from_slice(name);
                let pad = 23usize.saturating_sub(name.len());
                text.extend(std::iter::repeat_n(b' ', pad));
                text.push(b' ');
                text.extend_from_slice(class.as_bytes());
                line(&mut out, &text);
            }
        }
        // Where an RVA is: (section number, offset).
        let place = |rva: u32| -> (u16, u32) {
            sections
                .iter()
                .enumerate()
                .find(|&(_, &(lo, hi))| rva >= lo && rva < hi.max(lo + 1))
                .map_or((0, rva), |(i, &(lo, _))| {
                    (u16::try_from(i + 1).unwrap_or(0), rva - lo)
                })
        };
        let at = |rva: u32, name: Vec<u8>, function: bool, inline: bool, origin: String| {
            let (section, offset) = place(rva);
            Entry {
                va: self.base + u64::from(rva),
                section,
                offset,
                name,
                function,
                inline,
                origin,
            }
        };

        // The publics.
        let mut publics: Vec<Entry> = Vec::new();
        let has_feat = r.files.iter().any(|f| match &f.kind {
            FileKind::Object { object, .. } => object.feat_00.is_some(),
            FileKind::Import { .. } => false,
        });
        for &name in ABSOLUTES {
            let target = r.names.get(name).map(|&g| self.global(g));
            if let Some(super::Target::Rva(rva)) = target {
                // (A /GUARD table: where it is.)
                publics.push(at(
                    rva,
                    name.to_vec(),
                    false,
                    false,
                    "<linker-defined>".into(),
                ));
                continue;
            }
            // (__guard_flags that nothing refers to: link.exe still lists
            // it "instrumented", 0x100, when an object has `@feat.00` (a
            // compiler's: cl's 0x80010090 does), else 0.)
            let unreferenced = r
                .names
                .get(name)
                .is_none_or(|&g| r.globals[g as usize].referenced_by == NONE);
            let value = match target {
                Some(super::Target::Absolute(0)) | None
                    if unreferenced
                        && name == b"__guard_flags"
                        && !r.config.guard_cf
                        && has_feat =>
                {
                    0x100
                }
                Some(super::Target::Absolute(v)) => v,
                _ => 0,
            };
            publics.push(Entry {
                va: value,
                section: 0,
                offset: u32::try_from(value).unwrap_or(0),
                name: name.to_vec(),
                function: false,
                inline: false,
                origin: "<absolute>".into(),
            });
        }
        let file_origin = |f: u32| {
            let file = &r.files[f as usize];
            origin(&file.name, file.library.is_some())
        };
        // A chunk's section's "inline" mark: an external "any" COMDAT.
        let inline_chunk = |c: u32| -> bool {
            let Some(chunk) = l.chunks.get(c as usize) else {
                return false;
            };
            let ChunkKind::Input(f, s) = chunk.kind else {
                return false;
            };
            let FileKind::Object { object, .. } = &r.files[f as usize].kind else {
                return false;
            };
            matches!(
                object.sections[s as usize].comdat,
                Some((Selection::Any, _))
            ) && object
                .comdat_leaders
                .get(s as usize)
                .and_then(|&k| object.symbols.get(k as usize))
                .is_some_and(crate::object::Symbol::is_external)
        };
        let chunk_of = |f: u32, s: u32| self.chunk_of(f, s);
        let names_rva = self.names_rva();
        for (g, global) in r.globals.iter().enumerate() {
            let g = crate::n32(g);
            let name = global.name;
            if ABSOLUTES.contains(&name) {
                continue;
            }
            match global.def {
                Def::Section {
                    file,
                    section,
                    value,
                } => {
                    let fi = &r.files[file as usize];
                    // An import library's own pieces: where turbolink's
                    // tables put them.
                    if let Some(piece) = &fi.idata {
                        if let Some(rva) = self.piece_rva(piece, name) {
                            publics.push(at(rva, name.to_vec(), false, false, file_origin(file)));
                        }
                        continue;
                    }
                    let Some(c) = chunk_of(file, section) else {
                        continue;
                    };
                    let FileKind::Object {
                        object, globals, ..
                    } = &fi.kind
                    else {
                        continue;
                    };
                    let function = globals
                        .iter()
                        .position(|&x| x == g)
                        .and_then(|k| object.symbols.get(k))
                        .is_some_and(|s| s.ty & 0x30 == 0x20);
                    let rva = l.chunks[c as usize].rva + value;
                    publics.push(at(
                        rva,
                        name.to_vec(),
                        function,
                        function && inline_chunk(c),
                        file_origin(file),
                    ));
                }
                Def::Alias(_) => {
                    // An alternate name: listed at what it names, as that.
                    let mut t = g;
                    for _ in 0..64 {
                        match r.globals[t as usize].def {
                            Def::Alias(to) => t = to,
                            _ => break,
                        }
                    }
                    if let Def::Section {
                        file,
                        section,
                        value,
                    } = r.globals[t as usize].def
                        && r.files[file as usize].idata.is_none()
                        && let Some(c) = self.chunk_of(file, section)
                        && let FileKind::Object {
                            object, globals, ..
                        } = &r.files[file as usize].kind
                    {
                        let function = globals
                            .iter()
                            .position(|&x| x == t)
                            .and_then(|k| object.symbols.get(k))
                            .is_some_and(|s| s.ty & 0x30 == 0x20);
                        let rva = l.chunks[c as usize].rva + value;
                        publics.push(at(
                            rva,
                            name.to_vec(),
                            function,
                            function && inline_chunk(c),
                            file_origin(file),
                        ));
                    }
                    // (An alias of an import's thunk or address: as that.)
                    match r.globals[t as usize].def {
                        Def::ImportThunk(f) => {
                            if let Some(&c) = l.thunk_chunks.get(&f) {
                                publics.push(at(
                                    l.chunks[c as usize].rva,
                                    name.to_vec(),
                                    true,
                                    false,
                                    file_origin(f),
                                ));
                            }
                        }
                        Def::ImportAddress(f) => {
                            if let Some(rva) = self.import_slot(f) {
                                publics.push(at(rva, name.to_vec(), false, false, file_origin(f)));
                            }
                        }
                        _ => {}
                    }
                }
                Def::Common { .. } => {
                    if let Some(rva) = self.rva_of(g) {
                        publics.push(at(rva, name.to_vec(), false, false, "<common>".into()));
                    }
                }
                Def::ImportAddress(f) => {
                    if let Some(rva) = self.import_slot(f) {
                        publics.push(at(rva, name.to_vec(), false, false, file_origin(f)));
                    }
                    // A delay-loaded import's thunk, `__imp_load_x`.
                    if l.delay_slots.contains_key(&f)
                        && let Some(rest) = name.strip_prefix(b"__imp_")
                        && let Some(rva) = self.delay_rva(ChunkKind::DelayThunk(f))
                    {
                        let mut n = b"__imp_load_".to_vec();
                        n.extend_from_slice(rest);
                        publics.push(at(rva, n, true, false, file_origin(f)));
                    }
                }
                Def::ImportThunk(f) => {
                    if let Some(&c) = l.thunk_chunks.get(&f) {
                        publics.push(at(
                            l.chunks[c as usize].rva,
                            name.to_vec(),
                            true,
                            false,
                            file_origin(f),
                        ));
                    }
                }
                Def::Absolute(v) => {
                    // An object's absolute symbol.
                    publics.push(Entry {
                        va: v,
                        section: 0,
                        offset: u32::try_from(v).unwrap_or(0),
                        name: name.to_vec(),
                        function: false,
                        inline: false,
                        origin: "<absolute>".into(),
                    });
                }
                Def::Synthetic(Synthetic::ImageBase) => {
                    // (Only __ImageBase itself: not a function override's
                    // name, `f_$fo$`, which link.exe doesn't list.)
                    if global.referenced_by != NONE && name == b"__ImageBase" {
                        publics.push(Entry {
                            va: self.base,
                            section: 0,
                            offset: 0,
                            name: name.to_vec(),
                            function: false,
                            inline: false,
                            origin: "<linker-defined>".into(),
                        });
                    }
                }
                Def::Synthetic(Synthetic::VolatileMetadata) => {
                    if let Some(rva) = self.rva_of(g) {
                        publics.push(at(
                            rva,
                            name.to_vec(),
                            false,
                            false,
                            "<linker-defined>".into(),
                        ));
                    }
                }
                _ => {}
            }
        }
        let _ = names_rva;
        // The delay-load tables' symbols, as link.exe names what it builds
        // for each delay-loaded DLL (from its first import's library).
        for (d, dll) in l.delay_dlls.iter().enumerate() {
            let d = crate::n32(d);
            let Some(&first) = dll.imports.first() else {
                continue;
            };
            let from = file_origin(first);
            let underscored: Vec<u8> = dll
                .name
                .iter()
                .map(|&b| if b == b'.' { b'_' } else { b })
                .collect();
            let lower = underscored.to_ascii_lowercase();
            let stem = crate::resolve::dll_stem(dll.name);
            let named = |prefix: &[u8], rest: &[u8]| [prefix, rest].concat();
            let n = 8 * crate::n32(dll.imports.len());
            let mut put = |kind: ChunkKind, plus: u32, name: Vec<u8>| {
                if let Some(rva) = self.delay_rva(kind) {
                    publics.push(at(rva + plus, name, false, false, from.clone()));
                }
            };
            put(ChunkKind::TailMerge(d), 0, named(b"__tailMerge_", &lower));
            put(ChunkKind::DelayDllName(d), 0, named(b"__sz_", &underscored));
            put(
                ChunkKind::DelayHandle(d),
                0,
                named(b"__hmod__", &underscored),
            );
            put(
                ChunkKind::DelayDirectory,
                32 * d,
                named(b"__DELAY_IMPORT_DESCRIPTOR_", &lower),
            );
            if d == 0 {
                put(
                    ChunkKind::DelayNullDescriptor,
                    0,
                    b"__NULL_DELAY_IMPORT_DESCRIPTOR".to_vec(),
                );
            }
            let thunk = |suffix: &[u8]| [b"\x7f", stem, b"_NULL_THUNK_DATA_", suffix].concat();
            put(ChunkKind::DelayNames(d), n, thunk(b"DLN"));
            put(ChunkKind::DelayBound(d), n, thunk(b"DLB"));
            put(ChunkKind::DelayAddresses(d), n, thunk(b"DLA"));
            put(ChunkKind::DelayUnload(d), n, thunk(b"DLU"));
        }
        // (link.exe lists __volatile_metadata whenever it writes it.)
        if !r.names.contains_key(&b"__volatile_metadata"[..])
            && let Some(c) = l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::VolatileMetadata)
        {
            publics.push(at(
                c.rva,
                b"__volatile_metadata".to_vec(),
                false,
                false,
                "<linker-defined>".into(),
            ));
        }
        sort(&mut publics);
        line(&mut out, b"");
        line(
            &mut out,
            b"  Address         Publics by Value              Rva+Base               Lib:Object",
        );
        line(&mut out, b"");
        write_entries(&mut out, &publics);

        // The entry point.
        let (es, eo) = r.entry.and_then(|g| self.rva_of(g)).map_or((0, 0), place);
        line(&mut out, b"");
        line(
            &mut out,
            format!(" entry point at        {es:04x}:{eo:08x}").as_bytes(),
        );
        line(&mut out, b"");

        // The statics: every object's static symbols (not its sections' or
        // labels), where they are in the image.
        let files: Vec<u32> = (0..crate::n32(r.files.len())).collect();
        let statics = turbonop_par::map(&files, 16, |&f| {
            let file = &r.files[f as usize];
            let mut mine = Vec::new();
            let FileKind::Object { object, .. } = &file.kind else {
                return mine;
            };
            for sym in &object.symbols {
                let SymbolKind::Local { section, value } = sym.kind else {
                    continue;
                };
                // (Not a section's, a label, or .pdata's: link.exe lists none
                // of those.)
                // (Nor one in an empty section: MASM's `$xdatasym` in the
                // empty `.xdata` it starts with.)
                if sym.class != 3
                    || sym.aux != 0
                    || object
                        .sections
                        .get(section as usize)
                        .is_some_and(|x| x.name == b".pdata" || x.size == 0)
                {
                    continue;
                }
                let sec_name = object
                    .sections
                    .get(section as usize)
                    .map_or(&[][..], |x| x.name);
                if r.config.guard_cf
                    && chunk_of(f, section).is_none()
                    && GUARD_INPUTS.contains(&sec_name)
                    && let Some((i, s)) = l
                        .sections
                        .iter()
                        .enumerate()
                        .find(|(_, s)| s.groups.iter().any(|(g, _)| g == sec_name))
                {
                    mine.push((
                        f,
                        Entry {
                            va: self.base + u64::from(value),
                            section: u16::try_from(i + 1).unwrap_or(0),
                            offset: value.wrapping_sub(s.rva),
                            name: sym.name.to_vec(),
                            function: false,
                            inline: false,
                            origin: origin(&file.name, file.library.is_some()),
                        },
                    ));
                    continue;
                }
                let rva = if let Some(piece) = &file.idata {
                    // The DLL's name, in the names table.
                    match (piece, sym.name) {
                        (IdataPiece::Descriptor(_), b".idata$6") => self.dll_name_rva(piece),
                        _ => None,
                    }
                } else {
                    chunk_of(f, section).map(|c| l.chunks[c as usize].rva + value)
                };
                if let Some(rva) = rva {
                    let c = chunk_of(f, section).unwrap_or(u32::MAX);
                    let function = sym.ty & 0x30 == 0x20;
                    mine.push((
                        f,
                        at(
                            rva,
                            sym.name.to_vec(),
                            function,
                            function && inline_chunk(c),
                            origin(&file.name, file.library.is_some()),
                        ),
                    ));
                }
            }
            mine
        });
        let mut statics: Vec<(u32, Entry)> = statics.into_iter().flatten().collect();
        // The resources' data, as cvtres names it (`$R000000`), from each
        // `.res` file.
        if let Some(res) = &l.resources
            && let Some(c) = l.chunks.iter().find(|c| c.kind == ChunkKind::Resources)
        {
            for (name, offset, source) in res.symbols() {
                // (Not the embedded manifest's: no `.res` file's.)
                if source == u32::MAX {
                    continue;
                }
                let from = r
                    .resources
                    .get(source as usize)
                    .map_or_else(String::new, |(n, _)| origin(n, false));
                statics.push((
                    u32::MAX,
                    at(c.rva + offset, name.into_bytes(), false, false, from),
                ));
            }
        }
        // Each delay-loaded DLL's tail merge's unwind data, `xdata`.
        for (d, dll) in l.delay_dlls.iter().enumerate() {
            if let Some(&first) = dll.imports.first()
                && let Some(rva) = self.delay_rva(ChunkKind::TailMergeUnwind(crate::n32(d)))
            {
                statics.push((
                    first,
                    at(rva, b"xdata".to_vec(), false, false, file_origin(first)),
                ));
            }
        }
        // The export object's (link.exe links its .exp into the image).
        if let Some(t) = &l.exports
            && let Some(c) = l.chunks.iter().find(|c| c.kind == ChunkKind::Exports)
        {
            let exp = r
                .config
                .implib
                .clone()
                .unwrap_or_else(|| path.to_path_buf())
                .with_extension("exp");
            let exp = origin(&exp.display().to_string(), false);
            for (name, offset) in t.directory.symbols(&t.dll_name) {
                statics.push((
                    u32::MAX,
                    at(c.rva + offset, name.into_bytes(), false, false, exp.clone()),
                ));
            }
        }
        // (At one address: the command line's objects, then libraries'
        // members, each by its whole name as the link has it (a library's
        // path, then the member's), then by name.)
        let key = |f: u32| -> (u8, &[u8]) {
            r.files.get(f as usize).map_or((2, &[][..]), |file| {
                (u8::from(file.library.is_some()), file.name.as_bytes())
            })
        };
        turbonop_par::sort_by(&mut statics, |(fa, a), (fb, b)| {
            (a.va, key(*fa), &a.name).cmp(&(b.va, key(*fb), &b.name))
        });
        let statics: Vec<Entry> = statics.into_iter().map(|(_, e)| e).collect();
        line(&mut out, b" Static symbols");
        line(&mut out, b"");
        write_entries(&mut out, &statics);

        // The exports, with /MAPINFO:EXPORTS.
        if r.config.map_exports
            && let Some(t) = &l.exports
        {
            line(&mut out, b"");
            line(&mut out, b" Exports");
            line(&mut out, b"");
            line(&mut out, b"  ordinal    name");
            line(&mut out, b"");
            let mut by_ordinal: Vec<&crate::config::Export> = t.directory.exports.iter().collect();
            by_ordinal.sort_by_key(|e| e.ordinal);
            for e in by_ordinal {
                let ordinal = e.ordinal.unwrap_or(0);
                let exported = crate::exports::exported_name(e);
                let internal = e.internal.as_deref().unwrap_or(exported);
                line(&mut out, format!("{ordinal:>9}    {internal}").as_bytes());
                if internal != exported {
                    line(
                        &mut out,
                        format!("               exported name: {exported}").as_bytes(),
                    );
                }
            }
        }
        out
    }
}

/// Publics and statics in link.exe's order: by address, then name.
fn sort(v: &mut Vec<Entry>) {
    turbonop_par::sort_by(v, |a, b| (a.va, &a.name).cmp(&(b.va, &b.name)));
}

/// The lines of `entries`: " 0001:00000000       name  0000000140001000 f i origin".
fn write_entries(out: &mut Vec<u8>, entries: &[Entry]) {
    // (Formatted on every core: an LLVM link's map has a million lines.)
    let parts = turbonop_par::map_chunks(entries, 4096, |part| {
        let mut text = Vec::new();
        for e in part {
            let _ = write!(
                Bytes(&mut text),
                " {:04x}:{:08x}       ",
                e.section,
                e.offset
            );
            let name = escaped(&e.name);
            text.extend_from_slice(&name);
            text.extend(std::iter::repeat_n(
                b' ',
                26usize.saturating_sub(name.len()),
            ));
            let _ = write!(
                Bytes(&mut text),
                " {:016x} {} {} {}\r\n",
                e.va,
                if e.function { 'f' } else { ' ' },
                if e.inline { 'i' } else { ' ' },
                e.origin
            );
        }
        vec![text]
    });
    for p in parts {
        out.extend_from_slice(&p);
    }
}

/// A name as link.exe prints it: a control character (the null thunks'
/// 0x7F) as a backslash and three octal digits.
fn escaped(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    for &b in name {
        if b < 0x20 || b == 0x7F {
            out.extend_from_slice(format!("\\{b:03o}").as_bytes());
        } else {
            out.push(b);
        }
    }
    out
}

/// A `fmt::Write` into bytes.
struct Bytes<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for Bytes<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// `t` (seconds since 1970) as C's `ctime` writes it, in UTC: "Fri Nov 27
/// 14:13:35 2026".
fn ctime(t: u32) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = t / 86400;
    let secs = t % 86400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = i64::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let weekday = DAYS[(days % 7) as usize];
    let month_name = MONTHS[usize::try_from(month - 1).unwrap_or(0)];
    format!(
        "{weekday} {month_name} {day:>2} {:02}:{:02}:{:02} {year}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

impl Image<'_> {
    /// A section's chunk: its own, or (folded by `/OPT:ICF`) its leader's,
    /// or for the unwind data of a folded function its leader's.
    fn chunk_of(&self, f: u32, s: u32) -> Option<u32> {
        let own = |f: u32, s: u32| {
            let c = *self.l.section_chunks.get(f as usize)?.get(s as usize)?;
            (c != u32::MAX).then_some(c)
        };
        if let Some(c) = own(f, s) {
            return Some(c);
        }
        if let Some(&(lf, ls)) = self.l.folded.get(&(f, s)) {
            return own(lf, ls);
        }
        // An associative section of a folded function: the leader's
        // associative section of the same name, in the same place.
        let FileKind::Object { object, .. } = &self.r.files[f as usize].kind else {
            return None;
        };
        let sec = object.sections.get(s as usize)?;
        let Some((Selection::Associative(parent), _)) = sec.comdat else {
            return None;
        };
        let &(lf, lp) = self.l.folded.get(&(f, parent))?;
        let kids = |file: u32, parent: u32, name: &[u8]| -> Vec<u32> {
            match &self.r.files[file as usize].kind {
                FileKind::Object { object, .. } => (0..crate::n32(object.sections.len()))
                    .filter(|&k| {
                        let x = &object.sections[k as usize];
                        x.name == name
                            && matches!(x.comdat, Some((Selection::Associative(p), _)) if p == parent)
                    })
                    .collect(),
                FileKind::Import { .. } => Vec::new(),
            }
        };
        let mine = kids(f, parent, sec.name);
        let at = mine.iter().position(|&k| k == s)?;
        let theirs = kids(lf, lp, sec.name);
        own(lf, *theirs.get(at)?)
    }

    /// The DLL (its index among the import tables' DLLs) whose name's stem
    /// is `stem`.
    fn dll_by_stem(&self, stem: &[u8]) -> Option<usize> {
        self.l
            .dlls
            .iter()
            .position(|d| crate::resolve::dll_stem(d.name) == stem)
    }

    /// Where an import library's piece's symbol `name` is in the tables
    /// turbolink builds: a DLL's import descriptor (its directory entry),
    /// the null import descriptor, a DLL's null thunk (after its IAT).
    fn piece_rva(&self, piece: &IdataPiece, name: &[u8]) -> Option<u32> {
        let chunk = |k: ChunkKind| self.l.chunks.iter().find(|c| c.kind == k);
        match piece {
            IdataPiece::Descriptor(stem) if name.starts_with(b"__IMPORT_DESCRIPTOR_") => {
                let d = self.dll_by_stem(stem)?;
                let slot = self
                    .l
                    .directory_order
                    .iter()
                    .position(|&x| x as usize == d)?;
                Some(chunk(ChunkKind::ImportDirectory)?.rva + 20 * crate::n32(slot))
            }
            IdataPiece::NullDescriptor => Some(chunk(ChunkKind::NullImportDescriptor)?.rva),
            IdataPiece::NullThunk(stem) => {
                let d = self.dll_by_stem(stem)?;
                let iat = chunk(ChunkKind::ImportAddresses(crate::n32(d)))?;
                Some(iat.rva + 8 * crate::n32(self.l.dlls[d].imports.len()))
            }
            IdataPiece::Descriptor(_) => None,
        }
    }

    /// Where a DLL's name is (its import descriptor's `.idata$6`).
    fn dll_name_rva(&self, piece: &IdataPiece) -> Option<u32> {
        let IdataPiece::Descriptor(stem) = piece else {
            return None;
        };
        let d = self.dll_by_stem(stem)?;
        Some(self.names_rva() + *self.l.dll_name_offsets.get(d)?)
    }
}
