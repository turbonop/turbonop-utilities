//! Writing the library file: straight from the members' bytes (mapped
//! inputs, or the members folding made, whose larger section data is still
//! the inputs') into the file, in order, with no copy of the whole library
//! in memory.
//!
//! One thread writes, from the start to the end. A file whose length is set
//! first, or that is written out of order, is zero-filled by NTFS up to each
//! write (its valid data length), which made writing 1 GiB take 1.2 to 1.4 s
//! against 0.3 s in order (measured on this PC's NVMe drive): in order, the
//! write is the kernel's copy into the file cache, at memory speed.
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
/// one thread then writes in order.
const CHUNK: usize = 4 << 20;

/// `pieces`, in order, cut into chunks of `CHUNK` bytes (the last may be
/// shorter): each chunk the pieces' parts it holds.
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
/// bytes from `members` (empty for a thin archive's), to a new file beside
/// `path`, and returns that file's path.
pub fn write_unix_beside(
    path: &Path,
    layout: &turbonop_archive::unix::Layout,
    members: &[Bytes<'_>],
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
            if data != m.data.len() as u64 {
                return Err(io::Error::other("a member isn't the size laid out"));
            }
            pieces.extend(segments(*m));
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
    let result = {
        let chunks = chunks(pieces);
        // A batch of chunks at a time is copied on every thread while one
        // thread writes the batch before it.
        let batch = turbonop_par::threads().max(1) * 2;
        std::thread::scope(|scope| {
            let (send, receive) = std::sync::mpsc::sync_channel::<Vec<Vec<u8>>>(1);
            let writer = scope.spawn(|| -> io::Result<()> {
                for bufs in receive {
                    for b in bufs {
                        io::Write::write_all(&mut &file, &b)?;
                    }
                }
                Ok(())
            });
            for group in chunks.chunks(batch) {
                let bufs = turbonop_par::map_balanced(group, 1, |parts| {
                    let mut buf = Vec::with_capacity(CHUNK);
                    for p in parts {
                        buf.extend_from_slice(p);
                    }
                    buf
                });
                if send.send(bufs).is_err() {
                    break;
                }
            }
            drop(send);
            writer.join().expect("the writer doesn't panic")
        })
    };
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
