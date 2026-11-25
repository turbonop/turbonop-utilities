//! A [`Symbolizer`] from the debug information found on this machine.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use rustc_hash::FxHashMap;

use turbonop_debuginfo::DebugInfo;
use turbonop_image::Image;
use turbonop_mmap::Mmap;
use turbonop_symbolize::{FunctionTable, SymbolPath};
use turbonop_trace::{ImageIx, Trace};

use crate::keys::Symbolizer;

/// A PDB's identity: GUID and age.
type DebugKey = ([u8; 16], u32);

/// An image file's identity, as the trace records it: path, size, and
/// timestamp.
type FileKey = (String, u64, u32);

/// Finds functions in the debug information of a trace's images, loading
/// each the first time one of its addresses is asked about: the PDB its
/// debug identity names; or else, from the image file itself (if it's
/// still there, and is the image the trace saw), its own DWARF (a PE image
/// MinGW built), the file its debug link names, or, without either, its
/// own symbols (exports, a COFF symbol table). Images none of these is
/// found for have no functions. Threads may share it.
pub struct DebugSymbolizer {
    path: SymbolPath,
    /// PDBs by identity: the same DLL is loaded in many processes. Each is
    /// loaded once: a thread that wants one being loaded waits for it.
    tables: RwLock<FxHashMap<DebugKey, Slot>>,
    /// Image files' own tables, by file, loaded once each likewise.
    files: RwLock<FxHashMap<FileKey, Slot>>,
    /// Whether lookups wait for a table to load (else they use only those
    /// loaded already: see [`DebugSymbolizer::set_waiting`]).
    waiting: AtomicBool,
    /// By image: what lookups found, so that later lookups take no lock
    /// (a tree's threads look up millions of frames at once).
    by_image: OnceLock<Vec<ImageCache>>,
    /// Bumped whenever a PDB is loaded, seeded or forgotten: an image
    /// found missing at an older generation is looked for again.
    generation: AtomicU64,
}

/// [`DebugSymbolizer`]'s former name, from when it read PDBs only.
pub type PdbSymbolizer = DebugSymbolizer;

/// What lookups found for an image.
#[derive(Default)]
struct ImageCache {
    /// Its PDB's table, once found, with the PDB's identity (checked on
    /// each use). A table found stays found.
    found: OnceLock<(DebugKey, Arc<FunctionTable>)>,
    /// The generation (plus one) at which its PDB was found missing; 0 if
    /// not.
    missing_at: AtomicU64,
    /// Its file's own table (or none), once looked for: used while its
    /// PDB is missing.
    file: OnceLock<Option<Arc<FunctionTable>>>,
    /// The generation (plus one) at which its file's table wasn't read
    /// yet (by a lookup that doesn't wait); 0 if not.
    file_missing_at: AtomicU64,
}

/// A table's slot in a [`DebugSymbolizer`].
type Slot = Arc<OnceLock<Option<Arc<FunctionTable>>>>;

impl DebugSymbolizer {
    /// A symbolizer that searches `path` (and each image's recorded PDB
    /// path, and the image's directory).
    pub fn new(path: SymbolPath) -> Self {
        Self {
            path,
            tables: RwLock::new(FxHashMap::default()),
            files: RwLock::new(FxHashMap::default()),
            waiting: AtomicBool::new(true),
            by_image: OnceLock::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// The function table of `image`, if its debug information (or the
    /// image itself) is found.
    pub fn table(&self, trace: &Trace, image: ImageIx) -> Option<Arc<FunctionTable>> {
        self.pdb_table(trace, image)
            .or_else(|| self.file_table(trace, image, self.waiting.load(Ordering::Relaxed)))
    }

    /// The table of `image`'s PDB, if found: loaded if lookups wait, else
    /// only if loaded already.
    fn pdb_table(&self, trace: &Trace, image: ImageIx) -> Option<Arc<FunctionTable>> {
        let id = trace.images[image.index()].debug_id.as_ref()?;
        if self.waiting.load(Ordering::Relaxed) {
            return self.load(&id.pdb_path, &id.guid, id.age);
        }
        self.pdbs().get(&(id.guid, id.age))?.get()?.clone()
    }

    /// The table of `image`'s own file, if found: read from the file if
    /// `read` (once for each file, however many processes loaded it), else
    /// only if read already.
    fn file_table(&self, trace: &Trace, image: ImageIx, read: bool) -> Option<Arc<FunctionTable>> {
        let img = &trace.images[image.index()];
        let key = (img.path.clone(), img.size, img.time_date_stamp);
        if !read {
            return self.files_read().get(&key)?.get()?.clone();
        }
        let slot: Slot = Arc::clone(self.files_write().entry(key).or_default());
        slot.get_or_init(|| self.table_of_file(img).map(Arc::new))
            .clone()
    }

    /// Reads the table of `image`'s own file, if not read yet (as a
    /// background preload does for PDBs: [`Self::load`]), so that lookups
    /// that don't wait have it. Returns whether one was found.
    pub fn load_file(&self, trace: &Trace, image: ImageIx) -> bool {
        let found = self.file_table(trace, image, true).is_some();
        if found {
            // (Lookups that found it missing look again.)
            self.generation.fetch_add(1, Ordering::Release);
        }
        found
    }

    /// The table of the image file at the path the trace records, if it's
    /// the image the trace saw: from its debug information if found, or
    /// else its own symbols.
    fn table_of_file(&self, img: &turbonop_trace::Image) -> Option<FunctionTable> {
        // (The trace names the path: only a local regular file is opened.)
        let map = Mmap::open_named(Path::new(&img.path)).ok()?;
        let image = Image::parse(&map).ok()?;
        if !is_same_image(&image, img) {
            return None;
        }
        let from_debug = self
            .path
            .find_debug(Path::new(&img.path), &image)
            .and_then(|p| {
                let debug = Mmap::open_named(&p).ok()?;
                let info = DebugInfo::parse(&debug).ok()?;
                FunctionTable::from_debug(&info, Some(&image)).ok()
            });
        let table = from_debug.unwrap_or_else(|| FunctionTable::from_image(&image));
        (!table.is_empty()).then_some(table)
    }

    /// `f` of `image`'s function table, if found: as [`Self::table`], but
    /// a table found already is only borrowed (lookups on many threads at
    /// once share nothing they write).
    fn with_table<R>(
        &self,
        trace: &Trace,
        image: ImageIx,
        f: impl FnOnce(&FunctionTable) -> R,
    ) -> Option<R> {
        let cache = self.by_image.get_or_init(|| {
            (0..trace.images.len())
                .map(|_| ImageCache::default())
                .collect()
        });
        let cached = cache.get(image.index());
        let key = trace.images[image.index()]
            .debug_id
            .as_ref()
            .map(|id| (id.guid, id.age));
        if let Some((k, table)) = cached.and_then(|c| c.found.get())
            && Some(*k) == key
        {
            return Some(f(table));
        }
        // The PDB missing, and nothing loaded since: still missing.
        let generation = self.generation.load(Ordering::Acquire);
        let pdb_missing =
            cached.is_some_and(|c| c.missing_at.load(Ordering::Relaxed) == generation + 1);
        if !pdb_missing {
            if let Some(table) = self.pdb_table(trace, image) {
                if let (Some(c), Some(k)) = (cached, key) {
                    let _ = c.found.set((k, Arc::clone(&table)));
                }
                return Some(f(&table));
            }
            if let Some(c) = cached {
                c.missing_at.store(generation + 1, Ordering::Relaxed);
            }
        }
        // Without its PDB: its file's own table. Read from the file only
        // if lookups wait; else only once read (by `load_file`), and not
        // looked for again until something is loaded.
        let waiting = self.waiting.load(Ordering::Relaxed);
        let file = match cached {
            Some(c) => match c.file.get() {
                Some(t) => t.clone(),
                None if c.file_missing_at.load(Ordering::Relaxed) == generation + 1 => None,
                None => {
                    let t = self.file_table(trace, image, waiting);
                    if t.is_some() || waiting {
                        let _ = c.file.set(t.clone());
                    } else {
                        c.file_missing_at.store(generation + 1, Ordering::Relaxed);
                    }
                    t
                }
            },
            None => self.file_table(trace, image, waiting),
        };
        file.map(|t| f(&t))
    }

    /// Whether lookups wait for a PDB to load (they do, at first). Without
    /// waiting, a lookup uses only the PDBs loaded already ([`Self::load`]
    /// loads them, as a background preload does), so that a query answers
    /// at once and a later one has the names.
    pub fn set_waiting(&self, waiting: bool) {
        self.waiting.store(waiting, Ordering::Relaxed);
        // (What wasn't loaded yet is worth waiting for now.)
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Whether the PDB with `guid` and `age` was found (`Some(true)`:
    /// loaded or seeded), looked for and not found (`Some(false)`), or not
    /// looked for yet (`None`).
    pub fn status(&self, guid: &[u8; 16], age: u32) -> Option<bool> {
        self.pdbs()
            .get(&(*guid, age))
            .and_then(|slot| slot.get().map(Option::is_some))
    }

    /// How many PDBs are loaded and found.
    pub fn loaded_count(&self) -> usize {
        self.pdbs()
            .values()
            .filter(|v| v.get().is_some_and(Option::is_some))
            .count()
    }

    /// The function table of the PDB recorded at `pdb_path` with `guid`
    /// and `age`, if found: loaded the first time it is asked for (by one
    /// thread; others asking meanwhile wait for it, and others' other
    /// PDBs load at once).
    pub fn load(&self, pdb_path: &str, guid: &[u8; 16], age: u32) -> Option<Arc<FunctionTable>> {
        let slot: Slot = Arc::clone(self.pdbs_mut().entry((*guid, age)).or_default());
        slot.get_or_init(|| {
            let t = self
                .path
                .find_pdb(pdb_path, guid, age)
                // (Perhaps the path the trace records: see `open_named`.)
                .and_then(|p| Mmap::open_named(p).ok())
                .and_then(|map| FunctionTable::from_pdb(&map).ok())
                .map(Arc::new);
            if t.is_some() {
                self.generation.fetch_add(1, Ordering::Release);
            }
            t
        })
        .clone()
    }

    /// Gives the PDB with `guid` and `age` the table `table` (one saved
    /// for this trace: see [`FunctionTable::subset`]) instead of loading
    /// it. A PDB loaded or seeded already keeps what it has.
    pub fn seed(&self, guid: &[u8; 16], age: u32, table: Arc<FunctionTable>) {
        let slot: Slot = Arc::clone(self.pdbs_mut().entry((*guid, age)).or_default());
        if slot.set(Some(table)).is_ok() {
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// The PDBs loaded (or seeded) and found: each one's GUID, age and
    /// table, in GUID order.
    pub fn found(&self) -> Vec<([u8; 16], u32, Arc<FunctionTable>)> {
        let mut out: Vec<_> = self
            .pdbs()
            .iter()
            .filter_map(|(&(guid, age), slot)| Some((guid, age, Arc::clone(slot.get()?.as_ref()?))))
            .collect();
        out.sort_by_key(|a| (a.0, a.1));
        out
    }

    /// The PDBs, to read: a lookup's (many threads at once).
    fn pdbs(&self) -> std::sync::RwLockReadGuard<'_, FxHashMap<DebugKey, Slot>> {
        self.tables.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The PDBs, to add or remove one.
    fn pdbs_mut(&self) -> std::sync::RwLockWriteGuard<'_, FxHashMap<DebugKey, Slot>> {
        self.tables.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn files_read(&self) -> std::sync::RwLockReadGuard<'_, FxHashMap<FileKey, Slot>> {
        self.files.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn files_write(&self) -> std::sync::RwLockWriteGuard<'_, FxHashMap<FileKey, Slot>> {
        self.files.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets the PDBs that weren't found, so that they are looked for
    /// again (after a download).
    pub fn forget_missing(&self) {
        self.pdbs_mut()
            .retain(|_, t| t.get().is_none_or(Option::is_some));
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// How many images' debug information was looked for (PDBs, and image
    /// files read for their own), and how many found.
    pub fn loaded(&self) -> (usize, usize) {
        fn found<'a>(slots: impl Iterator<Item = &'a Slot>) -> usize {
            slots
                .filter(|v| v.get().is_some_and(Option::is_some))
                .count()
        }
        let (pdbs, files) = (self.pdbs(), self.files_read());
        (
            pdbs.len() + files.len(),
            found(pdbs.values()) + found(files.values()),
        )
    }
}

/// Whether `image` (a file) is the image the trace saw: of its size in
/// memory, with its timestamp and checksum (a PE image's), and its debug
/// identity, where the trace records them.
fn is_same_image(image: &Image<'_>, seen: &turbonop_trace::Image) -> bool {
    if let Some(pe) = image.pe()
        && let Some(h) = pe.optional_header()
    {
        if seen.size != 0 && u64::from(h.size_of_image()) != seen.size {
            return false;
        }
        if seen.time_date_stamp != 0
            && pe.file_header().time_date_stamp.get() != seen.time_date_stamp
        {
            return false;
        }
        if seen.checksum != 0 && h.check_sum() != seen.checksum {
            return false;
        }
    }
    let Some(id) = &seen.debug_id else {
        return true;
    };
    image.debug_ids().iter().any(|d| match d {
        turbonop_image::DebugId::Pdb { guid, age, .. } => *guid == id.guid && *age == id.age,
        _ => false,
    })
}

impl Symbolizer for DebugSymbolizer {
    fn function(&self, trace: &Trace, image: ImageIx, offset: u64) -> Option<(u64, String)> {
        self.with_table(trace, image, |t| {
            t.function(offset)
                .map(|(start, name)| (start, name.to_string()))
        })?
    }

    fn function_start(&self, trace: &Trace, image: ImageIx, offset: u64) -> Option<u64> {
        self.with_table(trace, image, |t| t.function(offset).map(|(start, _)| start))?
    }

    fn inline_sites(&self, trace: &Trace, image: ImageIx, offset: u64) -> Vec<u32> {
        self.with_table(trace, image, |t| {
            t.inlines(offset)
                .into_iter()
                .map(|(site, _)| site)
                .collect()
        })
        .unwrap_or_default()
    }

    fn inlines(&self, trace: &Trace, image: ImageIx, offset: u64) -> Vec<(u32, String)> {
        self.with_table(trace, image, |t| {
            t.inlines(offset)
                .into_iter()
                .map(|(site, name)| (site, name.to_string()))
                .collect()
        })
        .unwrap_or_default()
    }

    fn inline_name(&self, trace: &Trace, image: ImageIx, site: u32) -> Option<String> {
        self.with_table(trace, image, |t| t.site_name(site).map(str::to_string))?
    }

    fn line(&self, trace: &Trace, image: ImageIx, offset: u64) -> Option<(String, u32)> {
        self.with_table(trace, image, |t| {
            t.line(offset).map(|(file, line)| (file.to_string(), line))
        })?
    }
}
