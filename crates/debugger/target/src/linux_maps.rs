//! Linux's `/proc/<pid>/maps` (Breakpad's `MD_LINUX_MAPS` stream): the
//! files mapped into a process, which find modules a module list missed.
//!
//! Each line is `start-end perms offset dev inode path`, addresses and
//! the offset in hex. A module is a file mapped from offset 0 (its
//! headers) with at least one executable mapping; it spans from there to
//! the end of the last mapping of the same file that follows.

use alloc::string::String;
use alloc::vec::Vec;

/// A file a process mapped as a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedModule {
    pub path: String,
    pub base: u64,
    pub size: u64,
}

/// A line of the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping<'a> {
    pub start: u64,
    pub end: u64,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub offset: u64,
    /// The file (or `[stack]`, `[heap]`, `[vdso]`, ...; empty for an
    /// anonymous mapping).
    pub path: &'a str,
}

/// The lines of a map that parse (a line whose path isn't UTF-8 doesn't).
pub fn mappings(text: &[u8]) -> Vec<Mapping<'_>> {
    text.split(|&b| b == b'\n')
        .filter_map(|line| core::str::from_utf8(line).ok())
        .filter_map(|line| parse_line(line.trim_end_matches('\r')))
        .collect()
}

fn parse_line(line: &str) -> Option<Mapping<'_>> {
    let mut fields = line.splitn(6, ' ');
    let range = fields.next()?;
    let perms = fields.next()?.as_bytes();
    let offset = u64::from_str_radix(fields.next()?, 16).ok()?;
    let _device = fields.next()?;
    let _inode = fields.next()?;
    let path = fields.next().unwrap_or("").trim_start();
    let (start, end) = range.split_once('-')?;
    let (start, end) = (
        u64::from_str_radix(start, 16).ok()?,
        u64::from_str_radix(end, 16).ok()?,
    );
    if perms.len() < 4 || end < start {
        return None;
    }
    Some(Mapping {
        start,
        end,
        readable: perms[0] == b'r',
        writable: perms[1] == b'w',
        executable: perms[2] == b'x',
        offset,
        path,
    })
}

/// The modules a map shows, by address.
pub fn modules(text: &[u8]) -> Vec<MappedModule> {
    let maps = mappings(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < maps.len() {
        let m = &maps[i];
        let file = !m.path.is_empty() && !m.path.starts_with('[');
        if !(file && m.offset == 0) {
            i += 1;
            continue;
        }
        // This file's mappings that follow (with anonymous ones between,
        // such as its .bss's).
        let mut end = m.end;
        let mut executable = m.executable;
        let mut j = i + 1;
        while j < maps.len() {
            let n = &maps[j];
            if n.path == m.path && n.offset != 0 {
                end = n.end;
                executable |= n.executable;
            } else if !n.path.is_empty() {
                break;
            }
            j += 1;
        }
        if executable {
            out.push(MappedModule {
                path: String::from(m.path),
                base: m.start,
                size: end - m.start,
            });
        }
        i = j;
    }
    out
}
