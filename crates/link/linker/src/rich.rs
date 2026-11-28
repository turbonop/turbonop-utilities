//! The Rich header: between the DOS stub and the PE header, a list of the
//! tools that made the image's inputs (each object's `@comp.id`, with how
//! many objects each made) and the linker, XORed with a checksum. As
//! `link.exe` builds it (its `IMAGE::CbBuildProdidBlock`):
//!
//! - The list starts with the linker's own entry; each tool not seen yet is
//!   put at the front, walking the inputs as link.exe's pass 2 does: the
//!   command line's objects from the last to the first, then library by
//!   library, each one's members from the last in the archive to the first
//!   (link.exe makes the command line's modules in order, and a library's
//!   in the order of their offsets, each put at the front of its list); so
//!   the list is newest first, the linker last. An object without a
//!   `@comp.id` counts as 0, every import object as `0x00010000`.
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

/// link.exe 14.52.37029's cvtres's entry (product 0xFF): the object it
/// makes of the `.res` files, seen before every input.
pub const CVTRES_COMP_ID: u32 = 0x00FF_90A5;

/// What each `.res` file counts as when a manifest is embedded too
/// (product 0x97, build 0).
pub const RES_BESIDE_MANIFEST_COMP_ID: u32 = 0x0097_0000;

/// What an import object counts as.
const IMPORT: u32 = 0x0001_0000;

/// The DOS stub's length: where the Rich header starts.
pub const STUB: u32 = 0x80;

/// The Rich header's entries (ID, count), in their order.
pub fn entries(r: &Resolved, resources: Option<&crate::resources::Rsrc>) -> Vec<(u32, u32)> {
    let id = |f: &crate::resolve::File| match &f.kind {
        FileKind::Object { object, .. } => object.comp_id.unwrap_or(0),
        FileKind::Import { .. } => IMPORT,
    };
    let mut walk: Vec<u32> = Vec::new();
    // (An embedded manifest with .res files besides: each .res counts
    // first, as link.exe has rc.exe and cvtres rework them; then cvtres, for
    // any resources; then the export object.)
    if let Some(res) = resources {
        walk.extend(std::iter::repeat_n(
            RES_BESIDE_MANIFEST_COMP_ID,
            res.res_beside_manifest as usize,
        ));
        walk.push(CVTRES_COMP_ID);
    }
    if !r.config.exports.is_empty() {
        walk.push(EXPORT_COMP_ID);
    }
    // The command line's objects, the last first. The code generated for
    // /GL objects is the last of them (it joins the link last, though it
    // is laid out first), and counts once for each /GL object, as link.exe
    // counts it.
    let generated = |f: &&crate::resolve::File| f.name == crate::resolve::LTCG_FILE;
    if let Some(f) = r.files.iter().find(generated) {
        walk.extend(std::iter::repeat_n(id(f), r.ltcg_objects.max(1) as usize));
    }
    walk.extend(
        r.files
            .iter()
            .rev()
            .filter(|f| f.library.is_none() && !generated(f))
            .map(id),
    );
    // The libraries as they are searched: the command line's first.
    for &lib in &r.library_order {
        let mut members: Vec<&crate::resolve::File> =
            r.files.iter().filter(|f| f.library == Some(lib)).collect();
        members.sort_by_key(|f| std::cmp::Reverse(f.member));
        walk.extend(members.into_iter().map(id));
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
