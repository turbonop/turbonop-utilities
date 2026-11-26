//! Writing the library file: straight from the members' bytes (mapped
//! inputs, or the members folding made, whose larger section data is still
//! the inputs') into the file, at the offsets the layout gives, on every
//! thread at once, with no copy of the whole library in memory.
//!
//! The library is written to a file beside the output and renamed over it
//! once it is complete, so the output is never half-written, and an input
//! that is also the output (`lib x.lib a.obj` updates `x.lib`) can stay
//! mapped while the new one is written.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use turbonop_archive::write::Layout;

/// Writes `data` at `offset` in `file`, all of it.
fn write_at(file: &File, mut offset: u64, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(file, data, offset)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(file, data, offset)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        offset += n as u64;
        data = &data[n..];
    }
    Ok(())
}

/// Members smaller than this are written with their header in one write.
const SMALL: usize = 1 << 16;
/// Larger pieces are written in pieces of this, so that one large member's
/// copy is shared out among the threads.
const PIECE: usize = 1 << 24;

/// A member to write: its bytes, and the pieces those leave out (each one's
/// offset in the member, and its bytes; see `turbonop_coff::Skeleton`).
#[derive(Clone, Copy, Debug)]
pub struct Bytes<'a> {
    pub data: &'a [u8],
    pub holes: &'a [(usize, &'a [u8])],
}

/// One write.
enum Write<'a> {
    /// A small member whole: its header, its bytes with the holes filled,
    /// and the padding byte if any.
    Whole(usize),
    /// A member's header (and its padding byte, if any).
    Header(usize),
    /// Bytes at an offset in the file.
    At(u64, &'a [u8]),
}

/// The member's bytes as pieces in order: the stretches of `data` between
/// holes, and the holes' bytes.
fn segments<'a>(b: Bytes<'a>) -> Vec<(usize, &'a [u8])> {
    let mut holes: Vec<(usize, &[u8])> = b.holes.to_vec();
    holes.sort_unstable_by_key(|h| h.0);
    let mut out = Vec::with_capacity(holes.len() * 2 + 1);
    let mut at = 0;
    for (start, piece) in holes {
        if start > at {
            out.push((at, &b.data[at..start]));
        }
        out.push((start, piece));
        at = start + piece.len();
    }
    if at < b.data.len() {
        out.push((at, &b.data[at..]));
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
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".turbolib-{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);
    let file = File::create(&temp)?;
    let result = (|| {
        file.set_len(layout.size())?;
        write_at(&file, 0, layout.head())?;
        let mut writes: Vec<Write<'_>> = Vec::with_capacity(members.len());
        for (i, m) in members.iter().enumerate() {
            if m.data.len() < SMALL {
                writes.push(Write::Whole(i));
                continue;
            }
            writes.push(Write::Header(i));
            let base = layout.offset(i) + 60;
            for (at, piece) in segments(*m) {
                let mut from = 0;
                while from < piece.len() {
                    let to = (from + PIECE).min(piece.len());
                    writes.push(Write::At(base + (at + from) as u64, &piece[from..to]));
                    from = to;
                }
            }
        }
        let results = turbonop_par::map_balanced(&writes, 8, |w| match *w {
            Write::Whole(i) => {
                let m = members[i];
                let mut buf = Vec::with_capacity(60 + m.data.len() + 1);
                buf.extend_from_slice(&layout.header(i));
                buf.extend_from_slice(m.data);
                for (at, piece) in m.holes {
                    buf[60 + at..60 + at + piece.len()].copy_from_slice(piece);
                }
                if m.data.len() & 1 == 1 {
                    buf.push(padding);
                }
                write_at(&file, layout.offset(i), &buf)
            }
            Write::Header(i) => {
                write_at(&file, layout.offset(i), &layout.header(i))?;
                if members[i].data.len() & 1 == 1 {
                    let end = layout.offset(i) + layout.span(i) - 1;
                    write_at(&file, end, &[padding])?;
                }
                Ok(())
            }
            Write::At(at, bytes) => write_at(&file, at, bytes),
        });
        results.into_iter().collect::<io::Result<()>>()
    })();
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
