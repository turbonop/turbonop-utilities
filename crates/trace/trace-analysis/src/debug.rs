//! A [`Symbolizer`] from the debug information found on this machine.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use rustc_hash::FxHashMap;

use hexcavator_debuginfo::DebugInfo;
use hexcavator_image::Image;
use hexcavator_mmap::Mmap;
use hexcavator_symbolize::{FunctionTable, SymbolPath};
use hexcavator_trace::{ImageIx, Trace};

use crate::keys::Symbolizer;

/// What a function table is cached by: its PDB's identity (GUID and age),
/// or, for an image without one, its path, size, and timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CacheKey {
    Pdb([u8; 16], u32),
    Path(String, u64, u32),
}

/// Finds functions in the debug information of a trace's images, loading
/// each the first time one of its addresses is asked about: the PDB its
/// debug identity names; or else, from the image file itself (if it's
/// still there, and is the image the trace saw), its own DWARF (a PE image
/// MinGW built), the file its debug link names, or, without either, its
/// own symbols (exports, a COFF symbol table). Images none of these is
/// found for have no functions.
pub struct DebugSymbolizer {
    path: SymbolPath,
    /// By PDB identity (the same DLL is loaded in many processes), or path.
    tables: RefCell<FxHashMap<CacheKey, Option<Rc<FunctionTable>>>>,
}

/// [`DebugSymbolizer`]'s former name, from when it read PDBs only.
pub type PdbSymbolizer = DebugSymbolizer;

impl DebugSymbolizer {
    /// A symbolizer that searches `path` (and each image's recorded PDB
    /// path, and the image's directory).
    pub fn new(path: SymbolPath) -> Self {
        Self {
            path,
            tables: RefCell::new(FxHashMap::default()),
        }
    }

    /// The function table of `image`, if its debug information (or the
    /// image itself) is found.
    pub fn table(&self, trace: &Trace, image: ImageIx) -> Option<Rc<FunctionTable>> {
        let img = &trace.images[image.index()];
        let key = match &img.debug_id {
            Some(id) => CacheKey::Pdb(id.guid, id.age),
            None => CacheKey::Path(img.path.clone(), img.size, img.time_date_stamp),
        };
        if let Some(t) = self.tables.borrow().get(&key) {
            return t.clone();
        }
        let table = img
            .debug_id
            .as_ref()
            .and_then(|id| self.path.find_pdb(&id.pdb_path, &id.guid, id.age))
            // (Perhaps the path the trace records: see `open_named`.)
            .and_then(|p| Mmap::open_named(p).ok())
            .and_then(|map| FunctionTable::from_pdb(&map).ok())
            .or_else(|| self.table_of_file(img))
            .map(Rc::new);
        self.tables.borrow_mut().insert(key, table.clone());
        table
    }

    /// The table of the image file at the path the trace records, if it's
    /// the image the trace saw: from its debug information if found, or
    /// else its own symbols.
    fn table_of_file(&self, img: &hexcavator_trace::Image) -> Option<FunctionTable> {
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

    /// How many images' debug information was looked for, and how many
    /// found.
    pub fn loaded(&self) -> (usize, usize) {
        let t = self.tables.borrow();
        (t.len(), t.values().filter(|v| v.is_some()).count())
    }
}

/// Whether `image` (a file) is the image the trace saw: of its size in
/// memory, with its timestamp and checksum (a PE image's), and its debug
/// identity, where the trace records them.
fn is_same_image(image: &Image<'_>, seen: &hexcavator_trace::Image) -> bool {
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
        hexcavator_image::DebugId::Pdb { guid, age, .. } => *guid == id.guid && *age == id.age,
        _ => false,
    })
}

impl Symbolizer for DebugSymbolizer {
    fn function(&self, trace: &Trace, image: ImageIx, offset: u64) -> Option<(u64, String)> {
        let table = self.table(trace, image)?;
        let (start, name) = table.function(offset)?;
        Some((start, name.to_string()))
    }

    fn inlines(&self, trace: &Trace, image: ImageIx, offset: u64) -> Vec<(u32, String)> {
        let Some(table) = self.table(trace, image) else {
            return Vec::new();
        };
        table
            .inlines(offset)
            .into_iter()
            .map(|(site, name)| (site, name.to_string()))
            .collect()
    }

    fn inline_name(&self, trace: &Trace, image: ImageIx, site: u32) -> Option<String> {
        self.table(trace, image)?
            .site_name(site)
            .map(str::to_string)
    }

    fn line(&self, trace: &Trace, image: ImageIx, offset: u64) -> Option<(String, u32)> {
        let table = self.table(trace, image)?;
        let (file, line) = table.line(offset)?;
        Some((file.to_string(), line))
    }
}
