//! What one archive member holds, as folding sees it.
//!
//! A COFF object's **fold units** are its COMDAT leaders' sections, each
//! with the associative sections that follow it (transitively): its
//! unwind data, its `.debug$S`, its dynamic initializer. A unit's
//! **identity** hashes what the linker would keep of it: every non-debug
//! section's name, characteristics, and bytes; its relocations, with
//! targets outside the unit named by symbol name; and the symbols it
//! defines. Debug sections are not part of the identity: whichever copy
//! survives brings its own, as the linker's choice does.
//!
//! The member's **environment** (machine, `@feat.00`, `/FAILIFMISMATCH`
//! directives) bounds which copies may stand for each other.
//!
//! Its **types** (`.debug$T`) are hashed structurally: each record's hash
//! covers its bytes with every index field replaced by the hash of the
//! record it names, so equal types in different objects hash alike
//! whatever their indices.

use crate::hash::{H128, h64};
use turbonop_codeview::type_refs;
use turbonop_codeview::types::leaf;
use turbonop_pe::consts::section_characteristics as scn;
use turbonop_pe::consts::storage_class;
use turbonop_pe::import_object::ImportObject;
use turbonop_pe::ltcg::LtcgObject;
use turbonop_pe::raw::SectionHeader;
use turbonop_pe::{PeFile, SectionDefinition, comdat_select};

/// What bytes are, by the sections that hold them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Category {
    /// `.text*`.
    Code,
    /// `.pdata`, `.xdata`.
    Unwind,
    /// Everything else the image keeps: `.rdata`, `.data`, `.CRT$*`, ...
    Data,
    /// `.debug$S`.
    DebugSymbols,
    /// `.debug$T` and `.debug$P`.
    DebugTypes,
    /// Other `.debug$*` sections (`.debug$H`, `.debug$F`).
    DebugOther,
    /// Symbol records and names (not attributed to a section).
    Symbols,
    /// Headers, directives, and what no section claims.
    Other,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Code,
        Category::Unwind,
        Category::Data,
        Category::DebugSymbols,
        Category::DebugTypes,
        Category::DebugOther,
        Category::Symbols,
        Category::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Code => "code",
            Category::Unwind => "unwind",
            Category::Data => "data",
            Category::DebugSymbols => ".debug$S",
            Category::DebugTypes => ".debug$T",
            Category::DebugOther => ".debug$ other",
            Category::Symbols => "symbols",
            Category::Other => "other",
        }
    }

    fn of_section(name: &[u8]) -> Category {
        if name.starts_with(b".text") {
            Category::Code
        } else if name.starts_with(b".pdata") || name.starts_with(b".xdata") {
            Category::Unwind
        } else if name == b".debug$S" {
            Category::DebugSymbols
        } else if name == b".debug$T" || name == b".debug$P" {
            Category::DebugTypes
        } else if name.starts_with(b".debug$") {
            Category::DebugOther
        } else if name == b".drectve" {
            Category::Other
        } else {
            Category::Data
        }
    }
}

/// Bytes by category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub [u64; 8]);

impl Bytes {
    pub fn add(&mut self, c: Category, n: u64) {
        self.0[c as usize] += n;
    }

    pub fn get(&self, c: Category) -> u64 {
        self.0[c as usize]
    }

    pub fn total(&self) -> u64 {
        self.0.iter().sum()
    }

    pub fn add_all(&mut self, other: &Bytes) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }

    pub fn sub_all(&mut self, other: &Bytes) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a = a.saturating_sub(b);
        }
    }
}

/// A COMDAT and everything that moves with it.
#[derive(Clone, Debug)]
pub struct Unit {
    /// The leader's (COMDAT symbol's) name.
    pub leader: Vec<u8>,
    /// A hash of the leader's name, for sharding.
    pub leader_hash: u64,
    /// `IMAGE_COMDAT_SELECT_*`.
    pub selection: u8,
    /// What the linker would keep of it (see the module's documentation).
    pub identity: u128,
    /// The leader section's size and checksum (for `SAME_SIZE` and
    /// `EXACT_MATCH`).
    pub size: u32,
    pub checksum: u32,
    /// Its relocations (outside debug sections) reach a static symbol
    /// defined outside the unit: internal-linkage data, a static helper.
    pub reaches_locals: bool,
    /// The bytes it occupies in the member, by category.
    pub bytes: Bytes,
}

/// A type or id record, hashed structurally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hashed {
    pub hash: u128,
    /// Its bytes, length field included.
    pub size: u32,
    /// Whether a precompiled-types member may hold it. `LF_FUNC_ID` and
    /// `LF_MFUNC_ID` may not: `link.exe` drops the types of the functions
    /// they name when they come from a PCH (lld takes them). Nothing in the
    /// type graph refers to them, so the rest stays closed under
    /// references without them.
    pub shareable: bool,
}

/// What a member's `.debug$T` holds.
#[derive(Clone, Debug, Default)]
pub enum Types {
    /// No `.debug$T`.
    #[default]
    None,
    /// `/Zi`: a reference to a type server (a PDB).
    TypeServer,
    /// Its types, hashed: one for each record, in index order. `precomp` says it begins with `LF_PRECOMP`; `pch` that it is
    /// a PCH object (`.debug$P`).
    Records {
        records: Vec<Hashed>,
        precomp: bool,
        pch: bool,
        /// Records that name an index not before their own.
        forward: u32,
    },
    /// Its records could not all be read.
    Unreadable(String),
}

/// What a member is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Coff,
    Import,
    /// `/GL` (LTCG) objects: passed through.
    Ltcg,
    /// Anything else, and why.
    Unrecognized(String),
}

/// What one member holds.
#[derive(Clone, Debug)]
pub struct Member {
    pub kind: Kind,
    pub size: u64,
    pub machine: u16,
    /// The environment key: machine, `@feat.00`, `/FAILIFMISMATCH`.
    pub environment: u64,
    /// The whole member's bytes by category.
    pub bytes: Bytes,
    pub units: Vec<Unit>,
    pub types: Types,
    /// It has non-COMDAT dynamic initializers, terminators, or TLS
    /// callbacks, or `/INCLUDE` directives: loading it does something.
    pub side_effects: bool,
    /// Section names no rule here knows (for `--diagnose`).
    pub unknown_sections: Vec<Vec<u8>>,
}

impl Member {
    fn other(kind: Kind, size: u64) -> Self {
        let mut bytes = Bytes::default();
        bytes.add(Category::Other, size);
        Member {
            kind,
            size,
            machine: 0,
            environment: 0,
            bytes,
            units: Vec::new(),
            types: Types::None,
            side_effects: false,
            unknown_sections: Vec::new(),
        }
    }
}

/// The section names turbolib knows what to do with (by prefix).
const KNOWN_SECTIONS: &[&[u8]] = &[
    b".text",
    b".data",
    b".rdata",
    b".bss",
    b".pdata",
    b".xdata",
    b".debug$S",
    b".debug$T",
    b".debug$P",
    b".debug$H",
    b".debug$F",
    b".drectve",
    b".CRT$",
    b".tls",
    b".gfids$y",
    b".giats$y",
    b".gljmp$y",
    b".gehcont$y",
    b".sxdata",
    b".voltbl",
    b".chks64",
    b".msvcjmc",
    b".rtc$",
    b".00cfg",
    b".retplne",
];

fn known(name: &[u8]) -> bool {
    KNOWN_SECTIONS.iter().any(|p| name.starts_with(p))
}

const WEAK_EXTERNAL: u8 = 105;
/// CodeView's signature at the start of `.debug$S` and `.debug$T`.
const CV_SIGNATURE_C13: u32 = 4;

/// Reads what the member `data` holds.
pub fn read(data: &[u8]) -> Member {
    let size = data.len() as u64;
    if ImportObject::is_import_object(data) {
        return Member::other(Kind::Import, size);
    }
    if LtcgObject::is_ltcg(data) {
        return Member::other(Kind::Ltcg, size);
    }
    match PeFile::parse_object(data) {
        Ok(pe) => coff(&pe, size),
        Err(e) => Member::other(Kind::Unrecognized(e.to_string()), size),
    }
}

fn section_data<'d>(pe: &PeFile<'d>, s: &SectionHeader) -> &'d [u8] {
    if s.characteristics.get() & scn::CNT_UNINITIALIZED_DATA != 0 {
        return &[];
    }
    let at = s.pointer_to_raw_data.get() as usize;
    let len = s.size_of_raw_data.get() as usize;
    at.checked_add(len)
        .and_then(|end| pe.data().get(at..end))
        .unwrap_or(&[])
}

fn coff(pe: &PeFile<'_>, size: u64) -> Member {
    let sections = pe.sections();
    let n = sections.len();
    let names: Vec<&[u8]> = sections.iter().map(|s| pe.section_name(s)).collect();
    let mut member = Member::other(Kind::Coff, 0);
    member.size = size;
    member.machine = pe.machine();
    member.bytes = Bytes::default();

    let symbols = match pe.symbols() {
        Ok(Some(t)) => t,
        Ok(None) => turbonop_pe::SymbolTable::default(),
        Err(e) => return Member::other(Kind::Unrecognized(e.to_string()), size),
    };
    let stride = symbols.record_size() as u64;

    // Each section's definition (the first section symbol naming it) and
    // COMDAT leader.
    let mut definitions: Vec<Option<SectionDefinition>> = vec![None; n + 1];
    let leaders = symbols.comdat_symbols(n);
    let mut feat00 = None;
    for sym in symbols.iter() {
        if let Ok(s) = usize::try_from(sym.section)
            && (1..=n).contains(&s)
            && definitions[s].is_none()
            && let Some(d) = sym.section_definition()
        {
            definitions[s] = Some(d);
        }
        if sym.section == -1 && symbols.name(&sym) == Some(b"@feat.00") {
            feat00 = Some(sym.value);
        }
    }

    // Each COMDAT section's unit root: follow associations to a COMDAT
    // that isn't associative. A section associated with a non-COMDAT
    // section (or a broken chain) belongs to no unit.
    let comdat = |s: usize| sections[s - 1].characteristics.get() & scn::LNK_COMDAT != 0;
    let mut root: Vec<Option<usize>> = vec![None; n + 1];
    for (s, slot) in root.iter_mut().enumerate().skip(1) {
        if !comdat(s) {
            continue;
        }
        let mut at = s;
        let mut steps = 0;
        let found = loop {
            let Some(d) = definitions[at] else { break None };
            if d.selection != comdat_select::ASSOCIATIVE {
                break Some(at);
            }
            let next = d.number as usize;
            steps += 1;
            if !(1..=n).contains(&next) || !comdat(next) || steps > n {
                break None;
            }
            at = next;
        };
        *slot = found;
    }
    // Units, by root, with their sections in order (the root first).
    let mut unit_of: Vec<Option<usize>> = vec![None; n + 1];
    let mut unit_sections: Vec<Vec<usize>> = Vec::new();
    for s in 1..=n {
        if root[s] == Some(s) && leaders[s].is_some() {
            unit_of[s] = Some(unit_sections.len());
            unit_sections.push(vec![s]);
        }
    }
    for s in 1..=n {
        if let Some(r) = root[s]
            && r != s
            && let Some(u) = unit_of[r]
        {
            unit_of[s] = Some(u);
            unit_sections[u].push(s);
        }
    }
    // Each section's position within its unit.
    let mut position = vec![0u32; n + 1];
    for list in &unit_sections {
        for (i, &s) in list.iter().enumerate() {
            position[s] = i as u32;
        }
    }

    // Bytes: the file header, then each section's header, data, and
    // relocations, then symbols (by the section they define), then the
    // string table.
    let mut unit_bytes = vec![Bytes::default(); unit_sections.len()];
    let header = if pe.is_bigobj() { 56 } else { 20 };
    member.bytes.add(Category::Other, header);
    for s in 1..=n {
        let h = &sections[s - 1];
        let c = Category::of_section(names[s - 1]);
        let mut b = 40
            + u64::from(h.size_of_raw_data.get())
                * u64::from(h.characteristics.get() & scn::CNT_UNINITIALIZED_DATA == 0);
        b += 10 * pe.section_relocations(h).len() as u64;
        if names[s - 1].len() > 8 {
            b += names[s - 1].len() as u64 + 1;
        }
        member.bytes.add(c, b);
        if let Some(u) = unit_of[s] {
            unit_bytes[u].add(c, b);
        }
        if !known(names[s - 1]) {
            member.unknown_sections.push(names[s - 1].to_vec());
        }
    }
    for sym in symbols.iter() {
        let mut b = stride * (1 + sym.aux_count() as u64);
        if sym.raw_name.starts_with(&[0, 0, 0, 0])
            && let Some(name) = symbols.name(&sym)
        {
            b += name.len() as u64 + 1;
        }
        member.bytes.add(Category::Symbols, b);
        if let Ok(s) = usize::try_from(sym.section)
            && (1..=n).contains(&s)
            && let Some(u) = unit_of[s]
        {
            unit_bytes[u].add(Category::Symbols, b);
        }
    }
    // The string table's size field, padding, and anything unaccounted.
    let counted = member.bytes.total();
    member
        .bytes
        .add(Category::Other, size.saturating_sub(counted));

    // Directives and side effects.
    let mut mismatches: Vec<&[u8]> = Vec::new();
    for s in 1..=n {
        let name = names[s - 1];
        let h = &sections[s - 1];
        if name == b".drectve" {
            for d in directives(section_data(pe, h)) {
                let lower = d.to_ascii_lowercase();
                if lower.starts_with(b"/failifmismatch:") || lower.starts_with(b"-failifmismatch:")
                {
                    mismatches.push(d);
                } else if (lower.starts_with(b"/include:") || lower.starts_with(b"-include:"))
                    // A PCH user's reference to its PCH object, which
                    // brings in the PCH object's types, not code of its own.
                    && !lower[9..].starts_with(b"__@@_pchsym_")
                {
                    member.side_effects = true;
                }
            }
        } else if !comdat(s)
            && (name.starts_with(b".CRT$XC")
                || name.starts_with(b".CRT$XI")
                || name.starts_with(b".CRT$XL")
                || name.starts_with(b".CRT$XP")
                || name.starts_with(b".CRT$XT"))
            && h.size_of_raw_data.get() > 0
        {
            member.side_effects = true;
        }
    }
    mismatches.sort_unstable();
    let mut env = H128::new();
    env.u64(u64::from(member.machine));
    env.u64(feat00.map_or(u64::MAX, u64::from));
    for m in &mismatches {
        env.bytes(m);
    }
    member.environment = env.finish() as u64;

    // Each unit's identity.
    // Each unit's symbols: (section position, value, storage class, name
    // if external).
    type Defined<'d> = (u32, u32, u8, Option<&'d [u8]>);
    let mut defined: Vec<Vec<Defined<'_>>> = vec![Vec::new(); unit_sections.len()];
    for sym in symbols.iter() {
        let Ok(s) = usize::try_from(sym.section) else {
            continue;
        };
        if !(1..=n).contains(&s) || sym.section_definition().is_some() {
            continue;
        }
        if let Some(u) = unit_of[s] {
            let name = (sym.storage_class == storage_class::EXTERNAL)
                .then(|| symbols.name(&sym))
                .flatten();
            defined[u].push((position[s], sym.value, sym.storage_class, name));
        }
    }
    for (u, list) in unit_sections.iter().enumerate() {
        let r = list[0];
        let Some(leader) = leaders[r] else { continue };
        let leader_name = symbols.name(&leader).unwrap_or(&[]).to_vec();
        let d = definitions[r].unwrap_or(SectionDefinition {
            length: 0,
            relocations: 0,
            line_numbers: 0,
            checksum: 0,
            number: 0,
            selection: 0,
        });
        let mut h = H128::new();
        let mut reaches_locals = false;
        h.u64(u64::from(d.selection));
        for &s in list {
            let name = names[s - 1];
            if name.starts_with(b".debug$") {
                continue;
            }
            let sh = &sections[s - 1];
            h.bytes(name);
            h.u64(u64::from(sh.characteristics.get()));
            h.u64(u64::from(sh.size_of_raw_data.get()));
            h.bytes(section_data(pe, sh));
            for rel in pe.section_relocations(sh) {
                h.u64(u64::from(rel.virtual_address.get()));
                h.u64(u64::from(rel.kind.get()));
                match symbols.get(rel.symbol_table_index.get() as usize) {
                    None => h.u64(u64::MAX),
                    Some(t) => {
                        let ts = usize::try_from(t.section)
                            .ok()
                            .filter(|s| (1..=n).contains(s));
                        if let Some(ts) = ts
                            && unit_of[ts] == Some(u)
                        {
                            h.u64(1);
                            h.u64(u64::from(position[ts]));
                            h.u64(u64::from(t.value));
                            h.u64(u64::from(t.storage_class));
                        } else if t.storage_class == storage_class::EXTERNAL
                            || t.storage_class == WEAK_EXTERNAL
                        {
                            h.u64(2);
                            h.bytes(symbols.name(&t).unwrap_or(&[]));
                        } else {
                            reaches_locals = true;
                            h.u64(3);
                            h.bytes(symbols.name(&t).unwrap_or(&[]));
                            h.u64(u64::from(t.value));
                        }
                    }
                }
            }
        }
        for &(p, value, class, name) in &defined[u] {
            h.u64(u64::from(p));
            h.u64(u64::from(value));
            h.u64(u64::from(class));
            if let Some(name) = name {
                h.bytes(name);
            }
        }
        member.units.push(Unit {
            leader_hash: h64(&leader_name),
            leader: leader_name,
            selection: d.selection,
            identity: h.finish(),
            size: sections[r - 1].size_of_raw_data.get(),
            checksum: d.checksum,
            reaches_locals,
            bytes: unit_bytes[u],
        });
    }

    // Types.
    let find = |want: &[u8]| (1..=n).find(|&s| names[s - 1] == want && !comdat(s));
    let pch = find(b".debug$P");
    if let Some(s) = pch.or_else(|| find(b".debug$T")) {
        member.types = types(section_data(pe, &sections[s - 1]), pch.is_some());
    }
    member
}

/// The directives of a `.drectve` section: space-separated, each may be
/// quoted in part.
fn directives(data: &[u8]) -> Vec<&[u8]> {
    // A UTF-8 BOM may start it.
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        while i < data.len() && (data[i] == b' ' || data[i] == 0) {
            i += 1;
        }
        let start = i;
        let mut quoted = false;
        while i < data.len() && (quoted || (data[i] != b' ' && data[i] != 0)) {
            if data[i] == b'"' {
                quoted = !quoted;
            }
            i += 1;
        }
        if i > start {
            out.push(&data[start..i]);
        }
    }
    out
}

/// Hashes a `.debug$T` (or `.debug$P`) section's records structurally.
pub fn types(data: &[u8], pch: bool) -> Types {
    const FIRST: u32 = 0x1000;
    if data.len() < 4
        || u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != CV_SIGNATURE_C13
    {
        return Types::Unreadable("no CodeView C13 signature".into());
    }
    let mut at = 4;
    let mut records: Vec<Hashed> = Vec::new();
    // The PCH range an LF_PRECOMP names, and its signature.
    let mut precomp: Option<(u32, u32, u32)> = None;
    let mut base = FIRST;
    let mut forward = 0;
    let mut refs = Vec::new();
    while at + 4 <= data.len() {
        let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
        let kind = u16::from_le_bytes([data[at + 2], data[at + 3]]);
        let Some(body) = data.get(at + 4..at + 2 + len).filter(|_| len >= 2) else {
            return Types::Unreadable(format!("record at {at:#x} overruns the section"));
        };
        if records.is_empty() && precomp.is_none() {
            if kind == leaf::LF_TYPESERVER2 {
                return Types::TypeServer;
            }
            if kind == leaf::LF_PRECOMP && body.len() >= 12 {
                let u =
                    |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
                let (start, count, signature) = (u(0), u(4), u(8));
                precomp = Some((start, count, signature));
                base = start.saturating_add(count);
                at += 2 + len;
                continue;
            }
        }
        refs.clear();
        if let Err(e) = type_refs::refs(kind, body, &mut refs) {
            return Types::Unreadable(format!("record {:#x}: {e}", base as usize + records.len()));
        }
        let index = base as usize + records.len();
        let mut h = H128::new();
        h.u64(u64::from(kind));
        let mut from = 0;
        for r in &refs {
            let o = r.offset as usize;
            h.bytes(&body[from..o]);
            let v = u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
            if v < FIRST {
                h.u64(u64::from(v));
            } else if let Some((start, count, signature)) = precomp
                && v >= start
                && v - start < count
            {
                h.u64(2);
                h.u64(u64::from(signature));
                h.u64(u64::from(v));
            } else if v >= base && (v as usize) < index {
                h.u64(1);
                h.u128(records[(v - base) as usize].hash);
            } else {
                forward += 1;
                h.u64(3);
                h.u64(u64::from(v));
            }
            from = o + 4;
        }
        h.bytes(&body[from..]);
        records.push(Hashed {
            hash: h.finish(),
            size: (len + 2) as u32,
            shareable: kind != leaf::LF_FUNC_ID && kind != leaf::LF_MFUNC_ID,
        });
        at += 2 + len;
    }
    Types::Records {
        records,
        precomp: precomp.is_some(),
        pch,
        forward,
    }
}

#[cfg(test)]
mod tests;
