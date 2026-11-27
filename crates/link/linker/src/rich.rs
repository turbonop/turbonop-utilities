//! The Rich header: between the DOS stub and the PE header, a list of the
//! tools that made the image's inputs (each object's `@comp.id`, with how
//! many objects each made) and the linker, XORed with a checksum. As
//! `link.exe` builds it (its `IMAGE::CbBuildProdidBlock`):
//!
//! - The list starts with the linker's own entry; each tool not seen yet is
//!   put at the front, walking the inputs library by library (the command
//!   line's objects first, then each library's in the order they loaded),
//!   so the list is newest first, the linker last. An input without a
//!   `@comp.id`, and every import object, counts as `0x00010000`.
//! - The checksum: the stub's length, plus each of its bytes rotated left
//!   by its offset (`e_lfanew` zero), plus each entry's ID rotated left by
//!   its count.
//! - After the entries and `Rich`, zero padding of 1 to 3 entries' room,
//!   `(checksum >> 5) % 3 + 1`, as many as fit in the headers without
//!   making them larger.

use crate::resolve::{FileKind, Resolved};

/// link.exe 14.52.37029's own entry (product 0x102, build 37029): the
/// linker whose output turbolink's is.
pub const LINKER_COMP_ID: u32 = 0x0102_90A5;

/// link.exe 14.52.37029's export object's entry (product 0x100): what it
/// makes of an image's exports (its .edata, and the .exp file), seen
/// before every input.
pub const EXPORT_COMP_ID: u32 = 0x0100_90A5;

/// What an input without a `@comp.id` counts as.
const UNMARKED: u32 = 0x0001_0000;

/// The DOS stub's length: where the Rich header starts.
pub const STUB: u32 = 0x80;

/// The Rich header's entries (ID, count), in their order.
pub fn entries(r: &Resolved) -> Vec<(u32, u32)> {
    let id = |f: &crate::resolve::File| match &f.kind {
        FileKind::Object { object, .. } => object.comp_id.unwrap_or(UNMARKED),
        FileKind::Import { .. } => UNMARKED,
    };
    // The inputs, library by library: the command line's first.
    let libraries = r
        .files
        .iter()
        .filter_map(|f| f.library)
        .max()
        .map_or(0, |m| m + 1);
    let mut walk: Vec<u32> = Vec::new();
    if !r.config.exports.is_empty() {
        walk.push(EXPORT_COMP_ID);
    }
    walk.extend(r.files.iter().filter(|f| f.library.is_none()).map(id));
    for lib in 0..libraries {
        walk.extend(r.files.iter().filter(|f| f.library == Some(lib)).map(id));
    }
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for v in walk {
        match seen.iter_mut().find(|e| e.0 == v) {
            Some(e) => e.1 += 1,
            None => seen.push((v, 1)),
        }
    }
    seen.reverse();
    seen.push((LINKER_COMP_ID, 1));
    seen
}

/// The checksum of the stub (`dos`, its `e_lfanew` zero) and `entries`.
pub fn key(dos: &[u8], entries: &[(u32, u32)]) -> u32 {
    let mut k = STUB;
    for (i, &b) in dos.iter().enumerate().take(STUB as usize) {
        k = k.wrapping_add(u32::from(b).rotate_left(crate::n32(i)));
    }
    for &(id, count) in entries {
        k = k.wrapping_add(id.rotate_left(count & 0xFF));
    }
    k
}

/// How many empty entries of padding follow, for `entries` entries in
/// headers with `sections` section headers, aligned to `file_align`.
pub fn padding(key: u32, entries: usize, sections: usize, is64: bool, file_align: u32) -> u32 {
    let wanted = (key >> 5) % 3 + 1;
    let rich = crate::n32(entries) * 8 + 0x18;
    let optional: u32 = if is64 { 0xF0 } else { 0xE0 };
    // The headers' end (less one), rounded down to the file alignment:
    // the aligned headers' size, less one alignment.
    let end = optional + 0x17 + STUB + crate::n32(sections) * 40 + rich;
    let aligned = end - end % file_align.max(1);
    let used = optional + STUB + crate::n32(sections) * 40 + rich;
    let room = (file_align + aligned)
        .saturating_sub(used)
        .saturating_sub(0x18)
        / 8;
    wanted.min(room)
}

/// The PE header's offset after a Rich header of `entries` and `padding`.
pub fn pe_offset(entries: usize, padding: u32) -> u32 {
    STUB + (crate::n32(entries) + padding) * 8 + 0x18
}

/// Writes the Rich header at the start of `out` (offset 0x80 of the file).
pub fn write(out: &mut [u8], entries: &[(u32, u32)], key: u32) {
    let mut at = 0;
    let mut put = |out: &mut [u8], v: u32| {
        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        at += 4;
    };
    put(out, 0x536E_6144 ^ key); // "DanS"
    put(out, key);
    put(out, key);
    put(out, key);
    for &(id, count) in entries {
        put(out, id ^ key);
        put(out, count ^ key);
    }
    put(out, 0x6863_6952); // "Rich"
    put(out, key);
}
