//! A target's address space: its memory, with its modules' image files
//! standing in for what it didn't capture.
//!
//! The target's own bytes always win. Where it has none, a module's image
//! file gives the bytes as they were when it loaded (see
//! [`crate::LoadedImage`]): [`Provenance::Image`], since they may differ
//! from what was there when the target stopped.

use alloc::vec::Vec;

use crate::{LoadedImage, Memory, Provenance};

/// A run of available bytes: where, how many, and from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u64,
    pub len: u64,
    pub provenance: Provenance,
}

/// A target's memory, and its modules' images.
pub struct AddressSpace<'a> {
    memory: &'a dyn Memory,
    images: Vec<LoadedImage<'a>>,
}

impl<'a> AddressSpace<'a> {
    pub fn new(memory: &'a dyn Memory, mut images: Vec<LoadedImage<'a>>) -> AddressSpace<'a> {
        images.sort_unstable_by_key(LoadedImage::base);
        AddressSpace { memory, images }
    }

    /// The images, by load address.
    pub fn images(&self) -> &[LoadedImage<'a>] {
        &self.images
    }

    /// The image whose extent `address` is in.
    pub fn image_at(&self, address: u64) -> Option<&LoadedImage<'a>> {
        let i = self.images.partition_point(|m| m.base() <= address);
        let m = self.images.get(i.checked_sub(1)?)?;
        m.contains(address).then_some(m)
    }

    /// Fills `buf` from `address`, if all of it is available: where its
    /// bytes came from (the least certain, if from more than one place).
    pub fn read(&self, address: u64, buf: &mut [u8]) -> Option<Provenance> {
        let mut provenance: Option<Provenance> = None;
        let mut done = 0;
        while done < buf.len() {
            let a = address.checked_add(done as u64)?;
            let (n, p) = self.read_some(a, &mut buf[done..]);
            if n == 0 {
                return None;
            }
            provenance = Some(provenance.map_or(p, |q| q.worst(p)));
            done += n;
        }
        provenance.or(Some(self.memory.provenance()))
    }

    /// What's available of the `len` bytes from `address`, as runs by
    /// address (a run from each source, each contiguous).
    pub fn runs(&self, address: u64, len: u64) -> Vec<Run> {
        let mut out: Vec<Run> = Vec::new();
        let end = address.saturating_add(len);
        let mut at = address;
        let mut scratch = [0u8; 4096];
        while at < end {
            let want = usize::try_from((end - at).min(scratch.len() as u64)).unwrap_or(0);
            let (n, p) = self.read_some(at, &mut scratch[..want]);
            if n == 0 {
                // To the next byte anything has.
                let next = [
                    self.memory.next_available(at),
                    self.image_at(at).and_then(|m| m.next_available(at)),
                    self.images.iter().map(LoadedImage::base).find(|&b| b > at),
                ]
                .into_iter()
                .flatten()
                .min();
                match next {
                    Some(n) if n > at => at = n,
                    _ => break,
                }
                continue;
            }
            match out.last_mut() {
                Some(r) if r.start + r.len == at && r.provenance == p => r.len += n as u64,
                _ => out.push(Run {
                    start: at,
                    len: n as u64,
                    provenance: p,
                }),
            }
            at += n as u64;
        }
        out
    }

    /// The bytes at `address` and after, as many as are contiguous from
    /// one source: how many, and from where.
    fn read_some(&self, address: u64, buf: &mut [u8]) -> (usize, Provenance) {
        let n = self.memory.read_at(address, buf);
        if n > 0 {
            return (n, self.memory.provenance());
        }
        let Some(image) = self.image_at(address) else {
            return (0, Provenance::Image);
        };
        // Up to where the target's own bytes resume.
        let limit = self
            .memory
            .next_available(address)
            .map_or(buf.len() as u64, |n| (n - address).min(buf.len() as u64));
        let limit = usize::try_from(limit).unwrap_or(buf.len());
        (image.read_at(address, &mut buf[..limit]), Provenance::Image)
    }
}

impl turbonop_unwind::Memory for AddressSpace<'_> {
    fn read(&self, address: u64, buf: &mut [u8]) -> bool {
        AddressSpace::read(self, address, buf).is_some()
    }
}
