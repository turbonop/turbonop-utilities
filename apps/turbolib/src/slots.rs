//! Sharing the machine's cores among turbolib processes run at once (a
//! build running many archivers in parallel): each process takes free
//! *slots*, one per core, before a parallel stage, and runs that many
//! threads; a process that finds none runs on one thread. A build that
//! runs one archiver at a time still gets every core for it.
//!
//! A slot is a file in the temporary directory (`turbonop-slots/<n>`),
//! held by the process that has it locked: the lock goes when the process
//! does, however it ends, so a crashed process never keeps a slot.
//! Locking is never waited for. Taking slots costs an open and a lock
//! attempt per slot tried (some microseconds each on Windows), so small
//! libraries, whose work is on one thread anyway, take none.
//!
//! `TURBOLIB_OPTIONS=-j N` (or `/TURBOLIB:threads=N`, or
//! `TURBONOP_THREADS`) sets the number of threads instead: no slots are
//! taken.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;

/// Slots held: dropping it lets them go.
#[derive(Default)]
pub struct Slots {
    held: Vec<File>,
}

impl Slots {
    /// How many are held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Takes free slots until `want` are held (of `total`, the machine's
    /// cores), starting at a place that differs from process to process
    /// so that processes don't all try the same slots first. Returns how
    /// many are held. Slots that can't be made (no temporary directory)
    /// count as free: turbolib then runs as it would alone.
    pub fn take(&mut self, want: usize, total: usize) -> usize {
        match directory() {
            Some(dir) => self.take_in(&dir, want, total),
            None => want.min(total),
        }
    }

    fn take_in(&mut self, dir: &std::path::Path, want: usize, total: usize) -> usize {
        let want = want.min(total);
        if self.held.len() >= want {
            return self.held.len();
        }
        let start = std::process::id() as usize % total.max(1);
        for i in 0..total {
            if self.held.len() >= want {
                break;
            }
            let n = (start + i) % total;
            let Ok(file) = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(dir.join(n.to_string()))
            else {
                return want;
            };
            match file.try_lock() {
                Ok(()) => self.held.push(file),
                Err(TryLockError::WouldBlock) => {}
                // Locks not supported here: as if alone.
                Err(TryLockError::Error(_)) => return want,
            }
        }
        self.held.len()
    }

    /// Lets all but `keep` go.
    #[cfg(test)]
    pub fn release(&mut self, keep: usize) {
        self.held.truncate(keep);
    }
}

/// Below this much input, a library is made on one thread, and takes no
/// slot: its work is too little to split (and threads cost more than they
/// save).
pub const SMALL: u64 = 4 << 20;

/// Sets the threads `turbonop_par` runs for a library of `input` bytes:
/// one for a small one, otherwise one for each slot `slots` holds or
/// takes now (at least one, at most the machine's).
pub fn share(slots: &mut Slots, input: u64) {
    if input < SMALL {
        turbonop_par::set_limit(1);
        return;
    }
    let cores = turbonop_par::configured();
    let held = slots.take(cores, cores);
    turbonop_par::set_limit(held.max(1));
}

fn directory() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("turbonop-slots");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slots held by one taker are not free to another; let go, they are.
    #[test]
    fn slots_are_shared() {
        let dir = std::env::temp_dir().join(format!("turbolib-slots-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Slots::default();
        let mut b = Slots::default();
        assert_eq!(a.take_in(&dir, 3, 8), 3);
        assert_eq!(a.take_in(&dir, 2, 8), 3, "never fewer than held");
        assert_eq!(b.take_in(&dir, 8, 8), 5);
        a.release(1);
        assert_eq!(a.len(), 1);
        assert_eq!(b.take_in(&dir, 8, 8), 7);
        assert_eq!(b.take_in(&dir, 20, 8), 7, "no more than the total");
        drop(a);
        assert_eq!(b.take_in(&dir, 8, 8), 8);
        drop(b);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
