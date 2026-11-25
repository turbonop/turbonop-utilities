//! Finding an image's symbols, and the function at an address.
//!
//! [`SymbolPath`] finds the debug information that matches an image,
//! looking only on this machine: the PDB its debug identity (GUID and age)
//! names, at the path the image recorded or in directories laid out like a
//! symbol store or a flat folder; or, for an image of any format, its own
//! DWARF, or the file its debug link or build ID names (see
//! [`SymbolPath::find_debug`]). It never downloads anything: [`server`]
//! does that, when asked, into a symbol server's local cache, which works
//! if `_NT_SYMBOL_PATH` names it. What's found is checked against the
//! identity before it is used.
//!
//! [`FunctionTable`] is an image's functions sorted by address, from its
//! debug information (a PDB or DWARF) and its own symbols, with
//! undecorated names, for finding the function an address is in, and the
//! calls inlined there and its source line.

use std::path::{Path, PathBuf};

mod saved;
pub mod server;

use turbonop_core::Result;
use turbonop_debuginfo::DebugInfo;
use turbonop_demangle::Demangler;
use turbonop_image::Image;
use turbonop_pdb::Pdb;

/// Where to look for symbol files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SymbolPath {
    /// Directories to search, in order.
    pub dirs: Vec<PathBuf>,
}

impl SymbolPath {
    /// The local directories of `_NT_SYMBOL_PATH`, if set.
    pub fn from_env() -> Self {
        std::env::var("_NT_SYMBOL_PATH").map_or_else(|_| Self::default(), |v| Self::parse(&v))
    }

    /// The local directories of a symbol path: plain directories, and the
    /// caches of `srv*` and `cache*` entries (URLs are skipped).
    pub fn parse(path: &str) -> Self {
        Self {
            dirs: turbonop_debuginfo::nt_symbol_dirs(path),
        }
    }

    /// The PDB with `guid` and `age`, recorded at `pdb_path` when the image
    /// was built: there, or in a directory of the symbol path (as
    /// `name.pdb/GUIDAGE/name.pdb` or `name.pdb`).
    pub fn find_pdb(&self, pdb_path: &str, guid: &[u8; 16], age: u32) -> Option<PathBuf> {
        let name = pdb_path
            .rsplit(['\\', '/'])
            .next()
            .filter(|n| !n.is_empty())?;
        let key = symbol_store_key(guid, age);
        let mut candidates = vec![PathBuf::from(pdb_path)];
        for dir in &self.dirs {
            candidates.push(dir.join(name).join(&key).join(name));
            candidates.push(dir.join(name));
        }
        candidates.into_iter().find(|p| matches(p, guid, age))
    }

    /// The debug information of the image at `image_path` (`image`): its
    /// own (DWARF: an ELF, Mach-O, or WebAssembly image's, or a PE image
    /// MinGW built), or else the first file one of its debug identities
    /// names (a PDB, a debug link, a build ID), beside it or in the symbol
    /// path, that matches it. (Only local regular files are opened: see
    /// [`SymbolPath::find_pdb`].)
    pub fn find_debug(&self, image_path: &Path, image: &Image<'_>) -> Option<PathBuf> {
        if DebugInfo::parse(image.data()).is_ok() {
            return Some(image_path.to_path_buf());
        }
        for id in image.debug_ids() {
            for candidate in turbonop_debuginfo::candidate_paths_in(image_path, &id, &self.dirs) {
                let Ok(map) = turbonop_mmap::Mmap::open_named(&candidate) else {
                    continue;
                };
                if DebugInfo::parse(&map).is_ok_and(|d| d.matches(&id)) {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

pub use turbonop_debuginfo::symbol_store_key;

/// Whether the PDB at `path` has `guid` and `age`. (The first candidate
/// is the path the trace records: a hostile trace could name a network
/// share, a device, or a FIFO, so only a local regular file is opened.)
fn matches(path: &Path, guid: &[u8; 16], age: u32) -> bool {
    let Ok(map) = turbonop_mmap::Mmap::open_named(path) else {
        return false;
    };
    let Ok(pdb) = Pdb::parse(&map) else {
        return false;
    };
    let info = pdb.info();
    // The image records the PDB's age when it was linked; a PDB can be
    // newer (updated later), never older.
    info.guid() == *guid && info.age() >= age
}

/// An image's functions, by address (the image's: a PE image's RVAs, as a
/// trace records an address's offset in its image): its debug
/// information's (procedures, with their code ranges; and, for a PDB,
/// public symbols for code no procedure covers), and the image's own
/// symbols (exports, a COFF or ELF symbol table) for code the debug
/// information doesn't describe (each running to the next function).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionTable {
    /// (start, end, name), sorted by start, not overlapping.
    entries: Vec<(u64, u64, String)>,
    /// Inlined code: (start, end, depth, site), sorted by start. Nested
    /// sites' ranges lie within their callers'.
    inlines: Vec<(u64, u64, u32, u32)>,
    /// Each inline site's inlined function's name.
    site_names: Vec<String>,
    /// Source lines: (start, end, file, line), sorted by start.
    lines: Vec<(u64, u64, u32, u32)>,
    /// The source files.
    files: Vec<String>,
}

impl FunctionTable {
    /// Reads the functions of the PDB in `data`.
    ///
    /// # Errors
    ///
    /// If `data` isn't a PDB, or its symbols can't be read.
    pub fn from_pdb(data: &[u8]) -> Result<Self> {
        Pdb::parse(data)?;
        let info = DebugInfo::parse(data)?;
        Self::from_debug(&info, None)
    }

    /// Reads the functions of any debug information (a PDB, or DWARF: an
    /// image's own, or its separate file's), with `image`'s own symbols
    /// for the code it doesn't describe.
    ///
    /// # Errors
    ///
    /// If the debug information's symbols can't be read.
    pub fn from_debug(info: &DebugInfo<'_>, image: Option<&Image<'_>>) -> Result<Self> {
        use turbonop_debuginfo::SymbolKind as K;
        let symbols = info.symbols()?;
        let mut demangler = Demangler::new();
        let mut procs: Vec<(u64, u64, String)> = Vec::new();
        let mut publics: Vec<(u64, String)> = Vec::new();
        for s in &symbols {
            match s.kind {
                K::Function | K::SeparatedBlock | K::Thunk => {
                    let size = s.size.unwrap_or(0).max(1);
                    procs.push((s.addr, s.addr.saturating_add(size), s.name.clone()));
                }
                K::Public => publics.push((s.addr, undecorate(&mut demangler, &s.name))),
                _ => {}
            }
        }
        if let Some(image) = image {
            publics.extend(image_functions(image, &mut demangler));
        }
        let mut table = Self::build(procs, publics);
        // Inline sites and lines. Debug information without them still
        // has its functions.
        if let Ok(sites) = info.inline_sites() {
            for (i, site) in sites.sites.iter().enumerate() {
                let Ok(index) = u32::try_from(i) else { break };
                let name = sites
                    .inlinees
                    .get(site.inlinee as usize)
                    .cloned()
                    .unwrap_or_default();
                table.site_names.push(name);
                for &(addr, size) in &site.ranges {
                    table
                        .inlines
                        .push((addr, addr.saturating_add(size), site.depth, index));
                }
            }
            table.inlines.sort_unstable();
        }
        if let Ok(lines) = info.source_lines() {
            table.files = lines.files;
            for span in &lines.spans {
                table.lines.push((
                    span.addr,
                    span.addr.saturating_add(span.size),
                    span.file,
                    span.line,
                ));
            }
            table.lines.sort_unstable();
        }
        Ok(table)
    }

    /// An image's functions from its own symbols alone (exports, a COFF
    /// symbol table, ELF's, Mach-O's), each running to the next: for an
    /// image whose debug information isn't found.
    pub fn from_image(image: &Image<'_>) -> Self {
        let mut demangler = Demangler::new();
        Self::build(Vec::new(), image_functions(image, &mut demangler))
    }

    /// The table of `procs` (with their ranges) and, where none covers
    /// them, `publics` (each running to the next function of either kind).
    fn build(mut procs: Vec<(u64, u64, String)>, mut publics: Vec<(u64, String)>) -> Self {
        procs.sort_by_key(|e| (e.0, std::cmp::Reverse(e.1)));
        procs.dedup_by_key(|e| e.0);
        publics.retain(|&(addr, _)| {
            let i = procs.partition_point(|p| p.0 <= addr);
            i == 0 || addr >= procs[i - 1].1
        });
        // (At an address named twice, the first: the debug information's
        // publics before the image's own symbols.)
        publics.sort_by_key(|e| e.0);
        publics.dedup_by_key(|e| e.0);
        let mut starts: Vec<u64> = procs
            .iter()
            .map(|e| e.0)
            .chain(publics.iter().map(|p| p.0))
            .collect();
        starts.sort_unstable();
        let mut entries = procs;
        for (addr, name) in publics {
            let next = starts[starts.partition_point(|&s| s <= addr)..]
                .first()
                .copied()
                .unwrap_or(u64::MAX);
            entries.push((addr, next, name));
        }
        entries.sort_by_key(|e| e.0);
        // Nested or overlapping ranges (rare) end where the next begins.
        for i in 1..entries.len() {
            let next = entries[i].0;
            if entries[i - 1].1 > next {
                entries[i - 1].1 = next;
            }
        }
        Self {
            entries,
            ..Self::default()
        }
    }

    /// The inlined calls at `addr`, outermost first: each one's site and
    /// inlined function's name.
    pub fn inlines(&self, addr: u64) -> Vec<(u32, &str)> {
        // Sites lie within their function, so only ranges starting in the
        // function (at or before `addr`) can cover it.
        let Some((start, _)) = self.function(addr) else {
            return Vec::new();
        };
        let lo = self.inlines.partition_point(|r| r.0 < start);
        let hi = self.inlines.partition_point(|r| r.0 <= addr);
        let mut found: Vec<(u32, u32)> = self.inlines[lo..hi]
            .iter()
            .filter(|r| addr < r.1)
            .map(|r| (r.2, r.3))
            .collect();
        found.sort_unstable();
        found.dedup_by_key(|f| f.0);
        found
            .into_iter()
            .map(|(_, site)| (site, self.site_names[site as usize].as_str()))
            .collect()
    }

    /// The name of an inline site's inlined function.
    pub fn site_name(&self, site: u32) -> Option<&str> {
        self.site_names.get(site as usize).map(String::as_str)
    }

    /// The source file and line of `addr`.
    pub fn line(&self, addr: u64) -> Option<(&str, u32)> {
        let i = self.lines.partition_point(|l| l.0 <= addr);
        let &(_, end, file, line) = self.lines.get(i.checked_sub(1)?)?;
        (addr < end).then(|| {
            (
                self.files.get(file as usize).map_or("", String::as_str),
                line,
            )
        })
    }

    /// The number of functions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The functions: start, end and name, by address.
    pub fn iter(&self) -> impl Iterator<Item = (u64, u64, &str)> {
        self.entries.iter().map(|(s, e, n)| (*s, *e, n.as_str()))
    }

    /// The function containing `addr`: its start and name.
    pub fn function(&self, addr: u64) -> Option<(u64, &str)> {
        let i = self.entries.partition_point(|e| e.0 <= addr);
        let (start, end, name) = self.entries.get(i.checked_sub(1)?)?;
        (addr < *end).then_some((*start, name.as_str()))
    }
}

/// The image's own code symbols (exports, a COFF symbol table, ELF's,
/// Mach-O's), undecorated, by address.
fn image_functions(image: &Image<'_>, demangler: &mut Demangler) -> Vec<(u64, String)> {
    let macho = image.macho().is_some();
    image
        .symbols()
        .into_iter()
        .filter(|s| s.kind == turbonop_image::SymbolKind::Function && !s.name.is_empty())
        .map(|s| {
            let raw = String::from_utf8_lossy(s.name);
            let raw = match raw.strip_prefix('_') {
                // (Mach-O's C names have a `_` before them; its mangled
                // names keep theirs for the demangler.)
                Some(c) if macho && !c.starts_with('_') && !c.starts_with('$') => c,
                _ => &raw,
            };
            (s.addr, undecorate(demangler, raw))
        })
        .collect()
}

/// A symbol's name as a tree shows it: MSVC names undecorated to the
/// qualified name only, Rust names demangled (without legacy names'
/// hashes), other C++ names demangled, C names as they are.
fn undecorate(demangler: &mut Demangler, raw: &str) -> String {
    let mut out = String::new();
    if !raw.starts_with('?')
        && (raw.starts_with("_ZN") || raw.starts_with("_R"))
        && turbonop_demangle::rust::demangle_into(raw, &mut out)
    {
        // Rust's legacy names end in a hash; trees read better without it.
        if let Some(i) = out.rfind("::h")
            && out[i + 3..].len() == 16
            && out[i + 3..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            out.truncate(i);
        }
        return out;
    }
    out.clear();
    if demangler.demangle_into(raw, turbonop_demangle::msvc::flags::NAME_ONLY, &mut out) {
        return out;
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDB path a trace records is the trace's to choose: one naming a
    /// share, a device, or a namespace isn't opened (on Windows, opening a
    /// share would connect to it, offering the user's credentials).
    #[test]
    fn recorded_paths_naming_shares_are_not_opened() {
        let path = SymbolPath::default();
        for pdb in [
            r"\\attacker.invalid\share\app.pdb",
            "//attacker.invalid/share/app.pdb",
            r"\\.\pipe\app.pdb",
            r"\\?\UNC\attacker.invalid\share\app.pdb",
        ] {
            assert_eq!(path.find_pdb(pdb, &[0; 16], 1), None, "{pdb}");
        }
    }

    #[test]
    fn parses_symbol_paths() {
        let p = SymbolPath::parse(
            r"srv*C:\symbols*https://msdl.microsoft.com/download/symbols; C:\mine ;cache*D:\cache;;",
        );
        assert_eq!(
            p.dirs,
            [
                PathBuf::from(r"C:\symbols"),
                PathBuf::from(r"C:\mine"),
                PathBuf::from(r"D:\cache")
            ]
        );
        assert_eq!(SymbolPath::parse(""), SymbolPath::default());
    }

    #[test]
    fn symbol_store_keys() {
        // {3844DBB9-2017-4967-BE79-96A1D5D8A60D}, age 1.
        let guid = [
            0xb9, 0xdb, 0x44, 0x38, 0x17, 0x20, 0x67, 0x49, 0xbe, 0x79, 0x96, 0xa1, 0xd5, 0xd8,
            0xa6, 0x0d,
        ];
        assert_eq!(
            symbol_store_key(&guid, 1),
            "3844DBB920174967BE7996A1D5D8A60D1"
        );
        assert_eq!(
            symbol_store_key(&guid, 0x1a),
            "3844DBB920174967BE7996A1D5D8A60D1A"
        );
    }

    #[test]
    fn finds_nothing_that_is_not_there() {
        let p = SymbolPath::parse(r"C:\no\such\dir");
        assert_eq!(p.find_pdb(r"C:\no\such\file.pdb", &[0; 16], 1), None);
        assert_eq!(p.find_pdb("", &[0; 16], 1), None);
    }

    #[test]
    fn function_lookup() {
        let t = FunctionTable {
            entries: vec![(0x1000, 0x1080, "a".into()), (0x1100, 0x1200, "b".into())],
            ..FunctionTable::default()
        };
        assert_eq!(t.function(0xfff), None);
        assert_eq!(t.function(0x1000), Some((0x1000, "a")));
        assert_eq!(t.function(0x107f), Some((0x1000, "a")));
        // Between functions: in none.
        assert_eq!(t.function(0x1080), None);
        assert_eq!(t.function(0x11ff), Some((0x1100, "b")));
        assert_eq!(t.function(0x1200), None);
    }

    #[test]
    fn inlines_and_lines() {
        // a (0x1000..0x1080) inlines x at 0x1010..0x1040, which inlines y
        // at 0x1020..0x1030.
        let t = FunctionTable {
            entries: vec![(0x1000, 0x1080, "a".into())],
            inlines: vec![(0x1010, 0x1040, 0, 0), (0x1020, 0x1030, 1, 1)],
            site_names: vec!["x".into(), "y".into()],
            lines: vec![(0x1000, 0x1020, 0, 10), (0x1020, 0x1080, 0, 11)],
            files: vec!["a.cpp".into()],
        };
        assert!(t.inlines(0x1008).is_empty());
        assert_eq!(t.inlines(0x1010), [(0, "x")]);
        assert_eq!(t.inlines(0x1025), [(0, "x"), (1, "y")]);
        assert_eq!(t.inlines(0x1030), [(0, "x")]);
        assert!(t.inlines(0x1040).is_empty());
        assert!(t.inlines(0x2000).is_empty());
        assert_eq!(t.site_name(1), Some("y"));
        assert_eq!(t.site_name(2), None);
        assert_eq!(t.line(0x101f), Some(("a.cpp", 10)));
        assert_eq!(t.line(0x1020), Some(("a.cpp", 11)));
        assert_eq!(t.line(0x1080), None);
    }

    #[test]
    fn undecorates() {
        let mut d = Demangler::new();
        assert_eq!(undecorate(&mut d, "?f@C@@QEAAXH@Z"), "C::f");
        assert_eq!(
            undecorate(&mut d, "_ZN4core3ptr13drop_in_place17h0123456789abcdefE"),
            "core::ptr::drop_in_place"
        );
        assert_eq!(undecorate(&mut d, "memcpy"), "memcpy");
        assert_eq!(undecorate(&mut d, "_Z1fi"), "f(int)");
    }
}

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(all(test, not(target_family = "wasm")))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
