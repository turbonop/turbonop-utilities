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
//!
//! # `/GL` objects
//!
//! An LTCG (`/GL`) object, on the command line or a library's member, is
//! handed to the link's [`Ltcg`] as it loads; what comes back, a stand-in
//! COFF object with its definitions, references and directives, is
//! resolved like any other. Once the search is over, the [`Ltcg`] generates
//! code for the program from the link as resolved. The stand-ins leave the
//! link, the generated object joins it, first (where link.exe lays it
//! out), and the libraries are searched again for what the code refers to,
//! as link.exe does. Nothing loaded before is unloaded, as with link.exe.

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
    pub(crate) fn error(code: u32, message: String) -> LinkError {
        LinkError {
            code,
            message,
            warning: false,
        }
    }

    pub(crate) fn warning(code: u32, message: String) -> LinkError {
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
    /// For an import library's own pieces (its import descriptor, null
    /// import descriptor, and null thunks): which. turbolink builds the
    /// import tables itself; these are loaded as link.exe loads them, for
    /// their place in the order of the tables' contents.
    pub idata: Option<IdataPiece>,
    /// The library it was loaded from (its index among the libraries, in
    /// the order they were opened), if any.
    pub library: Option<u32>,
    /// Its offset in its library, as a member of one (link.exe keeps a
    /// library's modules in the order of their offsets).
    pub member: Option<u64>,
}

/// One of an import library's own objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdataPiece {
    /// `__IMPORT_DESCRIPTOR_x`: DLL `x`'s directory entry and name.
    Descriptor(Vec<u8>),
    /// `\x7fx_NULL_THUNK_DATA`: DLL `x`'s lookup and address tables' end.
    NullThunk(Vec<u8>),
    /// `__NULL_IMPORT_DESCRIPTOR`: the directory's end.
    NullDescriptor,
}

/// The name a DLL's import library pieces are named for: its name without
/// its extension (`KERNEL32.dll`: `KERNEL32`).
pub fn dll_stem(dll: &[u8]) -> &[u8] {
    match dll.iter().rposition(|&b| b == b'.') {
        Some(p) => &dll[..p],
        None => dll,
    }
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
    /// `__volatile_metadata`: the volatile metadata link.exe adds (x64).
    VolatileMetadata,
    /// One of `/GUARD:CF`'s tables, counts, or its flags.
    Guard(crate::guard::Symbol),
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
    /// The libraries (their indices, as files name them) in the order they
    /// are searched: the command line's, then the default libraries.
    pub library_order: Vec<u32>,
    /// Whether the link had `/GL` objects (their code generated).
    pub ltcg: bool,
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
    /// Whether the load configuration has been asked for (after the
    /// rest of the search).
    load_config_wanted: bool,
    /// The library the file being added comes from.
    loading_from: Option<u32>,
    /// The member being loaded's offset (with `loading_from`).
    loading_member: Option<u64>,
    /// Whether a delay-loaded import has asked for `__delayLoadHelper2`.
    delay_helper_wanted: bool,
    /// Globals that may want a library's definition: every new global,
    /// until it is seen not to (a definition is never taken back, so a
    /// global dropped from here never needs one again).
    pending: Vec<u32>,
    /// The `.res` files, in order: their names and contents.
    resources: Vec<(String, &'static [u8])>,
    /// Link-time code generation, until the code is generated.
    ltcg: Option<Box<dyn Ltcg>>,
    /// The `/GL` objects' stand-ins (their files), in the order they were
    /// opened.
    stand_ins: Vec<u32>,
    /// Whether code was generated for `/GL` objects.
    generated: bool,
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
        // A function override's name (`f_$fo$`) that nothing defines is the
        // image's base (RVA 0), as link.exe makes it without /FUNCTIONOVERRIDE:
        // no library is searched for it, and an /ALTERNATENAME for it is not
        // used (memcpy.obj's memcpy_$fo$; checkcfg.obj's
        // _guard_check_icall_$fo$). Only that suffix: `f$fo$` is searched for.
        let def = if name.ends_with(b"_$fo$") {
            Def::Synthetic(Synthetic::ImageBase)
        } else {
            Def::Undefined
        };
        self.globals.push(Global {
            name,
            def,
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
        if let Some(f) = self.files.last_mut() {
            f.library = self.loading_from;
            f.member = self.loading_member.take();
        }
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
                let new_includes = self.config.includes.split_off(includes_before);
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
                let mut idata = None;
                for (i, s) in object.symbols.iter().enumerate() {
                    if !s.is_external() {
                        continue;
                    }
                    if let SymbolKind::Defined { .. } = s.kind {
                        if let Some(x) = s.name.strip_prefix(b"__IMPORT_DESCRIPTOR_") {
                            idata = Some(IdataPiece::Descriptor(x.to_vec()));
                        } else if s.name == b"__NULL_IMPORT_DESCRIPTOR" {
                            idata = Some(IdataPiece::NullDescriptor);
                        } else if let Some(x) = s
                            .name
                            .strip_prefix(b"\x7f")
                            .and_then(|n| n.strip_suffix(b"_NULL_THUNK_DATA"))
                        {
                            idata = Some(IdataPiece::NullThunk(x.to_vec()));
                        }
                    }
                    let g = self.intern(s.name);
                    globals[i] = g;
                    self.define(g, place, &object, i, &mut kept, &name);
                }
                // An object's /INCLUDEs are wanted as the command line's are
                // (and are roots of /OPT:REF's mark); after its symbols, as
                // link.exe adds them to the names still wanted (libucrt's
                // _file.obj's __acrt_stdio_initializer is searched for after
                // its __pioinfo).
                for include in &new_includes {
                    let g = self.intern(Box::leak(include.clone().into_bytes().into_boxed_slice()));
                    if self.globals[g as usize].referenced_by == NONE {
                        self.globals[g as usize].referenced_by = place;
                    }
                }
                self.config.includes.extend(new_includes);
                self.files.push(File {
                    name,
                    idata,
                    library: None,
                    member: None,
                    kind: FileKind::Object {
                        object,
                        globals,
                        kept,
                    },
                });
            }
            LoadedKind::Il { data, path } => self.add_il(name, data, path),
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
                if self.config.is_delay_loaded(import.dll) {
                    // A delay-loaded import wants no import descriptor, but
                    // the helper that loads its DLL (from delayimp.lib), as
                    // link.exe asks for it once the first one loads.
                    if !self.delay_helper_wanted {
                        self.delay_helper_wanted = true;
                        let h = self.intern(b"__delayLoadHelper2");
                        let global = &mut self.globals[h as usize];
                        if global.referenced_by == NONE {
                            global.referenced_by = place;
                        }
                    }
                } else {
                    // An import wants its DLL's import descriptor (from its
                    // library), as link.exe's do: where that loads orders
                    // the import directory and the DLLs' names.
                    let descriptor = [&b"__IMPORT_DESCRIPTOR_"[..], dll_stem(import.dll)].concat();
                    let d = self.intern(Box::leak(descriptor.into_boxed_slice()));
                    let global = &mut self.globals[d as usize];
                    global.optional = true;
                    if global.referenced_by == NONE {
                        global.referenced_by = place;
                    }
                }
                self.files.push(File {
                    name,
                    idata: None,
                    library: None,
                    member: None,
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
    // (The object's parts it needs are passed apart: `object` is being
    // added, not yet one of `self`'s files.)
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

    /// The entry point, the `/INCLUDE`s and the exports are wanted (by
    /// the command line: file 0, for messages).
    fn want_named(&mut self, entry: Option<u32>) {
        if let Some(g) = entry
            && self.globals[g as usize].referenced_by == NONE
        {
            self.globals[g as usize].referenced_by = 0;
        }
        for name in self.config.includes.clone() {
            let g = self.intern(Box::leak(name.into_bytes().into_boxed_slice()));
            if self.globals[g as usize].referenced_by == NONE {
                self.globals[g as usize].referenced_by = 0;
            }
        }
        // What is exported is wanted (a forwarder, `f=other.g`, names no
        // symbol: it is optional).
        for e in self.config.exports.clone() {
            let target = e.internal.clone().unwrap_or_else(|| e.name.clone());
            let forwarder = target.contains('.');
            let g = self.intern(Box::leak(target.into_bytes().into_boxed_slice()));
            if self.globals[g as usize].referenced_by == NONE {
                self.globals[g as usize].referenced_by = 0;
            }
            if forwarder {
                self.globals[g as usize].optional = true;
            }
        }
    }

    /// A `/GL` object, loaded: opened by the link's [`Ltcg`], its stand-in
    /// added in its place.
    fn add_il(&mut self, name: String, data: &'static [u8], path: Option<PathBuf>) {
        let place = crate::n32(self.files.len());
        let (file, offset) = match (self.loading_from, self.loading_member) {
            (Some(lib), Some(member)) => {
                let library = &self.libraries[lib as usize];
                let offset = library
                    .archive
                    .member_at_offset(member)
                    .map_or(0, |m| m.data_offset);
                (library.path.clone(), offset)
            }
            _ => (path.unwrap_or_else(|| PathBuf::from(&name)), 0),
        };
        let Some(ltcg) = self.ltcg.as_mut() else {
            let why = if self.generated {
                "found after the code was generated"
            } else {
                "but link-time code generation isn't available here"
            };
            self.errors.push(LinkError::error(
                1257,
                format!("{name}: an LTCG (/GL) object, {why}"),
            ));
            return;
        };
        let stand_in = ltcg
            .open(&self.config, place, &file, offset, data)
            .and_then(|b| Object::parse(b).map_err(|e| format!("{name}: its stand-in: {}", e.0)));
        match stand_in {
            Ok(object) => {
                if self.stand_ins.is_empty() && !self.config.ltcg {
                    // As link.exe restarts such a link with /LTCG.
                    self.errors.push(LinkError::warning(
                        4197,
                        "/GL objects found; linking with /LTCG: add /LTCG to the link command line"
                            .into(),
                    ));
                }
                self.stand_ins.push(place);
                self.add_file(name, LoadedKind::Object(object));
            }
            Err(m) => self.errors.push(LinkError::error(1257, m)),
        }
    }

    /// Code generation, once the search is over: the stand-ins leave the
    /// link, the generated object joins it first (where link.exe lays it
    /// out), and the libraries are searched for what it refers to.
    fn generate_code(&mut self, entry: Option<u32>) {
        let Some(mut ltcg) = self.ltcg.take() else {
            return;
        };
        self.generated = true;
        if self.errors.iter().any(|e| !e.warning) {
            return;
        }
        let view = LtcgView {
            files: &self.files,
            globals: &self.globals,
            names: &self.names,
            entry,
            config: &self.config,
        };
        let code = ltcg.generate(&view);
        // (The back end is done with: released.)
        drop(ltcg);
        let object = match code.and_then(|code| {
            Object::parse(code).map_err(|e| {
                vec![LinkError::error(
                    1257,
                    format!("the generated code: {}", e.0),
                )]
            })
        }) {
            Ok(o) => o,
            Err(mut e) => {
                self.errors.append(&mut e);
                return;
            }
        };
        // What the stand-ins defined is undefined again, and what only they
        // referred to is no longer wanted.
        let mut stand_in = vec![false; self.files.len()];
        for &f in &self.stand_ins {
            stand_in[f as usize] = true;
        }
        let gone = |f: u32| stand_in.get(f as usize).copied().unwrap_or(false);
        for g in &mut self.globals {
            match g.def {
                Def::Section { file, .. } | Def::Common { file, .. } if gone(file) => {
                    g.def = Def::Undefined;
                }
                _ => {}
            }
            if g.referenced_by != NONE && gone(g.referenced_by) {
                g.referenced_by = NONE;
            }
            if g.weak.is_some_and(|(f, _, _)| gone(f)) {
                g.weak = None;
            }
        }
        // The generated object, then the files in their order, without the
        // stand-ins.
        let generated = self.files.len();
        self.loading_from = None;
        self.loading_member = None;
        self.add(LTCG_FILE.to_owned(), LoadedKind::Object(object));
        let mut new_place = vec![NONE; self.files.len()];
        new_place[generated] = 0;
        let mut next = 1;
        for (f, place) in new_place.iter_mut().enumerate().take(generated) {
            if !stand_in[f] {
                *place = next;
                next += 1;
            }
        }
        self.renumber_files(&new_place);
        self.stand_ins.clear();
        // What the files left refer to is wanted (by the first of them to
        // name it), as are the command line's names.
        for (place, file) in self.files.iter().enumerate() {
            if let FileKind::Object { globals, .. } = &file.kind {
                for &g in globals {
                    if g != NONE && self.globals[g as usize].referenced_by == NONE {
                        self.globals[g as usize].referenced_by = crate::n32(place);
                    }
                }
            }
        }
        self.want_named(entry);
        self.pending = (0..crate::n32(self.globals.len())).collect();
        self.search_libraries();
    }

    /// Moves each file `f` to `new_place[f]` (`NONE`: it leaves the link),
    /// and what refers to it with it.
    fn renumber_files(&mut self, new_place: &[u32]) {
        let at = |f: u32| new_place.get(f as usize).copied().unwrap_or(NONE);
        for g in &mut self.globals {
            match &mut g.def {
                Def::Section { file, .. }
                | Def::Common { file, .. }
                | Def::ImportAddress(file)
                | Def::ImportThunk(file) => *file = at(*file),
                _ => {}
            }
            if g.referenced_by != NONE {
                g.referenced_by = at(g.referenced_by);
            }
            if let Some((f, _, _)) = &mut g.weak {
                *f = at(*f);
            }
        }
        let mut files: Vec<Option<File>> = std::mem::take(&mut self.files)
            .into_iter()
            .map(Some)
            .collect();
        let mut order: Vec<(u32, usize)> = new_place
            .iter()
            .enumerate()
            .filter(|&(_, &p)| p != NONE)
            .map(|(f, &p)| (p, f))
            .collect();
        order.sort_unstable();
        self.files = order
            .into_iter()
            .filter_map(|(_, f)| files[f].take())
            .collect();
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
                // link.exe's walk: the undefined symbols in order, a member
                // loaded as soon as one is found (what it leaves undefined
                // joins the end of the list, and is reached in this walk).
                // Members are parsed ahead, in parallel: all the list needs
                // now, and again whenever the walk finds one not parsed.
                let mut pending = std::mem::take(&mut self.pending);
                pending.retain(|&g| self.wanted(&self.globals[g as usize]));
                self.pending = pending;
                let mut parsed: Map<u64, Result<(String, LoadedKind), String>> = Map::default();
                let mut logged = false;
                let mut i = 0;
                while i < self.pending.len() {
                    let g = self.pending[i];
                    i += 1;
                    if !self.wanted(&self.globals[g as usize]) {
                        continue;
                    }
                    let library = &self.libraries[lib];
                    let Some(&member) = library.index.get(self.globals[g as usize].name) else {
                        continue;
                    };
                    if library.loaded.contains(&member) {
                        continue;
                    }
                    if !parsed.contains_key(&member) {
                        let ahead: Vec<u64> = {
                            let mut seen = Set::default();
                            self.pending[i - 1..]
                                .iter()
                                .filter_map(|&g| {
                                    library.index.get(self.globals[g as usize].name).copied()
                                })
                                .filter(|m| !library.loaded.contains(m) && !parsed.contains_key(m))
                                .filter(|m| seen.insert(*m))
                                .collect()
                        };
                        let archive = library.archive;
                        let path = library.path.display().to_string();
                        let start = std::time::Instant::now();
                        let results = turbonop_par::map(&ahead, 8, |&offset| {
                            let Some(m) = archive.member_at_offset(offset) else {
                                return Err(format!("{path}: no member at offset {offset:#x}"));
                            };
                            let name = format!("{path}({})", String::from_utf8_lossy(&m.name));
                            load(m.data, &name).map(|k| (name, k))
                        });
                        self.t_members += start.elapsed();
                        parsed.extend(ahead.into_iter().zip(results));
                    }
                    if self.config.verbose {
                        if !logged {
                            self.log.push(format!(
                                "    Searching {}:",
                                self.libraries[lib].path.display()
                            ));
                            logged = true;
                        }
                        self.log.push(format!(
                            "      Found {}",
                            String::from_utf8_lossy(self.globals[g as usize].name)
                        ));
                    }
                    self.libraries[lib].loaded.insert(member);
                    match parsed.remove(&member) {
                        Some(Ok((name, kind))) => {
                            self.loading_from = Some(crate::n32(lib));
                            self.loading_member = Some(member);
                            self.add(name, kind);
                            self.loading_from = None;
                        }
                        Some(Err(m)) => self.errors.push(LinkError::error(1136, m)),
                        None => {}
                    }
                    loaded_any = true;
                }
                position += 1;
            }
            if !loaded_any && !self.resolve_aliases() {
                // Once nothing more is needed, link.exe looks for the load
                // configuration (a last round, if a library has it).
                if !self.load_config_wanted {
                    self.load_config_wanted = true;
                    let g = self.intern(b"_load_config_used");
                    let global = &mut self.globals[g as usize];
                    global.optional = true;
                    if global.def == Def::Undefined {
                        self.pending.push(g);
                        continue;
                    }
                }
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
            if self.globals[g].def != Def::Undefined || self.globals[g].referenced_by == NONE {
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
    /// A `/GL` object (the file it's in, if it is a command-line input).
    Il {
        /// Its bytes.
        data: &'static [u8],
        /// Its file, as found (a member's is its library's).
        path: Option<PathBuf>,
    },
}

/// The link's link-time code generation (`ltcg.rs`'s), which resolution
/// drives: each `/GL` object as it loads, then the code for them all.
pub trait Ltcg {
    /// Opens the `/GL` object `data`, at `offset` in `path` (a library's
    /// member is opened in its library), to be the link's file `place`:
    /// returns a COFF object to stand in for it while symbols are resolved.
    ///
    /// # Errors
    ///
    /// When the back end can't be loaded or can't read the object.
    fn open(
        &mut self,
        config: &Config,
        place: u32,
        path: &Path,
        offset: u64,
        data: &'static [u8],
    ) -> Result<&'static [u8], String>;

    /// Generates code for the `/GL` objects, from the link as resolved:
    /// a COFF object.
    ///
    /// # Errors
    ///
    /// When code generation fails.
    fn generate(&mut self, link: &LtcgView<'_>) -> Result<&'static [u8], Vec<LinkError>>;
}

/// The link as resolved before code generation, for [`Ltcg::generate`].
pub struct LtcgView<'a> {
    /// The files (the stand-ins are those [`Ltcg::open`] made).
    pub files: &'a [File],
    /// The globals.
    pub globals: &'a [Global],
    /// Each name's global.
    pub names: &'a Map<&'static [u8], u32>,
    /// The entry point's global.
    pub entry: Option<u32>,
    /// The configuration, the objects' directives in it.
    pub config: &'a Config,
}

/// The generated code's file name, as link.exe names it.
pub const LTCG_FILE: &str = "* CIL library *(* CIL module *)";

/// Reads an object or import object.
fn load(data: &'static [u8], name: &str) -> Result<LoadedKind, String> {
    match identify(data) {
        InputKind::Import => ImportObject::parse(data)
            .map(LoadedKind::Import)
            .map_err(|e| format!("{name}: invalid import object: {e}")),
        InputKind::Object => Object::parse(data)
            .map(LoadedKind::Object)
            .map_err(|e| format!("{name}: invalid or corrupt file: {}", e.0)),
        InputKind::Ltcg => Ok(LoadedKind::Il { data, path: None }),
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
    b"__CHPEMetadata",
];

/// Loads the inputs and resolves their symbols.
pub fn resolve(config: Config, errors: &mut Vec<LinkError>) -> Option<Resolved> {
    resolve_with(config, None, errors)
}

/// Loads the inputs and resolves their symbols, `/GL` objects' code
/// generated by `ltcg` (without it, a `/GL` object is an error).
pub fn resolve_with(
    config: Config,
    ltcg: Option<Box<dyn Ltcg>>,
    errors: &mut Vec<LinkError>,
) -> Option<Resolved> {
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
        load_config_wanted: false,
        loading_from: None,
        loading_member: None,
        delay_helper_wanted: false,
        resources: Vec::new(),
        ltcg,
        stand_ins: Vec::new(),
        generated: false,
        t_index: std::time::Duration::ZERO,
        t_members: std::time::Duration::ZERO,
        t_add: std::time::Duration::ZERO,
    };
    let mut times = Vec::new();
    let mut lap = std::time::Instant::now();
    let image_base = l.intern(b"__ImageBase");
    l.globals[image_base as usize].def = Def::Synthetic(Synthetic::ImageBase);
    let volatile = l.intern(b"__volatile_metadata");
    l.globals[volatile as usize].def = Def::Synthetic(Synthetic::VolatileMetadata);
    // What the linker defines when nothing else does (after the libraries
    // were searched: a library's definition wins, as the CRT's of
    // __guard_xfg_check_icall_fptr).
    for &name in SYNTHETIC {
        let g = l.intern(name);
        l.globals[g as usize].optional = true;
    }

    // The command line's inputs: objects are read in parallel, then added
    // in order; libraries are searched later, in their order.
    // A file not found as named is looked for as libraries are (in each
    // /LIBPATH, then in LIB's directories): `kernel32.lib`.
    let inputs = l.config.inputs.clone();
    let (lib_paths, lib_env) = (&l.config.lib_paths, &l.lib_env);
    let read = turbonop_par::map_balanced(&inputs, 1, |path| {
        let mut p = PathBuf::from(path);
        if !p.is_file()
            && p.is_relative()
            && let Some(found) = lib_paths
                .iter()
                .chain(lib_env)
                .map(|d| d.join(path))
                .find(|c| c.is_file())
        {
            p = found;
        }
        let data = map(&p).map_err(|e| format!("cannot open file '{path}': {e}"))?;
        Ok(match identify(data) {
            InputKind::Library => Input::Library(p),
            InputKind::Resources => Input::Resources(data),
            _ => Input::Loaded(match load(data, path)? {
                LoadedKind::Il { data, .. } => LoadedKind::Il {
                    data,
                    path: Some(p),
                },
                kind => kind,
            }),
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
    let entry = entry_name.map(|n| l.intern(Box::leak(n.into_bytes().into_boxed_slice())));
    l.want_named(entry);

    times.push(("  libraries' indexes", lap.elapsed()));
    lap = std::time::Instant::now();
    l.search_libraries();
    if !l.stand_ins.is_empty() {
        times.push(("  library search (before code generation)", lap.elapsed()));
        lap = std::time::Instant::now();
        l.generate_code(entry);
        times.push(("  code generation, and the search after it", lap.elapsed()));
        lap = std::time::Instant::now();
    }
    for &name in SYNTHETIC {
        if let Some(&g) = l.names.get(name)
            && l.globals[g as usize].def == Def::Undefined
        {
            // (With /GUARD:CF, the tables the layout builds.)
            let guard = crate::guard::Symbol::of(name).filter(|_| l.config.guard_cf);
            l.globals[g as usize].def =
                Def::Synthetic(guard.map_or(Synthetic::Zero, Synthetic::Guard));
        }
    }
    // __guard_flags without /GUARD:CF: "instrumented" (0x100) if an
    // object was compiled for it (@feat.00's 0x800), as link.exe sets it.
    if !l.config.guard_cf
        && let Some(&g) = l.names.get(&b"__guard_flags"[..])
        && l.globals[g as usize].def == Def::Synthetic(Synthetic::Zero)
        && l.files.iter().any(|f| match &f.kind {
            FileKind::Object { object, .. } => object.feat_00.is_some_and(|v| v & 0x800 != 0),
            FileKind::Import { .. } => false,
        })
    {
        l.globals[g as usize].def = Def::Absolute(0x100);
    }
    times.push(("  library search", lap.elapsed()));
    times.push(("    of which: indexes", l.t_index));
    times.push(("    of which: members parsed", l.t_members));
    times.push(("  (all files' symbols added)", l.t_add));

    // A /DELAYLOAD DLL nothing imports from: link.exe's warning.
    for dll in &l.config.delay_load {
        let imported = l.files.iter().any(|f| match &f.kind {
            FileKind::Import { import, .. } => import.dll.eq_ignore_ascii_case(dll.as_bytes()),
            FileKind::Object { .. } => false,
        });
        if !imported {
            l.errors.push(LinkError::warning(
                4199,
                format!("/DELAYLOAD:{dll} ignored; no imports found from {dll}"),
            ));
        }
    }

    // Without /SUBSYSTEM, link.exe's inference: what the image defines
    // (main or wmain: the console; WinMain or wWinMain: Windows), then a
    // DLL's (Windows, entry point or not); an EXE's can't be inferred.
    if l.config.subsystem.is_none() {
        use crate::config::Subsystem;
        let defined = |n: &str| {
            l.names
                .get(n.as_bytes())
                .is_some_and(|&g| matches!(l.globals[g as usize].def, Def::Section { .. }))
        };
        l.config.subsystem = if defined("main") || defined("wmain") {
            Some(Subsystem::Console)
        } else if defined("WinMain") || defined("wWinMain") || l.config.dll {
            Some(Subsystem::Windows)
        } else {
            None
        };
        if l.config.subsystem.is_none() {
            l.errors.push(LinkError::error(
                1221,
                "a subsystem can't be inferred and must be defined".to_owned(),
            ));
        }
    }

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
    let library_order = (0..l.libraries.len())
        .filter(|&i| !l.libraries[i].default)
        .chain((0..l.libraries.len()).filter(|&i| l.libraries[i].default))
        .map(crate::n32)
        .collect();
    Some(Resolved {
        ltcg: l.generated,
        library_order,
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
