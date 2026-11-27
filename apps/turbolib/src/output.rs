//! Writing the library file: straight from the members' bytes (mapped
//! inputs, or the members folding made, whose larger section data is still
//! the inputs') into the file, in order, with no copy of the whole library
//! in memory.
//!
//! One thread writes, from the start to the end. A file whose length is set
//! first, or that is written out of order, is zero-filled by NTFS up to each
//! write (its valid data length), which made writing 1 GiB take 1.2 to 1.4 s
//! against 0.3 s in order (measured on this PC's NVMe drive): in order, the
//! write is the kernel's copy into the file cache, at memory speed. On
//! Windows, the bytes are first copied into chunks on every thread (which
//! faults the inputs' mappings in at once); elsewhere each large piece is
//! written straight from its bytes.
//!
//! The library is written to a file beside the output and renamed over it
//! once it is complete, so the output is never half-written, and an input
//! that is also the output (`lib x.lib a.obj` updates `x.lib`) can stay
//! mapped while the new one is written.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use turbonop_archive::write::Layout;

/// A member to write: its bytes, and the pieces those leave out (each one's
/// offset in the member, and its bytes; see `turbonop_coff::Skeleton`).
#[derive(Clone, Copy, Debug)]
pub struct Bytes<'a> {
    pub data: &'a [u8],
    pub holes: &'a [(usize, &'a [u8])],
}

/// The member's bytes as pieces in order: the stretches of `data` between
/// holes, and the holes' bytes.
fn segments<'a>(b: Bytes<'a>) -> Vec<&'a [u8]> {
    let mut holes: Vec<(usize, &[u8])> = b.holes.to_vec();
    holes.sort_unstable_by_key(|h| h.0);
    let mut out = Vec::with_capacity(holes.len() * 2 + 1);
    let mut at = 0;
    for (start, piece) in holes {
        if start > at {
            out.push(&b.data[at..start]);
        }
        out.push(piece);
        at = start + piece.len();
    }
    if at < b.data.len() {
        out.push(&b.data[at..]);
    }
    out
}

/// The output is copied, on every thread, into chunks of this size, which
/// one thread then writes in order (Windows).
#[cfg_attr(unix, allow(dead_code))]
const CHUNK: usize = 4 << 20;

/// Writes `pieces` in order, each large one straight from its bytes (the
/// kernel's copy is the only one) and runs of small ones (headers,
/// padding) gathered first. On Linux this is faster than copying chunks
/// on every thread and writing those: writing `libclangSema.a` folded
/// (1.45 GB, its inputs in the page cache) took 0.32 to 0.35 s this way
/// against 0.38 to 0.63 s.
#[cfg(unix)]
fn write_direct(file: &File, pieces: &[&[u8]]) -> io::Result<()> {
    const SMALL: usize = 64 << 10;
    let mut buf: Vec<u8> = Vec::with_capacity(SMALL);
    for &p in pieces {
        if p.len() >= SMALL {
            if !buf.is_empty() {
                io::Write::write_all(&mut &*file, &buf)?;
                buf.clear();
            }
            io::Write::write_all(&mut &*file, p)?;
        } else {
            if buf.len() + p.len() > SMALL {
                io::Write::write_all(&mut &*file, &buf)?;
                buf.clear();
            }
            buf.extend_from_slice(p);
        }
    }
    io::Write::write_all(&mut &*file, &buf)
}

/// `pieces`, in order, cut into chunks of `CHUNK` bytes (the last may be
/// shorter): each chunk the pieces' parts it holds.
#[cfg(not(unix))]
fn chunks<'a>(pieces: &[&'a [u8]]) -> Vec<Vec<&'a [u8]>> {
    let mut out: Vec<Vec<&[u8]>> = vec![Vec::new()];
    let mut room = CHUNK;
    for &p in pieces {
        let mut p = p;
        while !p.is_empty() {
            if room == 0 {
                out.push(Vec::new());
                room = CHUNK;
            }
            let n = room.min(p.len());
            out.last_mut().expect("a chunk").push(&p[..n]);
            p = &p[n..];
            room -= n;
        }
    }
    out
}

/// How many chunks are in memory at once, at most (copied or being
/// copied, and waiting to be written): bounded whatever the number of
/// threads, so that many turbolib processes on a machine of many cores
/// don't each hold a buffer per thread.
#[cfg_attr(unix, allow(dead_code))]
const IN_FLIGHT: usize = 12;

/// Writes `chunks` in order on this thread, while up to `IN_FLIGHT - 1`
/// other threads copy the chunks ahead into buffers (reused: at most
/// `IN_FLIGHT` of them are ever made).
#[cfg(not(unix))]
fn write_chunked(file: &File, chunks: &[Vec<&[u8]>]) -> io::Result<()> {
    use std::sync::{Condvar, Mutex};
    struct State {
        /// The chunks copied and not yet written, by number.
        ready: std::collections::BTreeMap<usize, Vec<u8>>,
        /// Buffers written, to reuse.
        free: Vec<Vec<u8>>,
        /// The next chunk to give a copier, and the next to write.
        next: usize,
        written: usize,
        failed: bool,
    }
    let copiers = turbonop_par::threads()
        .min(IN_FLIGHT - 1)
        .min(chunks.len().saturating_sub(1));
    let fill = |buf: &mut Vec<u8>, parts: &[&[u8]]| {
        buf.clear();
        for p in parts {
            buf.extend_from_slice(p);
        }
    };
    if copiers < 2 {
        let mut buf = Vec::with_capacity(CHUNK);
        for parts in chunks {
            fill(&mut buf, parts);
            io::Write::write_all(&mut &*file, &buf)?;
        }
        return Ok(());
    }
    let state = Mutex::new(State {
        ready: std::collections::BTreeMap::new(),
        free: Vec::new(),
        next: 0,
        written: 0,
        failed: false,
    });
    let changed = Condvar::new();
    let lock = || state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::thread::scope(|scope| {
        for _ in 0..copiers {
            scope.spawn(|| {
                loop {
                    // A chunk no further ahead of the writer than the
                    // buffers allow.
                    let (i, mut buf) = {
                        let mut s = lock();
                        while !s.failed && s.next < chunks.len() && s.next - s.written >= IN_FLIGHT
                        {
                            s = changed.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        if s.failed || s.next >= chunks.len() {
                            return;
                        }
                        let i = s.next;
                        s.next += 1;
                        (i, s.free.pop().unwrap_or_else(|| Vec::with_capacity(CHUNK)))
                    };
                    fill(&mut buf, &chunks[i]);
                    lock().ready.insert(i, buf);
                    changed.notify_all();
                }
            });
        }
        let mut result = Ok(());
        for i in 0..chunks.len() {
            let buf = {
                let mut s = lock();
                loop {
                    if let Some(b) = s.ready.remove(&i) {
                        break b;
                    }
                    s = changed.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            result = io::Write::write_all(&mut &*file, &buf);
            let mut s = lock();
            s.written = i + 1;
            s.free.push(buf);
            s.failed |= result.is_err();
            drop(s);
            changed.notify_all();
            if result.is_err() {
                break;
            }
        }
        result
    })
}

/// Writes the library `layout` describes, with each member's bytes from
/// `members`, to a new file beside `path`, and returns that file's path.
/// `padding` is the byte after an odd-sized member.
pub fn write_beside(
    path: &Path,
    layout: &Layout<'_>,
    members: &[Bytes<'_>],
    padding: u8,
) -> io::Result<PathBuf> {
    let headers: Vec<[u8; 60]> = (0..members.len()).map(|i| layout.header(i)).collect();
    let pad = [padding];
    let mut pieces: Vec<&[u8]> = vec![layout.head()];
    let mut at = layout.head().len() as u64;
    let mut misplaced = false;
    for (i, m) in members.iter().enumerate() {
        misplaced |= at != layout.offset(i);
        pieces.push(&headers[i]);
        for piece in segments(*m) {
            pieces.push(piece);
        }
        if m.data.len() & 1 == 1 {
            pieces.push(&pad);
        }
        at += layout.span(i);
    }
    if misplaced || at != layout.size() {
        return Err(io::Error::other("members not where the layout puts them"));
    }
    write_pieces_beside(path, &pieces)
}

/// Writes the GNU or BSD archive `layout` describes, with each member's
/// bytes from `members` as parts in order (none for a thin archive's: a
/// member folding rewrote is mostly its input's bytes), to a new file
/// beside `path`, and returns that file's path.
pub fn write_unix_parts_beside(
    path: &Path,
    layout: &turbonop_archive::unix::Layout,
    members: &[Vec<&[u8]>],
) -> io::Result<PathBuf> {
    const NEWLINES: [u8; 8] = [b'\n'; 8];
    let mut pieces: Vec<&[u8]> = vec![layout.head()];
    let mut at = layout.head().len() as u64;
    for (i, m) in members.iter().enumerate() {
        if at != layout.offset(i) {
            return Err(io::Error::other("members not where the layout puts them"));
        }
        let header = layout.header(i);
        pieces.push(header);
        let data = layout.data_size(i);
        if data > 0 {
            if data != m.iter().map(|p| p.len() as u64).sum::<u64>() {
                return Err(io::Error::other("a member isn't the size laid out"));
            }
            pieces.extend(m.iter().copied());
        }
        // (The padding is at most 7 bytes.)
        let pad = usize::try_from(layout.span(i) - header.len() as u64 - data).unwrap_or(8);
        pieces.push(&NEWLINES[..pad.min(8)]);
        at += layout.span(i);
    }
    if at != layout.size() {
        return Err(io::Error::other("members not where the layout puts them"));
    }
    write_pieces_beside(path, &pieces)
}

/// Writes `pieces`, in order, to a new file beside `path`, and returns
/// that file's path.
pub fn write_pieces_beside(path: &Path, pieces: &[&[u8]]) -> io::Result<PathBuf> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".turbolib-{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);
    let file = File::create(&temp)?;
    #[cfg(unix)]
    let result = write_direct(&file, pieces);
    #[cfg(not(unix))]
    let result = write_chunked(&file, &chunks(pieces));
    drop(file);
    match result {
        Ok(()) => Ok(temp),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Puts the written library in its place.
pub fn replace(temp: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(temp);
    })
}

#[cfg(test)]
mod tests;
