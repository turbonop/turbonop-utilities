//! Compressed debug sections: ELF's `SHF_COMPRESSED` (zlib or zstd, after
//! an `Elf*_Chdr`), and the GNU `.zdebug_*` form (`ZLIB`, a big-endian
//! 64-bit size, then zlib) that ELF and Mach-O (`__zdebug_*`) use.
//!
//! Decompression stops at the size the header declares (at most
//! [`MAX_SIZE`]): a section that claims little and inflates to much is
//! rejected, not followed.
//!
//! Also xz ([`xz`]), for ELF's MiniDebugInfo (`.gnu_debugdata`): the
//! container is read here, its LZMA2 data by `lzma-rust2`.

use hexcavator_elf::consts::{ELFCOMPRESS_ZLIB, ELFCOMPRESS_ZSTD, SHF_COMPRESSED};
use hexcavator_elf::{ElfFile, Section};
use hexcavator_pe::PeFile;

/// A debug section: its name without its prefix (`info` for
/// `.debug_info`), its contents (decompressed if they were compressed),
/// and, if the contents are the file's own bytes, where they start in
/// the input.
pub type DebugSection<'d> = (&'d [u8], std::borrow::Cow<'d, [u8]>, Option<u64>);

/// The largest section decompressed.
pub const MAX_SIZE: u64 = 1 << 32;

/// zlib data to `size` bytes, exactly.
pub fn zlib(data: &[u8], size: u64) -> Option<Vec<u8>> {
    if size > MAX_SIZE {
        return None;
    }
    let limit = usize::try_from(size).ok()?;
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit).ok()?;
    (out.len() == limit).then_some(out)
}

/// zstd data to `size` bytes, exactly.
pub fn zstd(data: &[u8], size: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    if size > MAX_SIZE {
        return None;
    }
    let limit = usize::try_from(size).ok()?;
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(data).ok()?;
    let mut out = Vec::with_capacity(limit.min(1 << 24));
    // One byte past the limit tells a lie from the truth.
    (&mut decoder).take(size + 1).read_to_end(&mut out).ok()?;
    (out.len() == limit).then_some(out)
}

/// The largest LZMA2 dictionary [`xz`] allocates (xz's presets use at
/// most 64 MiB; a header can ask for 4 GiB).
pub const MAX_DICT: u32 = 64 << 20;

/// An xz stream's contents (all its blocks, and any streams after it),
/// at most `limit` bytes; `None` if it is damaged, uses a filter other
/// than LZMA2 alone, asks for a dictionary past [`MAX_DICT`], or holds
/// more than `limit` bytes. Headers, the index, and CRC-32 and CRC-64
/// checks are verified (SHA-256 checks are not).
pub fn xz(data: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = data;
    loop {
        rest = xz_stream(rest, limit, &mut out)?;
        // Stream padding (zero words), then perhaps another stream.
        let zeros = rest.iter().take_while(|&&b| b == 0).count();
        if zeros % 4 != 0 {
            return None;
        }
        rest = &rest[zeros..];
        if rest.is_empty() {
            return Some(out);
        }
    }
}

const XZ_MAGIC: &[u8; 6] = b"\xFD7zXZ\0";

fn le32(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

/// One xz stream's blocks into `out`; what follows its footer.
fn xz_stream<'d>(data: &'d [u8], limit: u64, out: &mut Vec<u8>) -> Option<&'d [u8]> {
    use crate::dwarf_backend::crc32;
    let header = data.get(..12)?;
    if &header[..6] != XZ_MAGIC || header[6] != 0 || le32(&header[8..])? != crc32(&header[6..8]) {
        return None;
    }
    let check = header[7] & 0x0F;
    if header[7] & 0xF0 != 0 {
        return None;
    }
    let check_size = match check {
        0 => 0,
        1..=3 => 4,
        4..=6 => 8,
        7..=9 => 16,
        _ => 32,
    };
    let mut at = 12usize;
    let mut blocks = 0u64;
    loop {
        let size_byte = *data.get(at)?;
        if size_byte == 0 {
            break;
        }
        let header_size = (usize::from(size_byte) + 1) * 4;
        let bh = data.get(at..at + header_size)?;
        let (body, crc) = bh.split_at(header_size - 4);
        if le32(crc)? != crc32(body) {
            return None;
        }
        let flags = body[1];
        // Reserved bits, or more than one filter.
        if flags & 0x3F != 0 {
            return None;
        }
        let mut p = 2usize;
        let compressed = if flags & 0x40 != 0 {
            Some(varint(body, &mut p)?)
        } else {
            None
        };
        let uncompressed = if flags & 0x80 != 0 {
            Some(varint(body, &mut p)?)
        } else {
            None
        };
        // The one filter: LZMA2 (0x21), with its dictionary size.
        let (id, props) = (varint(body, &mut p)?, varint(body, &mut p)?);
        if id != 0x21 || props != 1 {
            return None;
        }
        let dict = lzma2_dict(*body.get(p)?)?;
        p += 1;
        if body.get(p..)?.iter().any(|&b| b != 0) {
            return None;
        }
        at += header_size;
        let start_len = out.len();
        let consumed = lzma2(data.get(at..)?, dict, limit, out)?;
        let produced = (out.len() - start_len) as u64;
        if compressed.is_some_and(|c| c != consumed as u64)
            || uncompressed.is_some_and(|u| u != produced)
        {
            return None;
        }
        at += consumed;
        // Block padding to a multiple of four, then the check.
        while !at.is_multiple_of(4) {
            if *data.get(at)? != 0 {
                return None;
            }
            at += 1;
        }
        let stored = data.get(at..at + check_size)?;
        let block = &out[start_len..];
        let ok = match check {
            1 => stored == crc32(block).to_le_bytes(),
            4 => stored == crc64(block).to_le_bytes(),
            _ => true,
        };
        if !ok {
            return None;
        }
        at += check_size;
        blocks += 1;
    }
    // The index: its records (one per block), then the footer.
    let index_start = at;
    let mut p = at + 1;
    let records = varint(data, &mut p)?;
    if records != blocks {
        return None;
    }
    for _ in 0..records {
        varint(data, &mut p)?;
        varint(data, &mut p)?;
    }
    while !p.is_multiple_of(4) {
        if *data.get(p)? != 0 {
            return None;
        }
        p += 1;
    }
    if le32(data.get(p..)?)? != crc32(&data[index_start..p]) {
        return None;
    }
    p += 4;
    let footer = data.get(p..p + 12)?;
    if &footer[10..12] != b"YZ"
        || footer[8..10] != header[6..8]
        || le32(footer)? != crc32(&footer[4..10])
    {
        return None;
    }
    let backward = (u64::from(le32(&footer[4..])?) + 1) * 4;
    if backward != (p - index_start) as u64 {
        return None;
    }
    Some(&data[p + 12..])
}

/// An xz multibyte integer at `*p` (at most nine bytes, in its shortest
/// form).
fn varint(data: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..9 {
        let b = *data.get(*p)?;
        *p += 1;
        v |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return (i == 0 || b != 0).then_some(v);
        }
    }
    None
}

/// LZMA2's dictionary size from its property byte (at most [`MAX_DICT`]).
fn lzma2_dict(b: u8) -> Option<u32> {
    if b >= 40 {
        return None;
    }
    let size = (2 | u32::from(b & 1)) << (b / 2 + 11);
    (size <= MAX_DICT).then_some(size)
}

/// Decompresses the LZMA2 data at the start of `data` into `out`
/// (failing past `limit` bytes in all); how many bytes of `data` it took.
fn lzma2(data: &[u8], dict: u32, limit: u64, out: &mut Vec<u8>) -> Option<usize> {
    use std::io::Read;
    /// The input, counting what the decoder takes.
    struct Counted<'d> {
        data: &'d [u8],
        at: usize,
    }
    impl Read for Counted<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }
    let room = limit.checked_sub(out.len() as u64)?;
    let mut reader = lzma_rust2::Lzma2Reader::new(Counted { data, at: 0 }, dict, None);
    (&mut reader).take(room + 1).read_to_end(out).ok()?;
    if out.len() as u64 > limit {
        return None;
    }
    Some(reader.into_inner().at)
}

/// The CRC-64 xz checks with (ECMA-182, reflected).
fn crc64(data: &[u8]) -> u64 {
    static TABLE: std::sync::OnceLock<[u64; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, e) in (0u64..).zip(t.iter_mut()) {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xC96C_5795_D787_0F42 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u64, |crc, &b| {
        table[((crc ^ u64::from(b)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

/// A GNU `.zdebug_*` section's contents, decompressed.
pub fn gnu_zdebug(data: &[u8]) -> Option<Vec<u8>> {
    let rest = data.strip_prefix(b"ZLIB")?;
    let size = u64::from_be_bytes(rest.get(..8)?.try_into().ok()?);
    zlib(&rest[8..], size)
}

/// A debug section's name without its prefix, and whether it is in the
/// GNU compressed form: `.debug_info` is `info`, `.zdebug_info` is `info`
/// (compressed).
pub fn debug_name<'a>(name: &'a [u8], plain: &[u8], gnu: &[u8]) -> Option<(&'a [u8], bool)> {
    if let Some(n) = name.strip_prefix(plain) {
        return Some((n, false));
    }
    name.strip_prefix(gnu).map(|n| (n, true))
}

/// An ELF file's DWARF sections, compressed ones decompressed: each with
/// its name (prefix stripped), contents, and, if the contents are the
/// file's own bytes, where they start in the input (`file_offset` plus
/// the section's offset).
pub fn elf_dwarf_sections<'d>(
    elf: &ElfFile<'d>,
    file_offset: Option<u64>,
) -> Vec<DebugSection<'d>> {
    let mut out = Vec::new();
    for sec in elf.sections() {
        let Some((name, gnu)) = debug_name(sec.name, b".debug_", b".zdebug_") else {
            continue;
        };
        let data = elf.section_data(&sec);
        let mut section: DebugSection<'d> = if gnu {
            let Some(d) = gnu_zdebug(data) else { continue };
            (name, d.into(), None)
        } else if sec.flags & SHF_COMPRESSED != 0 {
            let Some(d) = elf_compressed(elf, &sec, data) else {
                continue;
            };
            (name, d.into(), None)
        } else {
            (name, data.into(), file_offset.map(|o| o + sec.offset))
        };
        // An object's: with its relocations applied, as a linker would
        // (its string offsets and addresses are their addends, in RELA
        // objects).
        if elf.is_object()
            && elf.sections().any(|r| {
                matches!(
                    r.kind,
                    hexcavator_elf::consts::SHT_REL | hexcavator_elf::consts::SHT_RELA
                ) && r.info as usize == sec.index
            })
            && relocate(elf, sec.index, section.1.to_mut())
        {
            section.2 = None;
        }
        out.push(section);
    }
    out
}

/// Applies the relocations of an object's section `index` that write an
/// absolute value (an address, an offset into another section: its
/// symbol's value and the addend) to `data`, its contents. Whether any
/// were.
fn relocate(elf: &ElfFile<'_>, index: usize, data: &mut [u8]) -> bool {
    use hexcavator_elf::consts::{EM_386, EM_AARCH64, EM_ARM, EM_PPC64, EM_RISCV, EM_X86_64};
    use hexcavator_elf::consts::{SHT_REL, SHT_RELA};
    let machine = elf.header().machine;
    // The width an absolute relocation writes, by machine and type.
    let width = |kind: u32| -> Option<usize> {
        Some(match (machine, kind) {
            // R_X86_64_64, _DTPOFF64; R_X86_64_32, _32S, _DTPOFF32.
            (EM_X86_64, 1 | 17) => 8,
            (EM_X86_64, 10 | 11 | 21) => 4,
            // R_AARCH64_ABS64, _ABS32.
            (EM_AARCH64, 257) => 8,
            (EM_AARCH64, 258) => 4,
            // R_386_32, R_386_TLS_LDO_32; R_ARM_ABS32, R_ARM_TLS_LDO32.
            (EM_386, 1 | 32) | (EM_ARM, 2 | 106) => 4,
            // R_RISCV_32, _64.
            (EM_RISCV, 1) => 4,
            (EM_RISCV, 2) => 8,
            // R_PPC64_ADDR32, _ADDR64.
            (EM_PPC64, 1) => 4,
            (EM_PPC64, 38) => 8,
            _ => return None,
        })
    };
    let big = elf.endian() == hexcavator_core::Endian::Big;
    let sections: Vec<_> = elf.sections().collect();
    // (Where a linker would lay the sections out: so the object's
    // addresses don't overlap, as its image's sections don't.)
    let addresses = elf.section_addresses();
    let mut any = false;
    for rel in sections
        .iter()
        .filter(|s| matches!(s.kind, SHT_REL | SHT_RELA) && s.info as usize == index)
    {
        let symbols = sections
            .get(rel.link as usize)
            .map(|t| elf.symbols(t))
            .unwrap_or_default();
        for r in elf.relocations(rel) {
            let Some(w) = width(r.kind) else { continue };
            let (Ok(at), Some(sym)) = (usize::try_from(r.offset), symbols.get(r.symbol as usize))
            else {
                continue;
            };
            let Some(place) = data.get_mut(at..at.saturating_add(w)) else {
                continue;
            };
            let read = |b: &[u8]| -> u64 {
                if big {
                    b.iter().fold(0, |v, &x| (v << 8) | u64::from(x))
                } else {
                    b.iter().rev().fold(0, |v, &x| (v << 8) | u64::from(x))
                }
            };
            // (REL: the addend is the bytes it patches.)
            let addend = r.addend.map_or_else(|| read(place), i64::cast_unsigned);
            let value = elf.symbol_address(sym, &addresses).wrapping_add(addend);
            let bytes = if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            };
            // (The low `w` bytes, in the image's order.)
            let low = if big { &bytes[8 - w..] } else { &bytes[..w] };
            place.copy_from_slice(low);
            any = true;
        }
    }
    any
}

/// Applies the relocations of a COFF object's section `index` (0-based)
/// that write an absolute value to `data`, its contents: an address
/// (`ADDR64`, `ADDR32`, `ADDR32NB`: its symbol's, as the object's sections
/// are laid out, see [`PeFile::section_addresses`]), an offset in a
/// section (`SECREL`), or a section's number (`SECTION`); each address
/// and offset plus the bytes it patches (COFF's addend). Whether any were.
pub fn coff_relocate(pe: &PeFile<'_>, index: usize, data: &mut [u8]) -> bool {
    use hexcavator_pe::consts::machine as m;
    #[derive(Clone, Copy)]
    enum Write {
        Address(usize),
        SectionOffset,
        Section,
    }
    let machine = pe.machine();
    let write = |kind: u16| -> Option<Write> {
        Some(match (machine, kind) {
            (m::AMD64, 1) | (m::ARM64 | m::ARM64EC | m::ARM64X, 0x0E) => Write::Address(8),
            (m::AMD64, 2 | 3)
            | (m::I386, 6 | 7)
            | (m::ARM64 | m::ARM64EC | m::ARM64X | m::ARM | m::ARMNT | m::THUMB, 1 | 2) => {
                Write::Address(4)
            }
            (m::AMD64 | m::I386, 0x0B)
            | (m::ARM64 | m::ARM64EC | m::ARM64X, 0x08)
            | (m::ARM | m::ARMNT | m::THUMB, 0x0F) => Write::SectionOffset,
            (m::AMD64 | m::I386, 0x0A)
            | (m::ARM64 | m::ARM64EC | m::ARM64X, 0x0D)
            | (m::ARM | m::ARMNT | m::THUMB, 0x0E) => Write::Section,
            _ => return None,
        })
    };
    let (Some(header), Ok(Some(symbols))) = (pe.sections().get(index), pe.symbols()) else {
        return false;
    };
    let addresses = pe.section_addresses();
    let mut any = false;
    for r in pe.section_relocations(header) {
        let Some(w) = write(r.kind.get()) else {
            continue;
        };
        let (Ok(at), Some(sym)) = (
            usize::try_from(r.virtual_address.get()),
            symbols.get(r.symbol_table_index.get() as usize),
        ) else {
            continue;
        };
        let width = match w {
            Write::Address(n) => n,
            Write::SectionOffset => 4,
            Write::Section => 2,
        };
        let Some(place) = data.get_mut(at..at.saturating_add(width)) else {
            continue;
        };
        let mut addend = [0u8; 8];
        addend[..width].copy_from_slice(place);
        let addend = u64::from_le_bytes(addend);
        let section = usize::try_from(sym.section)
            .ok()
            .and_then(|n| n.checked_sub(1));
        let value = match w {
            Write::Address(_) => section
                .and_then(|n| addresses.get(n))
                .copied()
                .unwrap_or(0)
                .wrapping_add(u64::from(sym.value))
                .wrapping_add(addend),
            Write::SectionOffset => u64::from(sym.value).wrapping_add(addend),
            Write::Section => u64::try_from(sym.section).unwrap_or(0),
        };
        place.copy_from_slice(&value.to_le_bytes()[..width]);
        any = true;
    }
    any
}

fn elf_compressed(elf: &ElfFile<'_>, sec: &Section<'_>, data: &[u8]) -> Option<Vec<u8>> {
    let c = elf.compression(sec)?;
    let body = data.get(usize::try_from(c.data_offset).ok()?..)?;
    match c.kind {
        ELFCOMPRESS_ZLIB => zlib(body, c.size),
        ELFCOMPRESS_ZSTD => zstd(body, c.size),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_limits() {
        let text: Vec<u8> = (0..10_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&text, 6);
        assert_eq!(zlib(&z, text.len() as u64).as_deref(), Some(&text[..]));
        // A size that lies, either way, is rejected.
        assert_eq!(zlib(&z, text.len() as u64 - 1), None);
        assert_eq!(zlib(&z, text.len() as u64 + 1), None);
        assert_eq!(zlib(b"not zlib", 10), None);
        assert_eq!(zlib(&z, MAX_SIZE + 1), None);
        let mut gnu = b"ZLIB".to_vec();
        gnu.extend((text.len() as u64).to_be_bytes());
        gnu.extend(&z);
        assert_eq!(gnu_zdebug(&gnu).as_deref(), Some(&text[..]));
        assert_eq!(gnu_zdebug(b"ZLIB\0\0"), None);
        assert_eq!(gnu_zdebug(b"ZSTD"), None);
        // zstd: a frame from the fixture generator's library is in the
        // compressed fixture; here, malformed input fails cleanly.
        assert_eq!(zstd(b"garbage", 10), None);
        assert_eq!(zstd(&[], 0), None);
        for n in 0..z.len() {
            let _ = zlib(&z[..n], text.len() as u64);
        }
        assert_eq!(
            debug_name(b".debug_info", b".debug_", b".zdebug_"),
            Some((&b"info"[..], false))
        );
        assert_eq!(
            debug_name(b".zdebug_line", b".debug_", b".zdebug_"),
            Some((&b"line"[..], true))
        );
        assert_eq!(debug_name(b".text", b".debug_", b".zdebug_"), None);
    }
}
