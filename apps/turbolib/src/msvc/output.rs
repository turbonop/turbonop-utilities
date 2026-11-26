//! Writing the library file: straight from the members' bytes (mapped
//! inputs, or the members folding made) into the file, at the offsets the
//! layout gives, on every thread at once, with no copy of the whole library
//! in memory.
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
/// Members larger than this are written in pieces of it, so that one large
/// member's copy is shared out among the threads.
const PIECE: usize = 1 << 24;

/// One write: where, and what (a member's header and the start of its data,
/// or a piece of its data).
enum Write<'a> {
    /// The header, the data, and the padding byte if any, together.
    Whole(usize),
    /// The header alone.
    Header(usize),
    /// `data[from..to]` of member `i`.
    Piece(usize, usize, usize, &'a [u8]),
}

/// Writes the library `layout` describes, with each member's bytes from
/// `data`, to a new file beside `path`, and returns that file's path.
/// `padding` is the byte after an odd-sized member.
pub fn write_beside(
    path: &Path,
    layout: &Layout<'_>,
    data: &[&[u8]],
    padding: u8,
) -> io::Result<PathBuf> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".turbolib-{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);
    let file = File::create(&temp)?;
    let result = (|| {
        file.set_len(layout.size())?;
        write_at(&file, 0, layout.head())?;
        let mut writes: Vec<Write<'_>> = Vec::with_capacity(data.len());
        for (i, d) in data.iter().enumerate() {
            if d.len() < SMALL {
                writes.push(Write::Whole(i));
            } else {
                writes.push(Write::Header(i));
                let mut at = 0;
                while at < d.len() {
                    let to = (at + PIECE).min(d.len());
                    writes.push(Write::Piece(i, at, to, d));
                    at = to;
                }
            }
        }
        let results = turbonop_par::map_balanced(&writes, 8, |w| match *w {
            Write::Whole(i) => {
                let mut buf = Vec::with_capacity(60 + data[i].len() + 1);
                buf.extend_from_slice(&layout.header(i));
                buf.extend_from_slice(data[i]);
                if data[i].len() & 1 == 1 {
                    buf.push(padding);
                }
                write_at(&file, layout.offset(i), &buf)
            }
            Write::Header(i) => {
                write_at(&file, layout.offset(i), &layout.header(i))?;
                if data[i].len() & 1 == 1 {
                    let end = layout.offset(i) + layout.span(i) - 1;
                    write_at(&file, end, &[padding])?;
                }
                Ok(())
            }
            Write::Piece(i, from, to, d) => {
                let at = layout.offset(i) + 60 + from as u64;
                write_at(&file, at, &d[from..to])
            }
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
