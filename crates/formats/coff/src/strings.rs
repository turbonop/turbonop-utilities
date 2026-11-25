//! The string table: reading names from it, and building one.
//!
//! The table starts with its size (its own 4 bytes included), and offsets
//! count from that size field. A long section name refers to it as
//! `/<decimal offset>`, or, for offsets above 9,999,999, LLVM's
//! `//<6 base-64 digits>`. A long symbol name is four zero bytes and the
//! offset.

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::object::Style;

/// The NUL-terminated string at `offset` in `table` (up to the end of the
/// table, if it has no NUL). Offset 0, which a symbol's all-zero name
/// field holds, is the empty name.
pub(crate) fn get(table: &[u8], offset: u32) -> Option<&[u8]> {
    let o = usize::try_from(offset).ok()?;
    if o == 0 {
        return Some(&[]);
    }
    if o < 4 {
        return None;
    }
    let rest = table.get(o..)?;
    if rest.is_empty() {
        return None;
    }
    let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(&rest[..len])
}

/// The string-table offset a section's name field refers to, if it is of
/// the form `/<decimal>` or `//<base 64>`.
pub(crate) fn section_name_offset(field: &[u8; 8]) -> Option<u32> {
    let len = field.iter().position(|&b| b == 0).unwrap_or(8);
    match &field[..len] {
        [b'/', b'/', digits @ ..] if !digits.is_empty() => {
            digits.iter().try_fold(0u32, |acc, &d| {
                let value = match d {
                    b'A'..=b'Z' => d - b'A',
                    b'a'..=b'z' => d - b'a' + 26,
                    b'0'..=b'9' => d - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return None,
                };
                acc.checked_mul(64)?.checked_add(u32::from(value))
            })
        }
        [b'/', digits @ ..] if !digits.is_empty() => digits.iter().try_fold(0u32, |acc, &d| {
            d.is_ascii_digit()
                .then(|| acc.checked_mul(10)?.checked_add(u32::from(d - b'0')))?
        }),
        _ => None,
    }
}

/// The name field of a section whose name is at `offset` in the string
/// table: `/<decimal>` (NUL-padded) up to 9,999,999, and LLVM's
/// `//<base 64>` above.
pub(crate) fn section_name_field(offset: u32) -> [u8; 8] {
    let mut field = [0u8; 8];
    if offset <= 9_999_999 {
        let mut digits = [0u8; 7];
        let mut n = offset;
        let mut len = 0;
        loop {
            digits[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        field[0] = b'/';
        for i in 0..len {
            field[1 + i] = digits[len - 1 - i];
        }
    } else {
        const DIGITS: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        field[0] = b'/';
        field[1] = b'/';
        let mut n = offset;
        for i in (2..8).rev() {
            field[i] = DIGITS[(n % 64) as usize];
            n /= 64;
        }
    }
    field
}

/// A name the table must hold, and where it was in the table read, if
/// anywhere.
pub(crate) struct Request<'a> {
    pub name: &'a [u8],
    pub hint: Option<u32>,
}

/// Builds the string table for `requests` (symbol names in order, then
/// section names), and gives each request's offset.
///
/// With `recorded` (the table as read), the table is that table, with any
/// names it lacks appended (its size field is rewritten only then); a
/// request whose hint still names its string there keeps it. Otherwise the
/// table is built afresh in `style`.
pub(crate) fn build(
    requests: &[Request<'_>],
    recorded: Option<&[u8]>,
    style: Style,
) -> (Vec<u8>, Vec<u32>) {
    match recorded {
        Some(table) => extend(requests, table),
        None => match style {
            Style::Msvc => per_use(requests),
            Style::Llvm => tail_merged(requests),
        },
    }
}

/// The table as read, with the names it lacks appended.
fn extend(requests: &[Request<'_>], table: &[u8]) -> (Vec<u8>, Vec<u32>) {
    let mut offsets = alloc::vec![0u32; requests.len()];
    let mut missing = Vec::new();
    for (i, r) in requests.iter().enumerate() {
        match r.hint {
            Some(at) if get(table, at) == Some(r.name) && at != 0 => offsets[i] = at,
            _ => missing.push(i),
        }
    }
    let mut out = table.to_vec();
    if missing.is_empty() {
        return (out, offsets);
    }
    // The table's strings, by content, to reuse an entry that has the name.
    let mut existing: Vec<(&[u8], u32)> = Vec::new();
    let mut o = 4;
    while o < table.len() {
        let len = table[o..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(table.len() - o);
        // Offsets past 4 GiB can't be referred to; the writer refuses such
        // tables.
        existing.push((&table[o..o + len], u32::try_from(o).unwrap_or(u32::MAX)));
        o += len + 1;
    }
    existing.sort_unstable();
    // Names appended are themselves deduplicated, in first-use order.
    let names: Vec<&[u8]> = missing.iter().map(|&i| requests[i].name).collect();
    let leads = leads(&names);
    if out.len() < 4 {
        out.resize(4, 0);
    }
    if out.len() > 4 && out.last().is_some_and(|&b| b != 0) {
        out.push(0);
    }
    for (k, &i) in missing.iter().enumerate() {
        let name = names[k];
        let lead = leads[k] as usize;
        offsets[i] = if lead == k {
            let at = existing
                .binary_search_by(|e| e.0.cmp(name).then(Ordering::Greater))
                .unwrap_err();
            match existing.get(at).filter(|e| e.0 == name) {
                Some(&(_, at)) => at,
                None => {
                    let at = u32::try_from(out.len()).unwrap_or(u32::MAX);
                    out.extend_from_slice(name);
                    out.push(0);
                    at
                }
            }
        } else {
            offsets[missing[lead]]
        };
    }
    let size = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[..4].copy_from_slice(&size.to_le_bytes());
    (out, offsets)
}

/// For each of `names`, the position of the first name equal to it.
fn leads(names: &[&[u8]]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..names.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| names[a as usize].cmp(names[b as usize]).then(a.cmp(&b)));
    let mut leads = alloc::vec![0u32; names.len()];
    let mut i = 0;
    while i < order.len() {
        let lead = order[i];
        let mut j = i;
        while j < order.len() && names[order[j] as usize] == names[lead as usize] {
            leads[order[j] as usize] = lead;
            j += 1;
        }
        i = j;
    }
    leads
}

/// MSVC's table: an entry for each use, in order, the same name as often
/// as it is used.
fn per_use(requests: &[Request<'_>]) -> (Vec<u8>, Vec<u32>) {
    let mut out = alloc::vec![0u8; 4];
    let mut offsets = Vec::with_capacity(requests.len());
    for r in requests {
        offsets.push(u32::try_from(out.len()).unwrap_or(u32::MAX));
        out.extend_from_slice(r.name);
        out.push(0);
    }
    let size = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[..4].copy_from_slice(&size.to_le_bytes());
    (out, offsets)
}

/// Orders `a` and `b` by their bytes from the last, a string before every
/// string it ends (LLVM's `multikeySort`, which sorts so that strings
/// sharing a suffix are adjacent, longest first).
fn reversed_descending(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (a.len(), b.len());
    loop {
        match (i, j) {
            (0, 0) => return Ordering::Equal,
            (0, _) => return Ordering::Greater,
            (_, 0) => return Ordering::Less,
            _ => {}
        }
        i -= 1;
        j -= 1;
        match b[j].cmp(&a[i]) {
            Ordering::Equal => {}
            other => return other,
        }
    }
}

/// LLVM's table (`StringTableBuilder::finalize` for COFF): each distinct
/// name once, sorted so that a name that ends the one before it is stored
/// as a pointer into it.
fn tail_merged(requests: &[Request<'_>]) -> (Vec<u8>, Vec<u32>) {
    let mut order: Vec<u32> = (0..requests.len() as u32).collect();
    order.sort_by(|&a, &b| {
        reversed_descending(requests[a as usize].name, requests[b as usize].name)
    });
    let mut out = alloc::vec![0u8; 4];
    let mut offsets = alloc::vec![0u32; requests.len()];
    let mut previous: &[u8] = &[];
    let mut previous_at = 0u32;
    for &i in &order {
        let name = requests[i as usize].name;
        offsets[i as usize] = if !out[4..].is_empty() && previous.ends_with(name) {
            // The previous string's end, less this one's length.
            previous_at + (previous.len() - name.len()) as u32
        } else {
            let at = u32::try_from(out.len()).unwrap_or(u32::MAX);
            out.extend_from_slice(name);
            out.push(0);
            previous = name;
            previous_at = at;
            at
        };
    }
    let size = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[..4].copy_from_slice(&size.to_le_bytes());
    (out, offsets)
}

#[cfg(test)]
mod tests;
