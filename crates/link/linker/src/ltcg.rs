//! Link-time code generation (`/LTCG`, for `/GL` objects), as link.exe does
//! it (docs/turbolink-c2.md). Symbol resolution drives it (`resolve.rs`'s
//! [`Ltcg`]), through [`Host`]:
//!
//! 1. Each `/GL` object, on the command line or a library's member, is
//!    opened by the back end (`c2.dll`, loaded for the first one) as it
//!    loads. Its publics (with what each definition refers to) and its
//!    directives come back, and a stand-in COFF object with its definitions,
//!    references and directives takes its place while symbols are resolved,
//!    so that the libraries load as they will.
//! 2. Once the libraries are searched, the back end generates code for the
//!    program from three lists of its handles ([`lists`]): the IL
//!    definitions something outside the IL refers to (the roots), the IL's
//!    references that are imports called directly, and (with `/OPT:REF`)
//!    the IL's COMDATs and data nothing reaches.
//! 3. The generated object replaces the stand-ins, and the link goes on.
//!
//! Still a first version: the back end's code-generation switches are
//! `-GA` or `-GD` alone; no incremental LTCG, no PGO.

use std::path::{Path, PathBuf};

use turbonop_hash::{Map, Set};

use crate::c2::{Backend, IlObject, Lists};
use crate::config::Config;
use crate::resolve::{Def, FileKind, LinkError, Ltcg, LtcgView, NONE};

/// Where the back end is: `TURBOLINK_C2`, else `c2.dll` beside the
/// `link.exe` (or `cl.exe`) found on `PATH`, else on `PATH` itself.
pub fn find_backend() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TURBOLINK_C2") {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let c2 = dir.join("c2.dll");
        (c2.is_file() && (dir.join("link.exe").is_file() || dir.join("cl.exe").is_file()))
            .then_some(c2)
    })
}

/// The switches link.exe initializes the back end with: `-optref` with
/// `/OPT:REF`, `-msilDebug` with `/DEBUG` (both, in that order, with
/// both).
pub fn init_switches(config: &Config) -> Vec<&'static str> {
    let mut s = Vec::new();
    if config.gc_sections() {
        s.push("-optref");
    }
    if config.debug != crate::config::Debug::None {
        s.push("-msilDebug");
    }
    s
}

/// A `/GL` object the back end opened, as the link has it.
struct Opened {
    /// Its file in the link (its stand-in's).
    place: u32,
    /// Its publics and directives.
    object: IlObject,
    /// Its definitions (indices into its publics), in its stand-in's order:
    /// section `i + 1` (0-based) is definition `i`'s.
    defs: Vec<usize>,
}

/// The link's link-time code generation: the back end, loaded for the
/// first `/GL` object, and the objects it opened.
#[derive(Default)]
pub struct Host {
    backend: Option<Backend>,
    opened: Vec<Opened>,
}

impl Host {
    /// No back end yet: it's loaded when a `/GL` object first needs it.
    pub fn new() -> Host {
        Host::default()
    }
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

impl Ltcg for Host {
    fn open(
        &mut self,
        config: &Config,
        place: u32,
        path: &Path,
        offset: u64,
        data: &'static [u8],
    ) -> Result<&'static [u8], String> {
        if self.backend.is_none() {
            let c2 = find_backend().ok_or(
                "link-time code generation needs the compiler's back end, c2.dll: not found (set TURBOLINK_C2, or run from a Visual C++ developer prompt)",
            )?;
            self.backend = Some(Backend::load(&c2, &init_switches(config))?);
        }
        let backend = self.backend.as_mut().ok_or("no back end")?;
        let too_big = || format!("{}: too big for the back end", path.display());
        let offset = u32::try_from(offset).map_err(|_| too_big())?;
        let size = u32::try_from(data.len()).map_err(|_| too_big())?;
        let object = backend.open(path, offset, size)?;
        let defs = (0..object.publics.len())
            .filter(|&i| is_definition(object.publics[i].kind))
            .collect();
        let bytes = leak(stand_in(&object));
        self.opened.push(Opened {
            place,
            object,
            defs,
        });
        Ok(bytes)
    }

    fn generate(&mut self, link: &LtcgView<'_>) -> Result<&'static [u8], Vec<LinkError>> {
        let err = |m: String| vec![LinkError::error(1257, m)];
        let (roots, imports, dead) = lists(&self.opened, link);
        let backend = self
            .backend
            .as_mut()
            .ok_or_else(|| err("no back end".into()))?;
        let switches: &[&str] = if link.config.dll { &["-GD"] } else { &["-GA"] };
        let lists = Lists {
            roots: &roots,
            imports: &imports,
            dead: &dead,
        };
        // (link.exe's lines around code generation, on its output, with
        // /LTCG:STATUS too: its progress meter isn't drawn.)
        println!("Generating code");
        let mut generated = backend.generate(switches, &lists).map_err(err)?;
        println!("Finished generating code");
        source_order(&mut generated.coff, &generated.files, &self.opened);
        if let Some(p) = std::env::var_os("TURBOLINK_LTCG_KEEP") {
            // (The generated object, kept for study.)
            let _ = std::fs::write(p, &generated.coff);
        }
        Ok(leak(generated.coff))
    }
}

/// Puts the generated object's sections in link.exe's order: each `/GL`
/// object's code in the order the objects were opened, and within one
/// object in its source's order (the reverse of the order the back end
/// enumerates its publics in). A section is placed by the symbol that
/// leads it (the first one defined in it), found by name among the
/// publics of the object its symbol comes from (`TkFile`, `files`); an
/// associative section goes with the one it is associated with. Sections
/// placed neither way keep their places. Only the section table is
/// reordered (and the symbols' section numbers with it): the contents stay.
fn source_order(coff: &mut [u8], files: &[u64], opened: &[Opened]) {
    let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    if coff.len() < 20 {
        return;
    }
    let nsec = usize::from(u16_at(coff, 2));
    let symbols_at = u32_at(coff, 8) as usize;
    let nsyms = u32_at(coff, 12) as usize;
    let headers_at = 20 + usize::from(u16_at(coff, 16));
    let strings_at = symbols_at + 18 * nsyms;
    if nsec < 2 || headers_at + 40 * nsec > coff.len() || strings_at > coff.len() {
        return;
    }
    let name = |coff: &[u8], at: usize| -> Vec<u8> {
        let raw = &coff[at..at + 8];
        let bytes = if raw[..4] == [0; 4] {
            let off = strings_at + u32_at(raw, 4) as usize;
            let tail = coff.get(off..).unwrap_or(&[]);
            &tail[..tail.iter().position(|&c| c == 0).unwrap_or(tail.len())]
        } else {
            &raw[..raw.iter().position(|&c| c == 0).unwrap_or(8)]
        };
        bytes.to_vec()
    };
    // Each section's leading symbol (its name and object) and, for an
    // associative section, the section it goes with (1-based).
    let mut leader: Vec<Option<(Vec<u8>, u64)>> = vec![None; nsec + 1];
    let mut parent = vec![0usize; nsec + 1];
    let mut i = 0;
    while i < nsyms {
        let at = symbols_at + 18 * i;
        let section = i16::from_le_bytes([coff[at + 12], coff[at + 13]]);
        let class = coff[at + 16];
        let naux = usize::from(coff[at + 17]);
        if let Ok(s) = usize::try_from(section)
            && (1..=nsec).contains(&s)
        {
            if naux > 0 && class == 3 && i + 1 < nsyms {
                // A section's definition: its selection, and with 5 the
                // section it is associated with.
                let aux = at + 18;
                if coff[aux + 14] == 5 {
                    parent[s] = usize::from(u16_at(coff, aux + 12));
                }
            } else if naux == 0 && matches!(class, 2 | 3) && leader[s].is_none() {
                leader[s] = Some((name(coff, at), files.get(i).copied().unwrap_or(0)));
            }
        }
        i += 1 + naux;
    }
    // Each object's publics' places, by name.
    let places: Vec<Map<&[u8], usize>> = opened
        .iter()
        .map(|o| {
            let mut m = Map::default();
            for (j, p) in o.object.publics.iter().enumerate() {
                m.entry(&p.name[..]).or_insert(j);
            }
            m
        })
        .collect();
    let key_of = |s: usize| -> Option<(usize, usize)> {
        let (n, tk) = leader.get(s)?.as_ref()?;
        let k = opened.iter().position(|o| o.object.token == *tk)?;
        let j = *places[k].get(&n[..])?;
        Some((k, opened[k].object.publics.len() - j))
    };
    let keys: Vec<Option<(usize, usize)>> = (0..=nsec)
        .map(|s| {
            if s == 0 {
                None
            } else if parent[s] != 0 && parent[s] != s {
                key_of(parent[s])
            } else {
                key_of(s)
            }
        })
        .collect();
    let placed: Vec<usize> = (1..=nsec).filter(|&s| keys[s].is_some()).collect();
    let mut sorted = placed.clone();
    sorted.sort_by_key(|&s| (keys[s], s));
    if sorted == placed {
        return;
    }
    // new_number[old] = new; the table rewritten.
    let mut new_number: Vec<usize> = (0..=nsec).collect();
    for (&slot, &s) in placed.iter().zip(&sorted) {
        new_number[s] = slot;
    }
    let old: Vec<u8> = coff[headers_at..headers_at + 40 * nsec].to_vec();
    for s in 1..=nsec {
        let to = headers_at + 40 * (new_number[s] - 1);
        coff[to..to + 40].copy_from_slice(&old[40 * (s - 1)..40 * s]);
    }
    let mut i = 0;
    while i < nsyms {
        let at = symbols_at + 18 * i;
        let section = i16::from_le_bytes([coff[at + 12], coff[at + 13]]);
        let class = coff[at + 16];
        let naux = usize::from(coff[at + 17]);
        if let Ok(s) = usize::try_from(section)
            && (1..=nsec).contains(&s)
        {
            let n = i16::try_from(new_number[s]).unwrap_or(section);
            coff[at + 12..at + 14].copy_from_slice(&n.to_le_bytes());
            if naux > 0 && class == 3 && i + 1 < nsyms && coff[at + 18 + 14] == 5 {
                let aux = at + 18;
                let p = usize::from(u16_at(coff, aux + 12));
                if (1..=nsec).contains(&p) {
                    let n = u16::try_from(new_number[p]).unwrap_or(0);
                    coff[aux + 12..aux + 14].copy_from_slice(&n.to_le_bytes());
                }
            }
        }
        i += 1 + naux;
    }
}

/// Whether a public of kind (`SymType`) `kind` is a definition its stand-in
/// has: an external definition (2) or data (4). (1 is a reference, 8 a
/// static.)
fn is_definition(kind: u32) -> bool {
    matches!(kind, 2 | 4)
}

/// `CodeGen6`'s three lists, as link.exe makes them (docs/turbolink-c2.md):
///
/// - **The roots:** the IL definitions (those that won: a COMDAT's first
///   copy) that something outside the IL names: a native object (one that
///   refers to it, or has a copy of the COMDAT that lost to it), the entry
///   point, an export, an `/INCLUDE`.
/// - **The imports:** the IL's references that resolved to an import's
///   thunk (a function imported without `__declspec(dllimport)`).
/// - **The dead:** with `/OPT:REF`, the IL's COMDATs and data (kind 4)
///   that won and that nothing reaches from the roots and from the
///   definitions that are neither, through what each definition refers to
///   (`RgISymRefs`; a weak alias, kind 6, leads to its default). If a
///   definition refers to a handle no object listed, or to a kind not
///   known, none are dead (the back end then keeps them, and the link's
///   own `/OPT:REF` drops what it can).
fn lists(opened: &[Opened], link: &LtcgView<'_>) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let by_place: Map<u32, usize> = opened
        .iter()
        .enumerate()
        .map(|(k, o)| (o.place, k))
        .collect();
    // A global's definition, aliases followed: which object's which public,
    // if it is the IL's.
    let winner = |mut g: u32| -> Option<(usize, usize)> {
        for _ in 0..64 {
            match link.globals.get(g as usize)?.def {
                Def::Alias(t) => g = t,
                Def::Section { file, section, .. } => {
                    let &k = by_place.get(&file)?;
                    let i = usize::try_from(section).ok()?.checked_sub(1)?;
                    return opened[k].defs.get(i).map(|&j| (k, j));
                }
                _ => return None,
            }
        }
        None
    };
    let global = |name: &[u8]| link.names.get(name).copied();
    let handle = |(k, j): (usize, usize)| opened[k].object.publics[j].handle;

    // The roots: by name, as link.exe roots them, whichever copy won (a
    // vtable a native member has too, "largest", is the native's: its
    // section holds the RTTI locator before it). The copy that won if it is
    // the IL's, else the IL's first.
    let mut first_def: Map<&[u8], (usize, usize)> = Map::default();
    for (k, o) in opened.iter().enumerate() {
        for &j in &o.defs {
            first_def.entry(&o.object.publics[j].name).or_insert((k, j));
        }
    }
    let mut roots = Vec::new();
    let mut rooted = Set::default();
    let mut root = |g: u32, roots: &mut Vec<u64>| {
        let d = winner(g).or_else(|| {
            let name = link.globals.get(g as usize)?.name;
            first_def.get(name).copied()
        });
        if let Some(d) = d
            && rooted.insert(d)
        {
            roots.push(handle(d));
        }
    };
    for (place, file) in link.files.iter().enumerate() {
        if let FileKind::Object { globals, .. } = &file.kind {
            // (A stand-in's own symbols don't count.)
            if by_place.contains_key(&crate::n32(place)) {
                continue;
            }
            // Any symbol of it: a reference, or a copy of a COMDAT that
            // lost to the IL's (as link.exe roots the IL's vtables and
            // RTTI that a native library member also has).
            for &g in globals {
                if g != NONE {
                    root(g, &mut roots);
                }
            }
        }
    }
    if let Some(g) = link.entry {
        root(g, &mut roots);
    }
    let named: Vec<&str> = link
        .config
        .exports
        .iter()
        .map(|e| e.internal.as_deref().unwrap_or(&e.name))
        .chain(link.config.includes.iter().map(String::as_str))
        .collect();
    for n in named {
        if let Some(g) = global(n.as_bytes()) {
            root(g, &mut roots);
        }
    }

    // The imports called directly.
    let mut imports = Vec::new();
    for o in opened {
        for p in o.object.publics.iter().filter(|p| p.kind == 1) {
            let mut g = global(&p.name);
            for _ in 0..64 {
                match g.and_then(|g| link.globals.get(g as usize)).map(|g| g.def) {
                    Some(Def::Alias(t)) => g = Some(t),
                    Some(Def::ImportThunk(_)) => {
                        imports.push(p.handle);
                        break;
                    }
                    _ => break,
                }
            }
        }
    }

    // The dead: what the roots and the non-COMDAT definitions don't reach.
    let mut dead = Vec::new();
    if link.config.gc_sections() {
        // (A definition refers to its own object's publics. Handles are
        // unique within an object only: a reference's may be another
        // object's too.)
        let at: Vec<Map<u64, usize>> = opened
            .iter()
            .map(|o| {
                let mut m = Map::default();
                for (j, p) in o.object.publics.iter().enumerate() {
                    m.entry(p.handle).or_insert(j);
                }
                m
            })
            .collect();
        let mut anywhere: Map<u64, Vec<(usize, usize)>> = Map::default();
        for (k, o) in opened.iter().enumerate() {
            for (j, p) in o.object.publics.iter().enumerate() {
                anywhere.entry(p.handle).or_default().push((k, j));
            }
        }
        let mut live: Set<(usize, usize)> = Set::default();
        let mut work: Vec<(usize, usize)> = Vec::new();
        for (k, o) in opened.iter().enumerate() {
            for (j, p) in o.object.publics.iter().enumerate() {
                // (Kind 4, data, the back end leaves out as it does a
                // COMDAT: link.exe lists it with them.)
                if matches!(p.kind, 2 | 8) && p.comdat.is_none() {
                    work.push((k, j));
                }
            }
        }
        work.extend(rooted.iter().copied());
        let mut complete = true;
        while let Some(d) = work.pop() {
            if !live.insert(d) {
                continue;
            }
            // (Handle 0 refers to nothing: an empty field, as a throw's
            // information has.)
            for h in opened[d.0].object.publics[d.1]
                .refs
                .iter()
                .filter(|&&h| h != 0)
            {
                // (Its own object's public, else any object's: a COMDAT's
                // copy may refer to another object's publics, as the back
                // end keeps one copy. All of them: more live is safe.)
                let found: Vec<(usize, usize)> = match at[d.0].get(h) {
                    Some(&j) => vec![(d.0, j)],
                    None => anywhere.get(h).cloned().unwrap_or_default(),
                };
                if found.is_empty() {
                    complete = false;
                }
                for r in found {
                    let p = &opened[r.0].object.publics[r.1];
                    let mut names = vec![&p.name[..]];
                    match p.kind {
                        // A reference: the copy that won.
                        1 => {}
                        // A definition, and (a copy that lost) the copy that
                        // won.
                        2 | 4 => work.push(r),
                        8 => {
                            work.push(r);
                            names.clear();
                        }
                        // A weak alias: its default, this object's or the
                        // copy that won.
                        6 => {
                            let Some(default) = p.default.as_deref() else {
                                complete = false;
                                continue;
                            };
                            let o = &opened[r.0];
                            work.extend(
                                o.defs
                                    .iter()
                                    .filter(|&&j| o.object.publics[j].name == default)
                                    .map(|&j| (r.0, j)),
                            );
                            names = vec![&p.name[..], default];
                        }
                        // (Another kind: what it leads to isn't known.)
                        _ => {
                            complete = false;
                            continue;
                        }
                    }
                    for name in names {
                        if let Some(w) = global(name).and_then(winner) {
                            work.push(w);
                        }
                    }
                }
            }
        }
        if complete {
            for (k, o) in opened.iter().enumerate() {
                for &j in &o.defs {
                    let p = &o.object.publics[j];
                    if (p.comdat.is_some() || p.kind == 4)
                        && !live.contains(&(k, j))
                        && global(&p.name).and_then(winner) == Some((k, j))
                    {
                        dead.push(p.handle);
                    }
                }
            }
        }
    }
    (roots, imports, dead)
}

/// A COFF object standing in for a `/GL` object during the first
/// resolution: its directives, each definition in a section of its own (a
/// COMDAT as the IL's is), each reference undefined.
pub fn stand_in(object: &crate::c2::IlObject) -> Vec<u8> {
    let defs: Vec<&crate::c2::Public> = object
        .publics
        .iter()
        .filter(|p| matches!(p.kind, 2 | 4))
        .collect();
    let refs: Vec<&crate::c2::Public> = object.publics.iter().filter(|p| p.kind == 1).collect();
    let nsec = 1 + defs.len();
    let mut strings: Vec<u8> = vec![0; 4];
    let name8 = |n: &[u8], strings: &mut Vec<u8>| -> [u8; 8] {
        let mut r = [0u8; 8];
        if n.len() <= 8 {
            r[..n.len()].copy_from_slice(n);
        } else {
            let off = u32::try_from(strings.len()).unwrap_or(0);
            r[4..].copy_from_slice(&off.to_le_bytes());
            strings.extend_from_slice(n);
            strings.push(0);
        }
        r
    };
    let mut symbols = Vec::new();
    let put = |name: [u8; 8], section: i16, class: u8, aux: u8, symbols: &mut Vec<u8>| {
        symbols.extend_from_slice(&name);
        symbols.extend_from_slice(&0u32.to_le_bytes());
        symbols.extend_from_slice(&section.to_le_bytes());
        symbols.extend_from_slice(&0u16.to_le_bytes());
        symbols.push(class);
        symbols.push(aux);
    };
    // The directives' section, then one per definition (its section symbol
    // and its definition).
    let mut headers = Vec::new();
    let headers_size = 20 + 40 * nsec;
    let header = |name: &[u8; 8], size: u32, at: u32, chars: u32, headers: &mut Vec<u8>| {
        headers.extend_from_slice(name);
        for v in [0, 0, size, at, 0, 0] {
            headers.extend_from_slice(&u32::to_le_bytes(v));
        }
        headers.extend_from_slice(&[0; 4]);
        headers.extend_from_slice(&chars.to_le_bytes());
    };
    header(
        b".drectve",
        u32::try_from(object.directives.len()).unwrap_or(0),
        u32::try_from(headers_size).unwrap_or(0),
        0x0010_0A00,
        &mut headers,
    );
    for (k, d) in defs.iter().enumerate() {
        let section = i16::try_from(k + 2).unwrap_or(i16::MAX);
        let chars = if d.comdat.is_some() {
            0x6050_1020
        } else {
            0x6050_0020
        };
        header(b".text$mn", 0, 0, chars, &mut headers);
        if let Some(selection) = d.comdat {
            put(
                name8(b".text$mn", &mut strings),
                section,
                3,
                1,
                &mut symbols,
            );
            let mut aux = [0u8; 18];
            aux[14] = if selection == 0 { 2 } else { selection };
            symbols.extend_from_slice(&aux);
        }
        put(name8(&d.name, &mut strings), section, 2, 0, &mut symbols);
    }
    for r in &refs {
        put(name8(&r.name, &mut strings), 0, 2, 0, &mut symbols);
    }
    let total = u32::try_from(strings.len()).unwrap_or(0);
    strings[..4].copy_from_slice(&total.to_le_bytes());
    let mut o = Vec::new();
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&u16::try_from(nsec).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    let symbols_at = headers_size + object.directives.len();
    o.extend_from_slice(&u32::try_from(symbols_at).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&u32::try_from(symbols.len() / 18).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&headers);
    o.extend_from_slice(&object.directives);
    o.extend_from_slice(&symbols);
    o.extend_from_slice(&strings);
    o
}
