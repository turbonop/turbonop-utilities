//! A module's image file as it would be in memory where the module was
//! loaded: its file-backed bytes at their load addresses, with the words
//! the loader writes (base relocations, rebases, relative relocations)
//! written as the loader would have.
//!
//! A word the loader fills with another module's address (an import) is
//! unknown here: its file bytes aren't what was in memory. So is memory a
//! file doesn't back (zero-fill sections, `.bss`): the program may have
//! written it since.

use alloc::vec::Vec;

use turbonop_core::Endian;
use turbonop_image::{Image, PointerSlot};

/// A run of an image's file bytes: `len` bytes at image address `addr`
/// (see `turbonop_image`'s addresses: RVAs for PE), from file offset
/// `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Piece {
    addr: u64,
    len: u64,
    offset: u64,
}

/// An image, loaded at an address.
#[derive(Debug)]
pub struct LoadedImage<'a> {
    data: &'a [u8],
    endian: Endian,
    /// What a load address less this is the image address (a PE image's
    /// base; an ELF or Mach-O image's base less the address its first
    /// byte was linked at).
    bias: u64,
    base: u64,
    size: u64,
    pieces: Vec<Piece>,
    slots: Vec<PointerSlot>,
    /// What a slot's target (an image address) is less its value in
    /// memory.
    target_bias: u64,
}

impl<'a> LoadedImage<'a> {
    /// `image` as loaded at `base` (where its first byte, its headers, is
    /// in memory). None for an object, a WebAssembly module, or an image
    /// with nothing to load.
    pub fn new(image: &Image<'a>, base: u64) -> Option<LoadedImage<'a>> {
        let mut pieces = Vec::new();
        let (bias, size) = match image {
            Image::Pe(pe) => {
                let header = pe.optional_header()?;
                let headers = u64::from(header.size_of_headers());
                pieces.push(Piece {
                    addr: 0,
                    len: headers,
                    offset: 0,
                });
                for s in image.sections() {
                    // (A PE section's file data may be padded past its
                    // size in memory.)
                    let len = if s.vm_size == 0 {
                        s.file_size
                    } else {
                        s.file_size.min(s.vm_size)
                    };
                    if len > 0 {
                        pieces.push(Piece {
                            addr: s.addr,
                            len,
                            offset: s.file_offset,
                        });
                    }
                }
                (base, u64::from(header.size_of_image()))
            }
            Image::Elf(elf) => {
                use turbonop_elf::consts::PT_LOAD;
                let loads: Vec<_> = elf.segments().filter(|s| s.kind == PT_LOAD).collect();
                // The address the file's first byte is linked at: its
                // first loaded segment's, less that segment's offset.
                let first = loads.iter().min_by_key(|s| s.vaddr)?;
                let link = first.vaddr.wrapping_sub(first.offset);
                let mut end = link;
                for s in &loads {
                    if s.filesz > 0 {
                        pieces.push(Piece {
                            addr: s.vaddr,
                            len: s.filesz,
                            offset: s.offset,
                        });
                    }
                    end = end.max(s.vaddr.saturating_add(s.memsz));
                }
                (base.wrapping_sub(link), end - link)
            }
            Image::MachO(m) => {
                let segments = m.segments();
                let text = segments.iter().find(|s| s.fileoff == 0 && s.filesize > 0)?;
                let link = text.vmaddr;
                let mut end = link;
                for s in &segments {
                    if s.filesize > 0 {
                        pieces.push(Piece {
                            addr: s.vmaddr,
                            len: s.filesize.min(s.vmsize),
                            offset: s.fileoff,
                        });
                    }
                    // (Not __PAGEZERO, below the image.)
                    if s.vmaddr >= link {
                        end = end.max(s.vmaddr.saturating_add(s.vmsize));
                    }
                }
                (base.wrapping_sub(link), end - link)
            }
            _ => return None,
        };
        pieces.retain(|p| {
            p.offset
                .checked_add(p.len)
                .is_some_and(|e| e <= image.data().len() as u64)
        });
        pieces.sort_unstable_by_key(|p| p.addr);
        if pieces.is_empty() {
            return None;
        }
        let mut slots = image.pointer_slots();
        slots.sort_unstable_by_key(|s| s.addr);
        // A slot's target is an image address: in memory, it's the load
        // address (and a PE image's targets are RVAs, as its addresses
        // are).
        let target_bias = bias;
        Some(LoadedImage {
            data: image.data(),
            endian: image.endian(),
            bias,
            base,
            size,
            pieces,
            slots,
            target_bias,
        })
    }

    /// Where it's loaded.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// How many bytes it spans in memory.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// What a load address less this is the image's own address for it.
    pub fn bias(&self) -> u64 {
        self.bias
    }

    /// Whether `address` is in its extent.
    pub fn contains(&self, address: u64) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    /// The image's own address for load address `address`.
    pub fn image_address(&self, address: u64) -> u64 {
        address.wrapping_sub(self.bias)
    }

    /// Copies the bytes at load address `address` and after into `buf`,
    /// as they were in memory, as many as are contiguous and known; how
    /// many (0 if none at `address`).
    pub fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        if !self.contains(address) || buf.is_empty() {
            return 0;
        }
        let at = self.image_address(address);
        let i = self.pieces.partition_point(|p| p.addr <= at);
        let Some(p) = i.checked_sub(1).map(|i| self.pieces[i]) else {
            return 0;
        };
        let into = at - p.addr;
        if into >= p.len {
            return 0;
        }
        let mut n = usize::try_from((p.len - into).min(buf.len() as u64)).unwrap_or(0);
        let Ok(from) = usize::try_from(p.offset + into) else {
            return 0;
        };
        buf[..n].copy_from_slice(&self.data[from..from + n]);
        // The loader's words over the range: written, or (another
        // module's address) the end of what's known.
        let end = at + n as u64;
        let first = self
            .slots
            .partition_point(|s| s.addr.saturating_add(u64::from(s.width)) <= at);
        for s in &self.slots[first..] {
            if s.addr >= end {
                break;
            }
            let Some(target) = s.target else {
                n = usize::try_from(s.addr.saturating_sub(at)).unwrap_or(0);
                break;
            };
            let value = target.wrapping_add(self.target_bias);
            let width = usize::from(s.width);
            let bytes = match (self.endian, width) {
                (Endian::Little, 8) => value.to_le_bytes(),
                (Endian::Big, 8) => value.to_be_bytes(),
                #[allow(clippy::cast_possible_truncation)]
                (Endian::Little, 4) => {
                    let mut b = [0u8; 8];
                    b[..4].copy_from_slice(&(value as u32).to_le_bytes());
                    b
                }
                #[allow(clippy::cast_possible_truncation)]
                (Endian::Big, 4) => {
                    let mut b = [0u8; 8];
                    b[..4].copy_from_slice(&(value as u32).to_be_bytes());
                    b
                }
                _ => {
                    n = usize::try_from(s.addr.saturating_sub(at)).unwrap_or(0);
                    break;
                }
            };
            // The part of the slot in the range.
            for (a, &b) in (s.addr..).zip(&bytes[..width]) {
                if a >= at && a < end {
                    buf[usize::try_from(a - at).unwrap_or(0)] = b;
                }
            }
        }
        n
    }

    /// The next load address after `address` at which it has known bytes.
    pub fn next_available(&self, address: u64) -> Option<u64> {
        let at = self.image_address(address);
        // Past an unknown word in the piece `address` is in.
        let i = self.pieces.partition_point(|p| p.addr <= at);
        if let Some(p) = i.checked_sub(1).map(|i| self.pieces[i])
            && at - p.addr < p.len
        {
            let first = self
                .slots
                .partition_point(|s| s.addr.saturating_add(u64::from(s.width)) <= at);
            if let Some(s) = self.slots.get(first)
                && s.target.is_none()
                && s.addr <= at
            {
                let after = s.addr + u64::from(s.width);
                if after - p.addr < p.len {
                    return Some(after.wrapping_add(self.bias));
                }
            }
        }
        let p = self.pieces.iter().find(|p| p.addr > at)?;
        Some(p.addr.wrapping_add(self.bias))
    }
}
