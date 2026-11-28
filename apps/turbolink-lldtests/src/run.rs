//! Running a test's RUN lines: each in bash (`set -o pipefail`, as lit's
//! shell fails a pipeline on any failing command), with lit's
//! substitutions, in a fresh directory of its own, `lld-link` on PATH being
//! turbolink.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::lit::Test;

/// Where things are.
pub struct Setup {
    /// The directory of the tests (`lld/test/COFF`).
    pub tests: PathBuf,
    /// The LLVM tools (`FileCheck`, `llvm-readobj`, `yaml2obj`, ...).
    pub tools: PathBuf,
    /// A directory holding `lld-link.exe`: turbolink, copied.
    pub shim: PathBuf,
    /// The scratch directory (a directory per test in it).
    pub work: PathBuf,
    /// bash.
    pub bash: PathBuf,
    /// How long one RUN line may take.
    pub timeout: Duration,
}

/// How a test ran.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Every RUN line succeeded.
    Passed,
    /// RUN line `index` (0-based) failed: as substituted, its exit code,
    /// what it printed to stdout and stderr.
    Failed {
        /// The line's index.
        index: usize,
        /// The line, substituted.
        line: String,
        /// Its exit code (`None`: killed, after the timeout).
        code: Option<i32>,
        /// Its standard output (the first 16 KiB).
        stdout: String,
        /// Its standard error (the first 16 KiB).
        stderr: String,
    },
}

/// A path as bash and the tools take it: forward slashes.
fn slashes(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// `line` with lit's substitutions for test `t` (its own directory `dir`).
pub fn substitute(line: &str, t: &Test, setup: &Setup, dir: &Path) -> String {
    let source = slashes(&setup.tests.join(&t.name));
    let source_dir = slashes(&setup.tests);
    let tmp = format!("{}/{}.tmp", slashes(dir), t.name);
    let basename = format!("{}.tmp", t.name);
    // (`%%` is a literal `%`; protected while the rest is replaced.)
    let mut s = line.replace("%%", "\u{1}");
    for (from, to) in [
        ("%basename_t", basename.as_str()),
        ("%errc_ENOENT", "'no such file or directory'"),
        ("%errc_EACCES", "'permission denied'"),
        ("%python", "python"),
        ("%s", source.as_str()),
        ("%S", source_dir.as_str()),
        ("%p", source_dir.as_str()),
        ("%t", tmp.as_str()),
        ("%T", &slashes(dir)),
    ] {
        s = s.replace(from, to);
    }
    s.replace('\u{1}', "%")
}

/// Reads at most 4 MiB of `r`, the rest discarded.
fn drain(mut r: impl Read) -> String {
    let mut all = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        if all.len() < 4 << 20 {
            all.extend_from_slice(&buf[..n]);
        }
    }
    String::from_utf8_lossy(&all).into_owned()
}

/// Runs test `t`.
pub fn run(t: &Test, setup: &Setup) -> Outcome {
    let dir = setup.work.join(&t.name);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Outcome::Failed {
            index: 0,
            line: String::new(),
            code: None,
            stdout: String::new(),
            stderr: format!("runner: can't make {}: {e}", dir.display()),
        };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![setup.shim.clone(), setup.tools.clone()];
    paths.extend(std::env::split_paths(&path));
    let path = std::env::join_paths(paths).unwrap_or(path);
    // The RUN lines, in one bash (lit keeps a `cd` from one line to the
    // next): a marker before each on both outputs, and the first failing
    // one's index and status on stderr.
    let lines: Vec<String> = t
        .runs
        .iter()
        .map(|r| substitute(r, t, setup, &dir))
        .collect();
    let mut script = String::from(
        "set -o pipefail
",
    );
    for (i, line) in lines.iter().enumerate() {
        script.push_str(&format!(
            "echo '{MARK}{i}'; echo '{MARK}{i}' >&2
{line}
__s=$?; if [ $__s -ne 0 ]; then echo \"{FAIL}{i} $__s\" >&2; exit $__s; fi
"
        ));
    }
    let failed =
        |index: usize, code: Option<i32>, stdout: String, stderr: String| Outcome::Failed {
            index,
            line: lines.get(index).cloned().unwrap_or_default(),
            code,
            stdout,
            stderr,
        };
    let child = Command::new(&setup.bash)
        .arg("-c")
        .arg(&script)
        .current_dir(&dir)
        .env("PATH", &path)
        .env("LLD_IN_TEST", "2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return failed(
                0,
                None,
                String::new(),
                format!("runner: can't run bash: {e}"),
            );
        }
    };
    // The pipes are read on threads of their own (a full pipe would stop
    // the child), while this one waits, to the timeout.
    let out = child
        .stdout
        .take()
        .map(|o| std::thread::spawn(move || drain(o)));
    let err = child
        .stderr
        .take()
        .map(|e| std::thread::spawn(move || drain(e)));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() > setup.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.is_some_and(|s| s.success()) {
        let _ = std::fs::remove_dir_all(&dir);
        return Outcome::Passed;
    }
    // The failing line: the FAIL marker's, else (killed) the last begun.
    let index = stderr
        .rfind(FAIL)
        .and_then(|at| {
            stderr[at + FAIL.len()..]
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .or_else(|| last_mark(&stderr).map(|(i, _)| i))
        .unwrap_or(0);
    let code = status.and_then(|s| s.code());
    failed(
        index,
        code,
        after_mark(&stdout, index),
        after_mark(&stderr, index),
    )
}

/// The marker before each RUN line's output, then its index.
const MARK: &str = "@@@turbolink-lldtests RUN ";
/// The marker of the failing line: its index and status.
const FAIL: &str = "@@@turbolink-lldtests FAILED ";

/// The last RUN marker in `s`: its index, and where its line's output
/// starts.
fn last_mark(s: &str) -> Option<(usize, usize)> {
    let at = s.rfind(MARK)?;
    let rest = &s[at + MARK.len()..];
    let end = rest.find('\n').unwrap_or(rest.len());
    Some((rest[..end].trim().parse().ok()?, at + MARK.len() + end + 1))
}

/// What RUN line `index` printed to `s` (from its marker to the next), the
/// first 16 KiB.
fn after_mark(s: &str, index: usize) -> String {
    let mark = format!(
        "{MARK}{index}
"
    );
    let start = s.find(&mark).map_or(0, |at| at + mark.len());
    let rest = &s[start.min(s.len())..];
    let end = rest
        .find(MARK)
        .or_else(|| rest.find(FAIL))
        .unwrap_or(rest.len());
    let mut out = rest[..end].to_owned();
    if out.len() > 16 * 1024 {
        let mut cut = 16 * 1024;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lits_substitutions() {
        let setup = Setup {
            tests: PathBuf::from("T"),
            tools: PathBuf::new(),
            shim: PathBuf::new(),
            work: PathBuf::new(),
            bash: PathBuf::new(),
            timeout: Duration::from_secs(1),
        };
        let t = Test {
            name: "a.s".into(),
            ..Test::default()
        };
        let got = substitute(
            "llvm-mc %s -o %t.obj && ls %S/Inputs %p && echo 100%% %basename_t",
            &t,
            &setup,
            Path::new("W"),
        );
        assert_eq!(
            got,
            "llvm-mc T/a.s -o W/a.s.tmp.obj && ls T/Inputs T && echo 100% a.s.tmp"
        );
    }
}
