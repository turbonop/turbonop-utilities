//! What tells one named type from another across compile units, as
//! dsymutil (classic, with ODR uniquing) tells them apart.
//!
//! dsymutil gives each named class, structure, union, enumeration,
//! typedef, and namespace a "declaration context"
//! (`DeclContextTree::getChildDeclContext`, in LLVM's
//! `DWARFLinker/Classic/DWARFLinkerDeclContext.cpp`): its parent's
//! context, its tag, its name (or linkage name), and, but for namespaces,
//! its byte size, the resolved path of its `DW_AT_decl_file`, and its
//! `DW_AT_decl_line`. The first complete definition of a context it keeps
//! is the one every later reference to the context is pointed at. A key
//! here hashes the same parts (the file as its directory and name, the
//! directory index 0 as the unit's compilation directory), so that two
//! units' definitions with one key are, for dsymutil, one type. Keys that
//! differ where dsymutil's contexts would be one (a path spelled two
//! ways) only make a synthesized member carry a definition it could have
//! left out.

use crate::hash::H128;

/// The key of the unit (the root of every chain).
pub(crate) const ROOT: u128 = 0;

/// The key of a DIE whose parent's key is `parent`.
pub(crate) fn of(
    parent: u128,
    tag: u16,
    name: &[u8],
    byte_size: Option<u64>,
    file: u128,
    line: u64,
) -> u128 {
    let mut h = H128::new();
    h.u128(parent);
    h.u64(u64::from(tag));
    h.bytes(name);
    h.u64(byte_size.unwrap_or(u64::MAX));
    h.u128(file);
    h.u64(line);
    h.finish()
}

/// Each file of the DWARF 2 to 4 line table at `off` of `line`, by its
/// index (index 0, which names no file before DWARF 5, is 0): its
/// directory's name and its own hashed, the directory index 0 being
/// `comp_dir`. `None` if the header doesn't read, or is DWARF 5's.
pub(crate) fn files(line: &[u8], off: u64, comp_dir: &[u8]) -> Option<Vec<u128>> {
    let mut at = usize::try_from(off).ok()?;
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(line.get(at..at + 4)?.try_into().ok()?))
    };
    let len = u32_at(at)? as usize;
    if len >= 0xFFFF_FFF0 {
        return None;
    }
    let end = at.checked_add(4)?.checked_add(len)?;
    let b = line.get(..end)?;
    at += 4;
    let version = u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?);
    if !(2..=4).contains(&version) {
        return None;
    }
    at += 2 + 4; // header_length
    at += 1; // minimum_instruction_length
    if version >= 4 {
        at += 1; // maximum_operations_per_instruction
    }
    at += 3; // default_is_stmt, line_base, line_range
    let opcode_base = *b.get(at)?;
    at += 1 + usize::from(opcode_base.checked_sub(1)?);
    let cstr = |at: &mut usize| -> Option<&[u8]> {
        let rest = b.get(*at..)?;
        let z = rest.iter().position(|&c| c == 0)?;
        *at += z + 1;
        Some(&rest[..z])
    };
    let uleb = |at: &mut usize| -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let c = *b.get(*at)?;
            *at += 1;
            if shift < 64 {
                v |= u64::from(c & 0x7F) << shift;
            }
            if c & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
        }
    };
    let mut dirs: Vec<&[u8]> = vec![comp_dir];
    loop {
        let d = cstr(&mut at)?;
        if d.is_empty() {
            break;
        }
        dirs.push(d);
    }
    let mut out = vec![0u128];
    loop {
        let name = cstr(&mut at)?;
        if name.is_empty() {
            break;
        }
        let dir = uleb(&mut at)?;
        uleb(&mut at)?; // modification time
        uleb(&mut at)?; // length
        let mut h = H128::new();
        let d = usize::try_from(dir).ok().and_then(|d| dirs.get(d)).copied();
        h.bytes(d.unwrap_or(b"\0?"));
        h.bytes(name);
        out.push(h.finish());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DWARF 4 line table header with `dirs` and `files` (name, dir).
    fn table(dirs: &[&[u8]], files: &[(&[u8], u8)]) -> Vec<u8> {
        let mut h = vec![1, 1, 1, 0xFB, 14, 13];
        h.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
        for d in dirs {
            h.extend_from_slice(d);
            h.push(0);
        }
        h.push(0);
        for (n, d) in files {
            h.extend_from_slice(n);
            h.extend_from_slice(&[0, *d, 0, 0]);
        }
        h.push(0);
        let mut b = Vec::new();
        b.extend_from_slice(&((2 + 4 + h.len()) as u32).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&(h.len() as u32).to_le_bytes());
        b.extend_from_slice(&h);
        b
    }

    #[test]
    fn files_by_directory_and_name() {
        let a = table(&[b"/usr/include"], &[(b"a.cpp", 0), (b"stdio.h", 1)]);
        let b = table(
            &[b"/opt", b"/usr/include"],
            &[(b"stdio.h", 2), (b"b.cpp", 0), (b"x.h", 1)],
        );
        let fa = files(&a, 0, b"/build").unwrap();
        let fb = files(&b, 0, b"/build").unwrap();
        assert_eq!(fa.len(), 3);
        assert_eq!(fb.len(), 4);
        assert_eq!(fa[2], fb[1]);
        assert_ne!(fa[1], fb[2]);
        assert_ne!(fa[2], fb[3]);
        // The compilation directory is directory 0's name.
        let fc = files(&a, 0, b"/elsewhere").unwrap();
        assert_ne!(fa[1], fc[1]);
        assert_eq!(fa[2], fc[2]);
        // A table at an offset, after another.
        let mut two = b.clone();
        two.extend_from_slice(&a);
        assert_eq!(files(&two, b.len() as u64, b"/build").unwrap(), fa);
        // Damaged or DWARF 5: none.
        assert!(files(&a[..a.len() - 3], 0, b"").is_none());
        let mut v5 = a.clone();
        v5[4] = 5;
        assert!(files(&v5, 0, b"").is_none());
    }

    #[test]
    fn keys_by_every_part() {
        let k = of(ROOT, 0x13, b"S", Some(8), 1, 10);
        assert_eq!(k, of(ROOT, 0x13, b"S", Some(8), 1, 10));
        for other in [
            of(5, 0x13, b"S", Some(8), 1, 10),
            of(ROOT, 0x02, b"S", Some(8), 1, 10),
            of(ROOT, 0x13, b"T", Some(8), 1, 10),
            of(ROOT, 0x13, b"S", Some(4), 1, 10),
            of(ROOT, 0x13, b"S", None, 1, 10),
            of(ROOT, 0x13, b"S", Some(8), 2, 10),
            of(ROOT, 0x13, b"S", Some(8), 1, 11),
        ] {
            assert_ne!(k, other);
        }
    }
}
