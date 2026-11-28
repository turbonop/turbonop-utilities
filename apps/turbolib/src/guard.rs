//! Running folding so that nothing it does can break a build: a panic, on
//! any thread, becomes an error naming where it happened, printed by the
//! caller as one line (the panic's own message isn't printed), and the
//! library is then written without folding.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

/// How many [`catch`]es are running: while any is, panics are recorded,
/// not printed.
static CATCHING: AtomicUsize = AtomicUsize::new(0);

/// The first panic recorded while catching.
static FIRST: Mutex<Option<String>> = Mutex::new(None);

static HOOK: Once = Once::new();

fn install_hook() {
    HOOK.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            // (A test binary runs other tests while one catches: their
            // panics are printed too.)
            if CATCHING.load(Ordering::SeqCst) == 0 || cfg!(test) {
                default(info);
                if CATCHING.load(Ordering::SeqCst) == 0 {
                    return;
                }
            }
            let payload = info.payload();
            let text = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".to_string());
            let at = info
                .location()
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            let mut first = FIRST
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if first.is_none() {
                *first = Some(format!("{}{at}", one_line(&text)));
            }
        }));
    });
}

/// `text` on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `f`'s result, or, if it panicked (on this thread or one it spawned and
/// joined), "panic: <message> at <file>:<line>".
pub fn catch<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    install_hook();
    CATCHING.fetch_add(1, Ordering::SeqCst);
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    CATCHING.fetch_sub(1, Ordering::SeqCst);
    // (Taken only by a catch that caught one: in one process, one fold
    // runs at a time, but tests run several.)
    result.map_err(|_| {
        let first = FIRST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        format!("panic: {}", first.unwrap_or_else(|| "(no message)".into()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_on_a_worker_thread_is_caught_and_named() {
        let r = catch(|| {
            std::thread::scope(|s| {
                let h = s.spawn(|| -> u32 { panic!("bad\nmember {}", 7) });
                h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
            })
        });
        let e = r.unwrap_err();
        assert!(e.starts_with("panic: bad member 7 at "), "{e}");
        assert!(e.contains("guard.rs"), "{e}");
        assert_eq!(catch(|| 5).unwrap(), 5);
    }
}
