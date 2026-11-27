//! The MSF 7.00 container: streams laid out in blocks, the free block
//! map, the stream directory and its block map.
//!
//! Blocks are allocated in order (skipping the free block maps' blocks,
//! which repeat every block-size blocks), so each stream's blocks are
//! mostly contiguous; the file is then filled block by block on every
//! core.

/// The superblock's magic.
pub const MAGIC: &[u8; 32] = b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0";

/// Why the streams don't fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooLarge(pub String);

/// Where each stream went, for patching after the file is built.
#[derive(Debug, Clone)]
pub struct Layout {
    /// The block size.
    pub block_size: u32,
    /// Per stream, its blocks.
    pub blocks: Vec<Vec<u32>>,
}

impl Layout {
    /// The file offset of byte `offset` of stream `stream`.
    pub fn file_offset(&self, stream: usize, offset: usize) -> usize {
        let bs = self.block_size as usize;
        self.blocks[stream][offset / bs] as usize * bs + offset % bs
    }
}

fn is_fpm(block: u32, bs: u32) -> bool {
    let m = block % bs;
    m == 1 || m == 2
}

/// The MSF file holding `streams` (stream 0 is the old directory, usually
/// empty), with blocks of `block_size` bytes (4096; larger for files over
/// 4 GB of blocks... the directory's block map must fit one block).
pub fn write(streams: &[&[u8]], block_size: u32) -> Result<(Vec<u8>, Layout), TooLarge> {
    let bs = block_size;
    let mut next = 3u32;
    let mut alloc = |n: usize| -> Vec<u32> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            if !is_fpm(next, bs) {
                v.push(next);
            }
            next += 1;
        }
        v
    };
    let per = |len: usize| len.div_ceil(bs as usize);
    let blocks: Vec<Vec<u32>> = streams.iter().map(|s| alloc(per(s.len()))).collect();
    let total_blocks: usize = blocks.iter().map(Vec::len).sum();
    let mut directory = Vec::with_capacity(4 + 4 * streams.len() + 4 * total_blocks);
    let n = u32::try_from(streams.len()).map_err(|_| TooLarge("too many streams".into()))?;
    directory.extend_from_slice(&n.to_le_bytes());
    for s in streams {
        let len = u32::try_from(s.len()).map_err(|_| TooLarge("a stream over 4 GB".into()))?;
        directory.extend_from_slice(&len.to_le_bytes());
    }
    for b in blocks.iter().flatten() {
        directory.extend_from_slice(&b.to_le_bytes());
    }
    let dir_blocks = alloc(per(directory.len()));
    if dir_blocks.len() * 4 > bs as usize {
        return Err(TooLarge(format!(
            "the stream directory needs {} blocks, more than a block of {bs} bytes can list (a larger /PDBPAGESIZE)",
            dir_blocks.len()
        )));
    }
    let map_block = alloc(1)[0];
    let num_blocks = next;
    let size = num_blocks as usize * bs as usize;
    let mut out = vec![0u8; size];

    // The superblock.
    out[..32].copy_from_slice(MAGIC);
    let put = |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut out, 32, bs);
    put(&mut out, 36, 1);
    put(&mut out, 40, num_blocks);
    put(
        &mut out,
        44,
        u32::try_from(directory.len()).map_err(|_| TooLarge("directory".into()))?,
    );
    put(&mut out, 48, 0);
    put(&mut out, 52, map_block);

    // The free block maps: every block in the file is used. The map's
    // bytes go in blocks 1, 1 + bs, 1 + 2 bs, ... (and the second map's
    // in 2, 2 + bs, ...: all free, as nothing is pending).
    let intervals = num_blocks.div_ceil(bs);
    for k in 0..intervals {
        let at1 = (k * bs + 1) as usize * bs as usize;
        let at2 = (k * bs + 2) as usize * bs as usize;
        if at1 < size {
            for (j, byte) in out[at1..at1 + bs as usize].iter_mut().enumerate() {
                let first = (k as usize * bs as usize + j) * 8;
                let mut v = 0u8;
                for bit in 0..8 {
                    if first + bit >= num_blocks as usize {
                        v |= 1 << bit;
                    }
                }
                *byte = v;
            }
        }
        if at2 < size {
            out[at2..at2 + bs as usize].fill(0xFF);
        }
    }

    // The stream data, the directory, and its block map: each block of
    // the file from its source, in parallel.
    let mut sources: Vec<Option<&[u8]>> = vec![None; num_blocks as usize];
    fn chunk(data: &[u8], i: usize, bs: u32) -> &[u8] {
        let s = i * bs as usize;
        &data[s..data.len().min(s + bs as usize)]
    }
    for (s, bl) in streams.iter().zip(&blocks) {
        for (i, &b) in bl.iter().enumerate() {
            sources[b as usize] = Some(chunk(s, i, bs));
        }
    }
    for (i, &b) in dir_blocks.iter().enumerate() {
        sources[b as usize] = Some(chunk(&directory, i, bs));
    }
    let map: Vec<u8> = dir_blocks.iter().flat_map(|b| b.to_le_bytes()).collect();
    sources[map_block as usize] = Some(&map);
    let mut parts: Vec<(&mut [u8], Option<&[u8]>)> = out
        .chunks_mut(bs as usize)
        .zip(sources)
        .filter(|(_, s)| s.is_some())
        .collect();
    turbonop_par::for_each_chunk_mut(&mut parts, 256, |part| {
        for (dst, src) in part.iter_mut() {
            if let Some(src) = src {
                dst[..src.len()].copy_from_slice(src);
            }
        }
    });
    drop(parts);
    let mut all = blocks;
    all.push(dir_blocks);
    Ok((
        out,
        Layout {
            block_size: bs,
            blocks: all,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_back() {
        let big: Vec<u8> = (0..40_000u32).flat_map(u32::to_le_bytes).collect();
        let streams: Vec<&[u8]> = vec![b"", b"one", &big, b"", b"tail"];
        let (file, layout) = write(&streams, 4096).unwrap();
        let msf = turbonop_pdb::msf::Msf::parse(&file).unwrap();
        assert_eq!(msf.stream_count(), 5);
        for (i, s) in streams.iter().enumerate() {
            let data = msf.stream_or_empty(u32::try_from(i).unwrap()).unwrap();
            assert_eq!(&data.to_vec()[..], *s, "stream {i}");
        }
        assert_eq!(
            &file[layout.file_offset(2, 4)..layout.file_offset(2, 4) + 4],
            &1u32.to_le_bytes()
        );
    }

    #[test]
    fn skips_the_free_block_maps() {
        // Over one interval of blocks: blocks 4097 and 4098 are maps.
        let big = vec![7u8; 4096 * 5000];
        let (file, layout) = write(&[b"", &big], 4096).unwrap();
        assert!(
            layout.blocks[1]
                .iter()
                .all(|&b| b % 4096 != 1 && b % 4096 != 2)
        );
        let msf = turbonop_pdb::msf::Msf::parse(&file).unwrap();
        assert!(msf.stream(1).unwrap().to_vec().iter().all(|&b| b == 7));
    }
}
