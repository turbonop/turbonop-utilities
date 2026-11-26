//! One member's fold units, found in its object model, and its environment.
//!
//! A **fold unit** is a COMDAT leader (any selection but `NODUPLICATES`,
//! `ASSOCIATIVE`, and `NEWEST`) with the sections associated with it,
//! transitively, and the symbols defined in them. A unit can be folded
//! only if moving it to another object changes nothing a linker sees
//! (`--locals=keep`):
//!
//! - every relocation it makes to something outside it names an external
//!   symbol (by name, which means the same in any object);
//! - nothing outside it refers to one of its non-external symbols;
//! - it has no COFF line numbers, and only sections of known kinds.
//!
//! Side tables' entries count as references too ([`tables`](super::tables)
//! says how); a member with one turbolib can't read folds nothing.

use super::Unfoldable;
use super::tables::{self, Tables};
use crate::hash::H128;
use turbonop_coff::{
    Aux, Object, SectionData, SectionId, SectionNumber, SymbolId, comdat_select, storage_class,
};

/// One fold unit of a member.
#[derive(Clone, Debug)]
pub struct Unit {
    /// The leader's section.
    pub leader: SectionId,
    /// The leader's section, then its associated sections, in section
    /// order.
    pub sections: Vec<SectionId>,
    /// The symbols defined in those sections, in symbol order.
    pub symbols: Vec<SymbolId>,
    /// The external symbols among them, by name.
    pub externals: Vec<Vec<u8>>,
    /// The leader's selection.
    pub selection: u8,
    /// The bytes of its sections (data, relocations, and line numbers),
    /// as the object stores them.
    pub size: u64,
    /// Its identity: equal for copies that can stand for each other.
    pub identity: u128,
    /// Why it can't be folded, if it can't.
    pub unfoldable: Option<Unfoldable>,
    /// What made it so, in words (for `--diagnose`).
    pub detail: Option<String>,
}

/// What one member is, for folding.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The environment key (machine, `@feat.00`, `/FAILIFMISMATCH`, and the
    /// compiler's `S_COMPILE3`).
    pub environment: u128,
    /// Loading the member does something by itself: dynamic initializers,
    /// terminators, or TLS callbacks outside COMDATs, or `/INCLUDE`
    /// directives (other than a PCH user's reference to its PCH object).
    pub side_effects: bool,
    /// `@feat.00`'s value, if the member has one.
    pub feat00: Option<u32>,
    /// `@comp.id`'s value, if the member has one.
    pub comp_id: Option<u32>,
    /// The `/FAILIFMISMATCH` directives, as written, in order.
    pub mismatches: Vec<Vec<u8>>,
    /// External symbols the member defines outside its units, by name.
    pub other_definitions: Vec<Vec<u8>>,
    /// Its units.
    pub units: Vec<Unit>,
    /// Its side tables.
    pub tables: Tables,
}

/// Section names (prefixes) whose meaning turbolib knows. A member with a
/// section of any other name is passed through.
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
    b".llvm_addrsig",
];

fn starts_with_any(name: &[u8], prefixes: &[&[u8]]) -> bool {
    prefixes.iter().any(|p| name.starts_with(p))
}

/// Whether a section's contents are debug information, which isn't part
/// of a unit's identity (it comes along with whichever copy survives).
pub fn is_debug(name: &[u8]) -> bool {
    name.starts_with(b".debug$")
}

/// The words of a `.drectve` section.
pub fn directives(data: &[u8]) -> Vec<&[u8]> {
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

/// The `S_COMPILE3` record's body in a `.debug$S` section, if any.
fn compile3(section: &[u8]) -> Option<Vec<u8>> {
    use turbonop_codeview::c13;
    const S_COMPILE3: u16 = 0x113C;
    let subsections = c13::section_subsections(section).ok()?;
    for s in subsections {
        if s.kind != turbonop_codeview::lines::subsection::DEBUG_S_SYMBOLS {
            continue;
        }
        for r in c13::symbol_records(s.data).ok()? {
            if r.kind == S_COMPILE3 {
                return Some(r.data.to_vec());
            }
        }
    }
    None
}

/// Finds the member's units and facts. `Err` is why the member passes
/// through whole.
pub fn find(object: &Object<'_>) -> Result<Facts, Unfoldable> {
    let n = object.sections.len();
    let mut facts = Facts::default();

    // Each section's definition: its section symbol's auxiliary record.
    let mut selection = vec![0u8; n];
    let mut associated: Vec<Option<SectionId>> = vec![None; n];
    let mut checksum = vec![0u32; n];
    for s in &object.symbols {
        if s.storage_class == storage_class::STATIC
            && let (SectionNumber::Section(id), Aux::Section(d)) = (s.section, &s.aux)
            && id.index() < n
            && object.sections[id.index()].characteristics & IMAGE_SCN_LNK_COMDAT != 0
        {
            selection[id.index()] = d.selection;
            associated[id.index()] = d.number;
            checksum[id.index()] = d.checksum;
        }
    }

    let (mut tables, unreadable_table) = match tables::find(object) {
        Ok(t) => (t, None),
        Err(why) => (Tables::default(), Some(why)),
    };
    let mut compile = None;
    for sec in &object.sections {
        let name = sec.name.as_bytes();
        if !starts_with_any(name, KNOWN_SECTIONS) {
            return Err(Unfoldable::UnknownSection);
        }
        let comdat = sec.characteristics & IMAGE_SCN_LNK_COMDAT != 0;
        if name == b".drectve" {
            for d in directives(sec.bytes()) {
                let lower = d.to_ascii_lowercase();
                let body = lower.get(1..).unwrap_or(&[]);
                if lower.first().is_some_and(|&c| c == b'/' || c == b'-') {
                    if body.starts_with(b"failifmismatch:") {
                        facts.mismatches.push(d.to_vec());
                    } else if body.starts_with(b"include:")
                        && !body[8..].starts_with(b"__@@_pchsym_")
                    {
                        facts.side_effects = true;
                    }
                }
            }
        } else if !comdat
            && sec.size() > 0
            && [
                &b".CRT$XC"[..],
                b".CRT$XI",
                b".CRT$XL",
                b".CRT$XP",
                b".CRT$XT",
            ]
            .iter()
            .any(|p| name.starts_with(p))
        {
            facts.side_effects = true;
        } else if name == b".debug$S" && !comdat && compile.is_none() {
            compile = compile3(sec.bytes());
        }
    }

    for s in &object.symbols {
        if s.section == SectionNumber::Absolute {
            match s.name.as_bytes() {
                b"@feat.00" => facts.feat00 = Some(s.value),
                b"@comp.id" => facts.comp_id = Some(s.value),
                _ => {}
            }
        }
    }
    let mut env = H128::new();
    env.u64(u64::from(object.machine));
    env.u64(facts.feat00.map_or(u64::MAX, u64::from));
    let mut sorted = facts.mismatches.clone();
    sorted.sort_unstable();
    for m in &sorted {
        env.u64(m.len() as u64);
        env.bytes(m);
    }
    match &compile {
        Some(c) => {
            env.u64(c.len() as u64);
            env.bytes(c);
        }
        None => env.u64(u64::MAX),
    }
    facts.environment = env.finish();

    // Each section's unit: a leader, or the leader its association chain
    // ends at.
    let leader = |id: usize| -> Option<usize> {
        let mut at = id;
        for _ in 0..=n {
            match selection[at] {
                comdat_select::ANY
                | comdat_select::SAME_SIZE
                | comdat_select::EXACT_MATCH
                | comdat_select::LARGEST => return Some(at),
                comdat_select::ASSOCIATIVE => at = associated[at]?.index(),
                _ => return None,
            }
            if at >= n {
                return None;
            }
        }
        None
    };
    let mut unit_of: Vec<Option<usize>> = vec![None; n];
    let mut units: Vec<Unit> = Vec::new();
    let mut unit_by_leader: Vec<Option<usize>> = vec![None; n];
    for (i, slot) in unit_of.iter_mut().enumerate() {
        if let Some(l) = leader(i) {
            let u = *unit_by_leader[l].get_or_insert_with(|| {
                units.push(Unit {
                    leader: SectionId(l as u32),
                    sections: Vec::new(),
                    symbols: Vec::new(),
                    externals: Vec::new(),
                    selection: selection[l],
                    size: 0,
                    identity: 0,
                    unfoldable: None,
                    detail: None,
                });
                units.len() - 1
            });
            *slot = Some(u);
        }
    }
    for (i, u) in unit_of.iter().enumerate() {
        if let Some(u) = *u {
            units[u].sections.push(SectionId(i as u32));
        }
    }
    // The leader first.
    for u in &mut units {
        u.sections.sort_by_key(|s| (*s != u.leader, s.0));
    }

    // Symbols.
    let symbol_unit: Vec<Option<usize>> = object
        .symbols
        .iter()
        .map(|s| {
            s.section
                .id()
                .and_then(|id| unit_of.get(id.index()).copied().flatten())
        })
        .collect();
    for (i, s) in object.symbols.iter().enumerate() {
        match symbol_unit[i] {
            Some(u) => {
                units[u].symbols.push(SymbolId(i as u32));
                if s.is_external() {
                    units[u].externals.push(s.name.as_bytes().to_vec());
                }
            }
            None => {
                if s.is_external() && s.section.id().is_some() {
                    facts.other_definitions.push(s.name.as_bytes().to_vec());
                }
            }
        }
    }

    // Foldability: references in and out.
    let mut reason: Vec<Option<Unfoldable>> = vec![None; units.len()];
    let mut detail: Vec<Option<String>> = vec![None; units.len()];
    let show = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    // A COMDAT's symbol is the first after its section's own; a local one
    // (internal linkage: anonymous namespaces, `static` inline functions)
    // is never shared between objects, so folding it is `--locals=copy`.
    for (k, u) in units.iter().enumerate() {
        let leader = u
            .symbols
            .iter()
            .map(|&id| &object.symbols[id.index()])
            .find(|s| {
                s.section == SectionNumber::Section(u.leader) && !matches!(s.aux, Aux::Section(_))
            });
        if leader.is_some_and(|s| !s.is_external()) {
            reason[k] = Some(Unfoldable::InternalLinkage);
            detail[k] = leader.map(|s| show(s.name.as_bytes()));
        }
    }
    if let Some(why) = &unreadable_table {
        reason
            .iter_mut()
            .for_each(|r| *r = Some(Unfoldable::SideTables));
        detail.iter_mut().for_each(|d| *d = Some(why.clone()));
    }
    for (i, sec) in object.sections.iter().enumerate() {
        let here = unit_of[i];
        if let Some(u) = here
            && !sec.line_numbers.is_empty()
        {
            reason[u].get_or_insert(Unfoldable::LineNumbers);
        }
        if tables.splits_relocations(SectionId(i as u32)) {
            continue;
        }
        for r in &sec.relocations {
            let Some(target) = object.symbol(r.symbol) else {
                if let Some(u) = here {
                    reason[u].get_or_insert(Unfoldable::BadRelocation);
                }
                continue;
            };
            let there = symbol_unit[r.symbol.index()];
            let what = || {
                format!(
                    "{} refers to {} (storage class {}, in {})",
                    show(sec.name.as_bytes()),
                    show(target.name.as_bytes()),
                    target.storage_class,
                    target
                        .section
                        .id()
                        .and_then(|id| object.sections.get(id.index()))
                        .map_or_else(|| "no section".into(), |t| show(t.name.as_bytes()))
                )
            };
            match (here, there) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), _) => {
                    // Out of the unit: only by an external's name.
                    if target.storage_class == storage_class::WEAK_EXTERNAL
                        && weak_default(object, target).is_none()
                    {
                        if reason[a].is_none() {
                            reason[a] = Some(Unfoldable::WeakExternalReference);
                            detail[a] = Some(what());
                        }
                    } else if !target.is_external()
                        && target.storage_class != storage_class::WEAK_EXTERNAL
                        && reason[a].is_none()
                    {
                        reason[a] = Some(Unfoldable::LocalReference);
                        detail[a] = Some(what());
                    }
                    if let Some(b) = there
                        && !target.is_external()
                        && reason[b].is_none()
                    {
                        reason[b] = Some(Unfoldable::ReferencedLocally);
                        detail[b] = Some(what());
                    }
                }
                (None, Some(b)) => {
                    if !target.is_external() && reason[b].is_none() {
                        reason[b] = Some(Unfoldable::ReferencedLocally);
                        detail[b] = Some(what());
                    }
                }
                (None, None) => {}
            }
        }
    }
    tables.check(object, &unit_of, &symbol_unit, &mut reason, &mut detail);
    // Auxiliary records that name a unit's symbols from outside it.
    for (i, s) in object.symbols.iter().enumerate() {
        let named = match &s.aux {
            Aux::WeakExternal(w) => Some(w.tag),
            Aux::Function(f) => f.tag,
            Aux::ClrToken(c) => Some(c.symbol),
            _ => None,
        };
        if let Some(t) = named
            && let Some(b) = symbol_unit.get(t.index()).copied().flatten()
            && symbol_unit[i] != Some(b)
            && !object
                .symbol(t)
                .is_some_and(turbonop_coff::Symbol::is_external)
            && reason[b].is_none()
        {
            reason[b] = Some(Unfoldable::ReferencedLocally);
            detail[b] = Some(format!(
                "{}'s auxiliary record names {}",
                show(s.name.as_bytes()),
                show(object.symbol(t).map_or(&[][..], |t| t.name.as_bytes()))
            ));
        }
        if let (Some(u), Aux::Raw(_)) = (symbol_unit[i], &s.aux) {
            reason[u].get_or_insert(Unfoldable::RawAuxiliary);
        }
    }

    let identities: Vec<u128> = (0..units.len())
        .map(|k| {
            identity(
                object,
                &units[k],
                k,
                &symbol_unit,
                facts.environment,
                &checksum,
                &tables,
            )
        })
        .collect();
    for (k, u) in units.iter_mut().enumerate() {
        u.unfoldable = reason[k];
        u.detail = detail[k].take();
        u.identity = identities[k];
        u.size = u
            .sections
            .iter()
            .map(|&s| {
                let sec = &object.sections[s.index()];
                u64::from(sec.size()) + 10 * sec.relocations.len() as u64
            })
            .sum();
    }
    facts.units = units;
    facts.tables = tables;
    Ok(facts)
}

const IMAGE_SCN_LNK_COMDAT: u32 = 0x1000;

/// A weak external's search kind and its default's name, when the default
/// is an external symbol (which means the same in any object).
pub fn weak_default<'o>(
    object: &'o Object<'_>,
    symbol: &turbonop_coff::Symbol<'_>,
) -> Option<(u32, &'o [u8])> {
    if symbol.storage_class != storage_class::WEAK_EXTERNAL {
        return None;
    }
    let Aux::WeakExternal(w) = &symbol.aux else {
        return None;
    };
    let default = object.symbol(w.tag)?;
    default
        .is_external()
        .then(|| (w.search, default.name.as_bytes()))
}

/// The unit's identity hash: its environment, selection, and checksum; each
/// non-debug section's name, characteristics, and contents; each
/// relocation, with a target inside the unit by position and one outside
/// by name; and each symbol defined in it (by position within the unit, a
/// local one without its name, which can differ between translation units
/// for the same code, such as `$LN` labels). A side table's entries count
/// as its relocations do.
fn identity(
    object: &Object<'_>,
    unit: &Unit,
    k: usize,
    symbol_unit: &[Option<usize>],
    environment: u128,
    checksum: &[u32],
    tables: &Tables,
) -> u128 {
    let mut h = H128::new();
    h.u128(environment);
    h.u64(u64::from(unit.selection));
    h.u64(u64::from(checksum[unit.leader.index()]));
    let position = |id: SymbolId| unit.symbols.binary_search(&id).ok();
    let section_position = |id: SectionId| unit.sections.iter().position(|&s| s == id);
    for &s in &unit.sections {
        let sec = &object.sections[s.index()];
        let name = sec.name.as_bytes();
        h.u64(name.len() as u64);
        h.bytes(name);
        if is_debug(name) {
            continue;
        }
        h.u64(u64::from(sec.characteristics));
        match (tables.get(s), &sec.data) {
            (Some(t), _) => t.hash(object, unit, k, symbol_unit, &mut h),
            (None, SectionData::Bytes(b)) => {
                h.u64(b.len() as u64);
                h.bytes(b);
            }
            (None, SectionData::Uninitialized(size)) => h.u64(u64::from(*size) | 1 << 63),
        }
        h.u64(sec.relocations.len() as u64);
        for r in &sec.relocations {
            h.u64(u64::from(r.offset) | u64::from(r.kind) << 32);
            let inside = symbol_unit.get(r.symbol.index()).copied().flatten() == Some(k);
            match (inside, position(r.symbol), object.symbol(r.symbol)) {
                (true, Some(p), _) => h.u64(p as u64),
                (_, _, Some(t)) => {
                    let name = t.name.as_bytes();
                    h.u64(u64::MAX - name.len() as u64);
                    h.bytes(name);
                    if let Some((search, default)) = weak_default(object, t) {
                        h.u64(u64::from(search) | 1 << 40);
                        h.u64(default.len() as u64);
                        h.bytes(default);
                    }
                }
                _ => h.u64(u64::MAX),
            }
        }
    }
    h.u64(unit.symbols.len() as u64);
    for &id in &unit.symbols {
        let s = &object.symbols[id.index()];
        h.u64(u64::from(s.value));
        h.u64(
            s.section
                .id()
                .and_then(section_position)
                .map_or(u64::MAX, |p| p as u64),
        );
        h.u64(u64::from(s.storage_class) | u64::from(s.kind) << 8);
        if s.is_external() {
            let name = s.name.as_bytes();
            h.u64(name.len() as u64);
            h.bytes(name);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests;
