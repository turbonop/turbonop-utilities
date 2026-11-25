//! A dump opened: its target, its modules' files, its address space, its
//! modules parsed, and its threads' stacks (walked once, when first
//! asked for).
//!
//! A session's parts borrow from one another (the target from the dump's
//! bytes; the address space from the target and the modules' files), so a
//! session is made once and kept for the process's life: a dump doesn't
//! change, and `serve` answers every request on it from the same one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use turbonop_frames::{Modules, Stack};
use turbonop_mmap::Mmap;
use turbonop_target::load::{Hints, Loaded, load};
use turbonop_target::{AddressSpace, CoreTarget, MinidumpTarget, Target};

/// A dump, opened.
pub struct Session {
    pub path: PathBuf,
    pub target: &'static dyn Target,
    pub loaded: &'static Loaded,
    pub space: AddressSpace<'static>,
    pub modules: Modules<'static>,
    stacks: OnceLock<Vec<Stack>>,
}

impl Session {
    /// Every thread's stack, walked when first asked for (on every core).
    pub fn stacks(&self) -> &[Stack] {
        self.stacks
            .get_or_init(|| turbonop_frames::walk_all(self.target, &self.space, &self.modules))
    }

    /// The stack of thread `id`.
    pub fn stack(&self, id: u64) -> Option<&Stack> {
        self.stacks().iter().find(|s| s.thread == id)
    }
}

static SESSIONS: Mutex<Option<HashMap<PathBuf, &'static Session>>> = Mutex::new(None);

/// The session for the dump at `path`: opened if it hasn't been.
///
/// # Errors
///
/// If the file can't be read, or isn't a minidump or core file.
pub fn open(path: &Path, hints: &Hints) -> Result<&'static Session, String> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut sessions = SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let sessions = sessions.get_or_insert_with(HashMap::new);
    if let Some(s) = sessions.get(&key) {
        return Ok(s);
    }
    let data = Mmap::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data: &'static [u8] = Box::leak(Box::new(data));
    let target: &'static dyn Target = if data.starts_with(b"MDMP") {
        Box::leak(Box::new(
            MinidumpTarget::new(data).map_err(|e| format!("{}: {e}", path.display()))?,
        ))
    } else if data.starts_with(b"\x7fELF") {
        Box::leak(Box::new(
            CoreTarget::new(data).map_err(|e| format!("{}: {e}", path.display()))?,
        ))
    } else {
        return Err(format!("{}: not a minidump or a core file", path.display()));
    };
    let timing = std::env::var_os("DUMP_ANALYZER_TIMING").is_some();
    let t0 = std::time::Instant::now();
    let mut hints = hints.clone();
    // (The dump's own directory: its program and symbols are often there.)
    if let Some(dir) = path.parent() {
        hints.dirs.push(dir.to_path_buf());
    }
    let loaded: &'static Loaded = Box::leak(Box::new(load(target, &hints)));
    if timing {
        eprintln!("load: {:?}", t0.elapsed());
    }
    let space = loaded.address_space(target);
    if timing {
        eprintln!("address space: {:?}", t0.elapsed());
    }
    let bases: Vec<u64> = target.modules().iter().map(|m| m.base).collect();
    let modules = Modules::new(loaded, &space, &bases);
    if timing {
        eprintln!("modules: {:?}", t0.elapsed());
    }
    let session: &'static Session = Box::leak(Box::new(Session {
        path: path.to_path_buf(),
        target,
        loaded,
        space,
        modules,
        stacks: OnceLock::new(),
    }));
    sessions.insert(key, session);
    Ok(session)
}
