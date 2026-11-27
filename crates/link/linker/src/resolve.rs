//! Loading the inputs and resolving symbols: objects first (all of them,
//! parsed in parallel), then libraries searched as `link.exe` searches
//! them, then weak externals, alternate names, and the entry point.
//!
//! # How libraries are searched
//!
//! `link.exe` searches the libraries in order (the command line's, then
//! the default libraries in the order objects name them), each one for
//! every symbol undefined at that moment, until it defines no more; then
//! the next. When a pass over all of them has loaded anything, it starts
//! over from the first. So a symbol that becomes undefined while the third
//! library is searched comes from the third library if it defines it,
//! even if the first does too (lld-link takes it from the first, always).
//! turbolink does as `link.exe` does. The members one step of a search
//! loads are parsed in parallel, and added in the order of their offsets.

use std::path::{Path, PathBuf};

use turbonop_archive::Archive;
use turbonop_hash::{Map, Set};
use turbonop_pe::import_object::{ImportObject, ImportType};

use crate::config::{Config, NoDefaultLib};
use crate::object::{Object, Selection, SymbolKind};

/// Something wrong with the link, to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkError {
    /// `link.exe`'s number for it (2005, 2019, 1104, ...), 0 if none.
    pub code: u32,
    /// What went wrong.
    pub message: String,
    /// Whether it is only a warning.
    pub warning: bool,
}

impl LinkError {
    fn error(code: u32, message: String) -> LinkError {
        LinkError {
            code,
            message,
            warning: false,
        }
    }

    fn warning(code: u32, message: String) -> LinkError {
        LinkError {
            code,
            message,
            warning: true,
        }
    }
}

/// A loaded file: an object (from the command line or a library) or an
/// import object.
#[derive(Debug)]
pub struct File {
    /// How it is named in messages: `a.obj`, or `x.lib(member.obj)`.
    pub name: String,
    /// What it is.
    pub kind: FileKind,
}

/// What a loaded file is.
#[derive(Debug)]
pub enum FileKind {
    /// A COFF object.
    Object {
        /// The object.
        object: Object,
        /// For each symbol record, the global it is (`NONE` for local
        /// symbols and auxiliary records).
        globals: Vec<u32>,
        /// For each section, whether it stays (a COMDAT that lost to
        /// another copy doesn't).
        kept: Vec<bool>,
    },
    /// An import object: one function or variable of a DLL.
    Import {
        /// The import.
        import: ImportObject<'static>,
        /// The `__imp_` global (the import address table's entry).
        address: u32,
        /// The thunk's global (functions only): the name itself.
        thunk: Option<u32>,
    },
}

/// No global.
pub const NONE: u32 = u32::MAX;

/// What defines a global.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Def {
    /// Nothing yet.
    Undefined,
    /// A section of an object (`section` 0-based), at `value`.
    Section {
        /// The file.
        file: u32,
        /// 0-based.
        section: u32,
        /// The offset in the section.
        value: u32,
    },
    /// Common data of this size, defined by `file`.
    Common {
        /// The file.
        file: u32,
        /// Its size.
        size: u32,
    },
    /// An absolute value.
    Absolute(u64),
    /// An absolute value the linker gives (`__guard_flags`), which any
    /// object's definition replaces.
    Synthetic(Synthetic),
    /// An import's address table entry (`__imp_x`).
    ImportAddress(u32),
    /// An import's thunk (`x`: `jmp [__imp_x]`).
    ImportThunk(u32),
    /// Another global's definition (a weak external's default, an
    /// `/ALTERNATENAME`).
    Alias(u32),
}

/// What the linker defines itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synthetic {
    /// `__ImageBase`: the image's base address (RVA 0).
    ImageBase,
    /// A value the link fills in (the `/GUARD` tables and counts, zero
    /// when they are absent).
    Zero,
}

/// A global symbol.
#[derive(Clone, Debug)]
pub struct Global {
    /// Its name.
    pub name: &'static [u8],
    /// What defines it.
    pub def: Def,
    /// A weak external's default: the file and its symbol index, and the
    /// search characteristics (the first weak external of the name).
    pub weak: Option<(u32, u32, u32)>,
    /// The first file that refers to it (for messages).
    pub referenced_by: u32,
    /// Whether nothing is wrong if it stays undefined (`_load_config_used`:
    /// taken from a library if one has it).
    pub optional: bool,
}

/// A library being searched.
struct Library {
    /// A default library (`/DEFAULTLIB`), searched after the command
    /// line's.
    default: bool,
    path: PathBuf,
    archive: Archive<'static>,
    /// Each symbol's member (its header's offset), the first member that
    /// defines it.
    index: Map<&'static [u8], u64>,
    loaded: Set<u64>,
}

/// The link after symbol resolution.
pub struct Resolved {
    /// The loaded files, in order (their place is their index).
    pub files: Vec<File>,
    /// The globals.
    pub globals: Vec<Global>,
    /// Each name's global.
    pub names: Map<&'static [u8], u32>,
    /// The entry point's global, if there is one.
    pub entry: Option<u32>,
    /// The configuration, with what the objects' directives added.
    pub config: Config,
    /// `/VERBOSE`'s lines.
    pub log: Vec<String>,
    /// The `.res` files, in order: their names and contents.
    pub resources: Vec<(String, &'static [u8])>,
    /// Its phases' times.
    pub times: Vec<(&'static str, std::time::Duration)>,
}

/// Maps `path`, for the rest of the process (inputs are never unmapped:
/// what refers to them lives as long, and the process's exit unmaps them).
fn map(path: &Path) -> std::io::Result<&'static [u8]> {
    let map = turbonop_mmap::Mmap::open(path)?;
    Ok(&Box::leak(Box::new(map))[..])
}

/// What a file holds, by its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// A COFF object (or `/bigobj`).
    Object,
    /// An LTCG (`/GL`) object.
    Ltcg,
    /// A short import object.
    Import,
    /// A library.
    Library,
    /// A compiled resource file (`.res`).
    Resources,
    /// Something else.
    Unknown,
}

/// What `data` is.
pub fn identify(data: &[u8]) -> InputKind {
    if Archive::is_archive(data) {
        InputKind::Library
    } else if ImportObject::is_import_object(data) {
        InputKind::Import
    } else if turbonop_pe::ltcg::LtcgObject::is_ltcg(data) {
        InputKind::Ltcg
    } else if data.len() >= 32 && data[..8] == [0, 0, 0, 0, 0x20, 0, 0, 0] {
        InputKind::Resources
    } else if data.len() >= 20 {
        InputKind::Object
    } else {
        InputKind::Unknown
    }
}

struct Linker {
    config: Config,
    files: Vec<File>,
    globals: Vec<Global>,
    names: Map<&'static [u8], u32>,
    libraries: Vec<Library>,
    /// The libraries' paths, lower case, so that each is searched once.
    library_paths: Set<String>,
    /// `/DEFAULTLIB`s seen so far (lower case), so that each is added once.
    default_libs_seen: Set<String>,
    errors: Vec<LinkError>,
    /// `LIB`'s directories.
    lib_env: Vec<PathBuf>,
    /// `/VERBOSE`'s lines.
    log: Vec<String>,
    /// Globals that may want a library's definition: every new global,
    /// until it is seen not to (a definition is never taken back, so a
    /// global dropped from here never needs one again).
    pending: Vec<u32>,
    /// The `.res` files, in order: their names and contents.
    resources: Vec<(String, &'static [u8])>,
    /// Time spent: reading libraries' indexes, parsing members, adding
    /// files' symbols.
    t_index: std::time::Duration,
    t_members: std::time::Duration,
    t_add: std::time::Duration,
}

impl Linker {
    fn intern(&mut self, name: &'static [u8]) -> u32 {
        if let Some(&g) = self.names.get(name) {
            return g;
        }
        let g = u32::try_from(self.globals.len()).expect("fewer than 4 billion symbols");
        self.globals.push(Global {
            name,
            def: Def::Undefined,
            weak: None,
            referenced_by: NONE,
            optional: false,
        });
        self.names.insert(name, g);
        self.pending.push(g);
        g
    }

    /// Adds the loaded file, its symbols resolved against those before it.
    fn add(&mut self, name: String, kind: LoadedKind) {
        let start = std::time::Instant::now();
        self.add_file(name, kind);
        self.t_add += start.elapsed();
    }

    fn add_file(&mut self, name: String, kind: LoadedKind) {
        let place = u32::try_from(self.files.len()).expect("fewer than 4 billion files");
        match kind {
            LoadedKind::Object(object) => {
                let n = object.symbols.len();
                let mut globals = vec![NONE; n];
                let mut kept = vec![true; object.sections.len()];
                // Directives first: they name libraries and alternate
                // names (the order of the objects is kept).
                let mut diags = Vec::new();
                let before = self.config.default_libs.len();
                let includes_before = self.config.includes.len();
                crate::config::apply_directives(&mut self.config, &object.directives, &mut diags);
                // An object's /INCLUDEs are wanted as the command line's are
                // (and are roots of /OPT:REF's mark).
                let new_includes = self.config.includes.split_off(includes_before);
                for include in &new_includes {
                    let g = self.intern(Box::leak(include.clone().into_bytes().into_boxed_slice()));
                    if self.globals[g as usize].referenced_by == NONE {
                        self.globals[g as usize].referenced_by = place;
                    }
                }
                self.config.includes.extend(new_includes);
                for d in diags {
                    if let crate::config::Diagnostic::Bad(m) = d {
                        self.errors
                            .push(LinkError::warning(4229, format!("{name}: {m}")));
                    }
                }
                let new_libs: Vec<String> = self.config.default_libs[before..].to_vec();
                for lib in new_libs {
                    self.add_default_lib(&lib);
                }
                for (i, s) in object.symbols.iter().enumerate() {
                    if !s.is_external() {
                        continue;
                    }
                    let g = self.intern(s.name);
                    globals[i] = g;
                    self.define(g, place, &object, i, &mut kept, &name);
                }
                self.files.push(File {
                    name,
                    kind: FileKind::Object {
                        object,
                        globals,
                        kept,
                    },
                });
            }
            LoadedKind::Import(import) => {
                let mut imp = b"__imp_".to_vec();
                imp.extend_from_slice(import.symbol);
                let address = self.intern(Box::leak(imp.into_boxed_slice()));
                self.define_import(address, Def::ImportAddress(place), &name);
                let thunk = (import.kind == ImportType::Code).then(|| {
                    let t = self.intern(import.symbol);
                    self.define_import(t, Def::ImportThunk(place), &name);
                    t
                });
                self.files.push(File {
                    name,
                    kind: FileKind::Import {
                        import,
                        address,
                        thunk,
                    },
                });
            }
        }
    }

    fn define_import(&mut self, g: u32, def: Def, file: &str) {
        let global = &mut self.globals[g as usize];
        match global.def {
            Def::Undefined | Def::Synthetic(_) => global.def = def,
            // An import beside a definition: the definition wins, with
            // link.exe's warning (LNK4049 or LNK4217 are for the other
            // way around); the import is unused.
            _ => self.errors.push(LinkError::warning(
                4006,
                format!(
                    "{file}: {} already defined; second definition ignored",
                    String::from_utf8_lossy(global.name)
                ),
            )),
        }
    }

    /// Resolves symbol `i` of `object` (file `place`), global `g`.
    fn define(
        &mut self,
        g: u32,
        place: u32,
        object: &Object,
        i: usize,
        kept: &mut [bool],
        file_name: &str,
    ) {
        let s = &object.symbols[i];
        let global = &mut self.globals[g as usize];
        if global.referenced_by == NONE {
            global.referenced_by = place;
        }
        match s.kind {
            SymbolKind::Undefined => {}
            SymbolKind::Weak { default, search } => {
                if global.weak.is_none() {
                    global.weak = Some((place, default, search));
                }
            }
            SymbolKind::Common(size) => match global.def {
                Def::Undefined | Def::Synthetic(_) => {
                    global.def = Def::Common { file: place, size }
                }
                Def::Common { size: old, .. } if size > old => {
                    global.def = Def::Common { file: place, size };
                }
                _ => {}
            },
            SymbolKind::Absolute(v) => match global.def {
                Def::Undefined | Def::Synthetic(_) | Def::Common { .. } => {
                    global.def = Def::Absolute(u64::from(v));
                }
                Def::Absolute(old) if old == u64::from(v) => {}
                _ => self.duplicate(g, file_name),
            },
            SymbolKind::Defined { section, value } => {
                let comdat = object.sections[section as usize].comdat;
                let leader = object.comdat_leaders[section as usize] == crate::n32(i);
                if comdat.is_some() && !kept[section as usize] {
                    // A copy of a COMDAT that lost: its symbols aren't
                    // definitions.
                    return;
                }
                let new = Def::Section {
                    file: place,
                    section,
                    value,
                };
                match global.def {
                    Def::Undefined | Def::Synthetic(_) | Def::Common { .. } | Def::Alias(_) => {
                        global.def = new;
                    }
                    Def::Section {
                        file,
                        section: other,
                        ..
                    } => {
                        let other_comdat = self.section_of(file, other).and_then(|s| s.comdat);
                        match (comdat, other_comdat) {
                            (Some((sel, sum)), Some((_, other_sum))) if leader => {
                                let size = object.sections[section as usize].size;
                                let other_size = self.section_of(file, other).map_or(0, |s| s.size);
                                match sel {
                                    Selection::NoDuplicates => {
                                        self.duplicate(g, file_name);
                                        kept[section as usize] = false;
                                    }
                                    Selection::SameSize if size != other_size => {
                                        self.duplicate(g, file_name);
                                        kept[section as usize] = false;
                                    }
                                    Selection::ExactMatch if sum != other_sum => {
                                        self.duplicate(g, file_name);
                                        kept[section as usize] = false;
                                    }
                                    Selection::Largest if size > other_size => {
                                        // This copy replaces the earlier one.
                                        self.discard(file, other);
                                        self.globals[g as usize].def = new;
                                    }
                                    _ => kept[section as usize] = false,
                                }
                            }
                            // Another symbol of a kept COMDAT: the first
                            // copy's stays.
                            (Some(_), Some(_)) => {}
                            _ => self.duplicate(g, file_name),
                        }
                    }
                    _ => self.duplicate(g, file_name),
                }
            }
            SymbolKind::Local { .. } | SymbolKind::Aux | SymbolKind::Other => {}
        }
    }

    fn section_of(&self, file: u32, section: u32) -> Option<&crate::object::Section> {
        match &self.files.get(file as usize)?.kind {
            FileKind::Object { object, .. } => object.sections.get(section as usize),
            FileKind::Import { .. } => None,
        }
    }

    fn discard(&mut self, file: u32, section: u32) {
        if let Some(File {
            kind: FileKind::Object { kept, .. },
            ..
        }) = self.files.get_mut(file as usize)
            && let Some(k) = kept.get_mut(section as usize)
        {
            *k = false;
        }
    }

    fn duplicate(&mut self, g: u32, file: &str) {
        if self.config.force_multiple {
            return;
        }
        let name = String::from_utf8_lossy(self.globals[g as usize].name).into_owned();
        self.errors.push(LinkError::error(
            2005,
            format!("{file}: {name} already defined"),
        ));
    }

    /// Finds the library `name` names: as given, then in each `/LIBPATH`,
    /// then in each of `LIB`'s directories; `.lib` added if it has no
    /// extension.
    fn find_library(&self, name: &str) -> Option<PathBuf> {
        let mut file = PathBuf::from(name);
        if file.extension().is_none() {
            file.set_extension("lib");
        }
        if file.is_file() {
            return Some(file);
        }
        if file.is_absolute() {
            return None;
        }
        self.config
            .lib_paths
            .iter()
            .chain(&self.lib_env)
            .map(|dir| dir.join(&file))
            .find(|p| p.is_file())
    }

    fn add_default_lib(&mut self, name: &str) {
        let key = name.to_ascii_lowercase();
        let stem = key.strip_suffix(".lib").unwrap_or(&key).to_owned();
        match &self.config.no_default_lib {
            NoDefaultLib::All => return,
            NoDefaultLib::These(list)
                if list
                    .iter()
                    .any(|n| n == &key || n.strip_suffix(".lib").unwrap_or(n) == stem) =>
            {
                return;
            }
            _ => {}
        }
        if !self.default_libs_seen.insert(stem) {
            return;
        }
        match self.find_library(name) {
            Some(path) => self.add_library(path, true),
            None => self
                .errors
                .push(LinkError::error(1104, format!("cannot open file '{name}'"))),
        }
    }

    fn add_library(&mut self, path: PathBuf, default: bool) {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if !self.library_paths.insert(key) {
            return;
        }
        let data = match map(&path) {
            Ok(d) => d,
            Err(e) => {
                self.errors.push(LinkError::error(
                    1104,
                    format!("cannot open file '{}': {e}", path.display()),
                ));
                return;
            }
        };
        let archive = match Archive::parse(data) {
            Ok(a) => a,
            Err(e) => {
                self.errors.push(LinkError::error(
                    1136,
                    format!("invalid or corrupt file '{}': {e}", path.display()),
                ));
                return;
            }
        };
        let start = std::time::Instant::now();
        let symbols = archive.symbols();
        let mut index = turbonop_hash::map_with_capacity(symbols.len());
        for s in symbols {
            index.entry(s.name).or_insert(s.member);
        }
        self.libraries.push(Library {
            path,
            archive,
            index,
            loaded: Set::default(),
            default,
        });
        self.t_index += start.elapsed();
    }

    /// Whether `g` still wants a definition a library could give.
    fn wanted(&self, g: &Global) -> bool {
        match g.def {
            Def::Undefined => match g.weak {
                // A weak external that searches no library takes its
                // default instead.
                Some((_, _, 1)) => false,
                _ => g.referenced_by != NONE || g.optional,
            },
            _ => false,
        }
    }

    /// Searches the libraries as `link.exe` does (see the module's
    /// documentation).
    fn search_libraries(&mut self) {
        loop {
            let mut loaded_any = false;
            // The command line's libraries in their order, then the default
            // libraries in the order objects named them (a library named
            // during the search joins the end).
            let mut position = 0;
            loop {
                let order: Vec<usize> = (0..self.libraries.len())
                    .filter(|&i| !self.libraries[i].default)
                    .chain((0..self.libraries.len()).filter(|&i| self.libraries[i].default))
                    .collect();
                let Some(&lib) = order.get(position) else {
                    break;
                };
                loop {
                    let mut pending = std::mem::take(&mut self.pending);
                    pending.retain(|&g| self.wanted(&self.globals[g as usize]));
                    self.pending = pending;
                    let library = &self.libraries[lib];
                    let mut members: Vec<u64> = self
                        .pending
                        .iter()
                        .filter_map(|&g| library.index.get(self.globals[g as usize].name).copied())
                        .filter(|m| !library.loaded.contains(m))
                        .collect();
                    if members.is_empty() {
                        break;
                    }
                    // In the order they are first needed (the order of the
                    // symbols that need them), as link.exe loads them: it
                    // decides the order of their code, and of the import
                    // directory.
                    let mut seen = Set::default();
                    members.retain(|m| seen.insert(*m));
                    if self.config.verbose {
                        let names: Vec<String> = self
                            .pending
                            .iter()
                            .map(|&g| &self.globals[g as usize])
                            .filter(|g| {
                                library
                                    .index
                                    .get(g.name)
                                    .is_some_and(|m| !library.loaded.contains(m))
                            })
                            .map(|g| format!("      Found {}", String::from_utf8_lossy(g.name)))
                            .collect();
                        self.log
                            .push(format!("    Searching {}:", library.path.display()));
                        self.log.extend(names);
                    }
                    let library = &mut self.libraries[lib];
                    library.loaded.extend(members.iter().copied());
                    let archive = library.archive;
                    let path = library.path.display().to_string();
                    let start = std::time::Instant::now();
                    let parsed = turbonop_par::map(&members, 8, |&offset| {
                        let Some(member) = archive.member_at_offset(offset) else {
                            return Err(format!("{path}: no member at offset {offset:#x}"));
                        };
                        let name = format!("{path}({})", String::from_utf8_lossy(&member.name));
                        load(member.data, &name).map(|k| (name, k))
                    });
                    self.t_members += start.elapsed();
                    for p in parsed {
                        match p {
                            Ok((name, kind)) => self.add(name, kind),
                            Err(m) => self.errors.push(LinkError::error(1136, m)),
                        }
                    }
                    loaded_any = true;
                }
                position += 1;
            }
            if !loaded_any && !self.resolve_aliases() {
                break;
            }
        }
    }

    /// Weak externals take their defaults; `/ALTERNATENAME`s their
    /// targets (for names still undefined, after the libraries were
    /// searched for them). A target becomes wanted itself (the libraries
    /// are searched for it next). Returns whether anything changed.
    fn resolve_aliases(&mut self) -> bool {
        let mut changed = false;
        for g in 0..self.globals.len() {
            if self.globals[g].def != Def::Undefined {
                continue;
            }
            if let Some((file, default, _)) = self.globals[g].weak
                && let FileKind::Object {
                    globals, object, ..
                } = &self.files[file as usize].kind
            {
                let target = globals.get(default as usize).copied().unwrap_or(NONE);
                if target != NONE && target != crate::n32(g) {
                    self.globals[g].def = Def::Alias(target);
                    changed = true;
                    if self.globals[target as usize].referenced_by == NONE {
                        self.globals[target as usize].referenced_by = file;
                    }
                } else if let Some(s) = object.symbols.get(default as usize)
                    && let SymbolKind::Local { section, value } = s.kind
                {
                    // A weak external whose default is a static symbol.
                    self.globals[g].def = Def::Section {
                        file,
                        section,
                        value,
                    };
                    changed = true;
                }
            }
        }
        let alternates = self.config.alternate_names.clone();
        for (from, to) in alternates {
            let Some(&g) = self.names.get(from.as_bytes()) else {
                continue;
            };
            let global = &self.globals[g as usize];
            if global.def != Def::Undefined || global.referenced_by == NONE || from == to {
                continue;
            }
            let by = global.referenced_by;
            if self.config.verbose {
                self.log
                    .push(format!("    {from} -> {to} (/ALTERNATENAME)"));
            }
            let t = self.intern(Box::leak(to.into_bytes().into_boxed_slice()));
            self.globals[g as usize].def = Def::Alias(t);
            if self.globals[t as usize].referenced_by == NONE {
                self.globals[t as usize].referenced_by = by;
            }
            changed = true;
        }
        changed
    }
}

/// A command-line input, read.
enum Input {
    /// An object or import object.
    Loaded(LoadedKind),
    /// A library (searched later), found here.
    Library(PathBuf),
    /// A compiled resource file.
    Resources(&'static [u8]),
}

/// What a loaded input is.
#[derive(Debug)]
pub enum LoadedKind {
    /// A COFF object.
    Object(Object),
    /// An import object.
    Import(ImportObject<'static>),
}

/// Reads an object or import object.
fn load(data: &'static [u8], name: &str) -> Result<LoadedKind, String> {
    match identify(data) {
        InputKind::Import => ImportObject::parse(data)
            .map(LoadedKind::Import)
            .map_err(|e| format!("{name}: invalid import object: {e}")),
        InputKind::Object => Object::parse(data)
            .map(LoadedKind::Object)
            .map_err(|e| format!("{name}: invalid or corrupt file: {}", e.0)),
        InputKind::Ltcg => Err(format!(
            "{name}: an LTCG (/GL) object: link-time code generation is not supported yet"
        )),
        _ => Err(format!("{name}: not an object")),
    }
}

/// The symbols the linker defines, for objects that refer to them.
const SYNTHETIC: &[&[u8]] = &[
    b"__guard_fids_count",
    b"__guard_fids_table",
    b"__guard_flags",
    b"__guard_iat_count",
    b"__guard_iat_table",
    b"__guard_longjmp_count",
    b"__guard_longjmp_table",
    b"__guard_eh_cont_count",
    b"__guard_eh_cont_table",
    b"__enclave_config",
    b"__dynamic_value_reloc_table",
    b"__volatile_metadata",
    b"__guard_xfg_check_icall_fptr",
    b"__guard_xfg_dispatch_icall_fptr",
    b"__guard_xfg_table_dispatch_icall_fptr",
    b"__castguard_check_failure_os_handled_fptr",
    b"__CHPEMetadata",
];

/// Loads the inputs and resolves their symbols.
pub fn resolve(config: Config, errors: &mut Vec<LinkError>) -> Option<Resolved> {
    let lib_env = std::env::var_os("LIB")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    let mut l = Linker {
        config,
        files: Vec::new(),
        globals: Vec::new(),
        names: Map::default(),
        libraries: Vec::new(),
        library_paths: Set::default(),
        default_libs_seen: Set::default(),
        errors: Vec::new(),
        lib_env,
        log: Vec::new(),
        pending: Vec::new(),
        resources: Vec::new(),
        t_index: std::time::Duration::ZERO,
        t_members: std::time::Duration::ZERO,
        t_add: std::time::Duration::ZERO,
    };
    let mut times = Vec::new();
    let mut lap = std::time::Instant::now();
    let image_base = l.intern(b"__ImageBase");
    l.globals[image_base as usize].def = Def::Synthetic(Synthetic::ImageBase);
    for &name in SYNTHETIC {
        let g = l.intern(name);
        l.globals[g as usize].def = Def::Synthetic(Synthetic::Zero);
    }

    // The command line's inputs: objects are read in parallel, then added
    // in order; libraries are searched later, in their order.
    // A file not found as named is looked for as libraries are (in each
    // /LIBPATH, then in LIB's directories): `kernel32.lib`.
    let inputs = l.config.inputs.clone();
    let lookup = &l;
    let read = turbonop_par::map_balanced(&inputs, 1, |path| {
        let mut p = PathBuf::from(path);
        if !p.is_file()
            && p.is_relative()
            && let Some(found) = lookup
                .config
                .lib_paths
                .iter()
                .chain(&lookup.lib_env)
                .map(|d| d.join(path))
                .find(|c| c.is_file())
        {
            p = found;
        }
        let data = map(&p).map_err(|e| format!("cannot open file '{path}': {e}"))?;
        Ok(match identify(data) {
            InputKind::Library => Input::Library(p),
            InputKind::Resources => Input::Resources(data),
            _ => Input::Loaded(load(data, path)?),
        })
    });
    for (path, r) in inputs.iter().zip(read) {
        match r {
            Ok(Input::Loaded(kind)) => l.add(path.clone(), kind),
            Ok(Input::Library(p)) => l.add_library(p, false),
            Ok(Input::Resources(data)) => l.resources.push((path.clone(), data)),
            Err(m) => l.errors.push(LinkError::error(1104, m)),
        }
    }
    times.push(("  inputs (objects read, added)", lap.elapsed()));
    lap = std::time::Instant::now();
    let cmdline_defaults = l.config.default_libs.clone();
    for lib in &cmdline_defaults {
        l.add_default_lib(lib);
    }

    // The entry point, the /INCLUDEs, and what the image needs if a
    // library has it, are wanted before the libraries are searched.
    let entry_name = entry_point(&l);
    let entry = entry_name.map(|n| {
        let g = l.intern(Box::leak(n.into_bytes().into_boxed_slice()));
        if l.globals[g as usize].referenced_by == NONE {
            l.globals[g as usize].referenced_by = 0;
        }
        g
    });
    for name in l.config.includes.clone() {
        let g = l.intern(Box::leak(name.into_bytes().into_boxed_slice()));
        if l.globals[g as usize].referenced_by == NONE {
            l.globals[g as usize].referenced_by = 0;
        }
    }
    // What is exported is wanted (a forwarder, `f=other.g`, names no
    // symbol: it is optional).
    for e in l.config.exports.clone() {
        let target = e.internal.clone().unwrap_or_else(|| e.name.clone());
        let forwarder = target.contains('.');
        let g = l.intern(Box::leak(target.into_bytes().into_boxed_slice()));
        if l.globals[g as usize].referenced_by == NONE {
            l.globals[g as usize].referenced_by = 0;
        }
        if forwarder {
            l.globals[g as usize].optional = true;
        }
    }
    // The load configuration is taken from a library if one has it (as
    // link.exe does); the TLS directory only if something refers to it.
    let g = l.intern(b"_load_config_used");
    l.globals[g as usize].optional = true;

    times.push(("  libraries' indexes", lap.elapsed()));
    lap = std::time::Instant::now();
    l.search_libraries();
    times.push(("  library search", lap.elapsed()));
    times.push(("    of which: indexes", l.t_index));
    times.push(("    of which: members parsed", l.t_members));
    times.push(("  (all files' symbols added)", l.t_add));

    // What is still undefined.
    for g in &l.globals {
        if g.def == Def::Undefined && g.referenced_by != NONE && !g.optional {
            let by = l
                .files
                .get(g.referenced_by as usize)
                .map_or("<command line>", |f| f.name.as_str());
            if !l.config.force_unresolved {
                l.errors.push(LinkError::error(
                    2001,
                    format!(
                        "{by}: unresolved external symbol {}",
                        String::from_utf8_lossy(g.name)
                    ),
                ));
            }
        }
    }
    errors.append(&mut l.errors);
    if errors.iter().any(|e| !e.warning) {
        return None;
    }
    Some(Resolved {
        files: l.files,
        globals: l.globals,
        names: l.names,
        entry,
        config: l.config,
        log: l.log,
        resources: l.resources,
        times,
    })
}

/// The entry point's name: `/ENTRY`, or what `link.exe` picks for the
/// subsystem (and, without `/SUBSYSTEM`, the subsystem from which of
/// `main`, `wmain`, `WinMain`, `wWinMain` the objects define).
fn entry_point(l: &Linker) -> Option<String> {
    use crate::config::Subsystem;
    if l.config.no_entry {
        return None;
    }
    if let Some(e) = &l.config.entry {
        return Some(e.clone());
    }
    let defined = |n: &str| {
        l.names
            .get(n.as_bytes())
            .is_some_and(|&g| l.globals[g as usize].def != Def::Undefined)
    };
    if l.config.dll {
        return Some("_DllMainCRTStartup".into());
    }
    let windows = match l.config.subsystem {
        Some(Subsystem::Windows) => true,
        Some(_) => false,
        None => {
            !defined("main") && !defined("wmain") && (defined("WinMain") || defined("wWinMain"))
        }
    };
    Some(
        match (windows, defined("wmain") || defined("wWinMain")) {
            (false, false) => "mainCRTStartup",
            (false, true) => "wmainCRTStartup",
            (true, false) => "WinMainCRTStartup",
            (true, true) => "wWinMainCRTStartup",
        }
        .into(),
    )
}
