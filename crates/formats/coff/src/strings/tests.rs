use alloc::vec;
use alloc::vec::Vec;

use super::*;

fn requests<'a>(names: &[&'a [u8]]) -> Vec<Request<'a>> {
    names
        .iter()
        .map(|&name| Request { name, hint: None })
        .collect()
}

/// The table's strings, in order.
fn strings(table: &[u8]) -> Vec<&[u8]> {
    table[4..]
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn get_reads_to_the_nul_or_the_end() {
    let table = b"\x10\0\0\0first\0second";
    assert_eq!(get(table, 4), Some(&b"first"[..]));
    assert_eq!(get(table, 6), Some(&b"rst"[..]));
    assert_eq!(get(table, 9), Some(&b""[..]));
    // The last string has no NUL: it ends with the table.
    assert_eq!(get(table, 10), Some(&b"second"[..]));
    // Offset 0 is the empty name; the size field holds no names.
    assert_eq!(get(table, 0), Some(&b""[..]));
    assert_eq!(get(table, 1), None);
    assert_eq!(get(table, 3), None);
    assert_eq!(get(table, 16), None);
    assert_eq!(get(table, u32::MAX), None);
}

#[test]
fn section_name_fields() {
    assert_eq!(&section_name_field(4), b"/4\0\0\0\0\0\0");
    assert_eq!(&section_name_field(1234), b"/1234\0\0\0");
    assert_eq!(&section_name_field(9_999_999), b"/9999999");
    // Above seven digits, LLVM's base 64: 10,000,000 is 38 9 26 0.
    assert_eq!(&section_name_field(10_000_000), b"//AAmJaA");
    assert_eq!(&section_name_field(64u32.pow(5) * 3), b"//DAAAAA");
    assert_eq!(&section_name_field(u32::MAX), b"//D/////");
    for offset in [
        4,
        9,
        10,
        99,
        100,
        9_999_999,
        10_000_000,
        0x7FFF_FFFF,
        u32::MAX,
    ] {
        assert_eq!(
            section_name_offset(&section_name_field(offset)),
            Some(offset),
            "{offset}"
        );
    }
    // Not long names.
    assert_eq!(section_name_offset(b".text\0\0\0"), None);
    assert_eq!(section_name_offset(b"/\0\0\0\0\0\0\0"), None);
    assert_eq!(section_name_offset(b"//\0\0\0\0\0\0"), None);
    assert_eq!(section_name_offset(b"/12a\0\0\0\0"), None);
    assert_eq!(section_name_offset(b"//AA*\0\0\0"), None);
    // Leading zeros, and what follows the NUL.
    assert_eq!(section_name_offset(b"/0004\0\0\0"), Some(4));
    assert_eq!(section_name_offset(b"/4\x009\0\0\0\0"), Some(4));
    // Six base-64 digits of 63 overflow 32 bits.
    assert_eq!(section_name_offset(b"////////"), None);
}

#[test]
fn msvc_gives_each_use_its_own_entry() {
    let (table, offsets) = build(
        &requests(&[b"long_symbol_name", b".rdata$ilfixup", b".rdata$ilfixup"]),
        None,
        Style::Msvc,
    );
    assert_eq!(offsets, [4, 21, 36]);
    assert_eq!(&table[..4], &51u32.to_le_bytes());
    assert_eq!(
        &table[4..],
        b"long_symbol_name\0.rdata$ilfixup\0.rdata$ilfixup\0"
    );
}

#[test]
fn an_empty_table_is_its_size() {
    for style in [Style::Msvc, Style::Llvm] {
        let (table, offsets) = build(&[], None, style);
        assert_eq!(table, 4u32.to_le_bytes());
        assert!(offsets.is_empty());
    }
}

#[test]
fn llvm_sorts_by_reversed_bytes_and_merges_tails() {
    let names: [&[u8]; 6] = [b"foo_bar", b"bar", b"xbar", b"zzz_name", b"_name", b"bar"];
    let (table, offsets) = build(&requests(&names), None, Style::Llvm);
    // Descending by bytes from the end: "xbar" ('x' after '_'), then
    // "foo_bar" with "bar" in it (not in "xbar", which isn't the string
    // before it), then "zzz_name" ('r' after 'e') with "_name" in it.
    assert_eq!(strings(&table), [&b"xbar"[..], b"foo_bar", b"zzz_name"]);
    assert_eq!(offsets, [9, 13, 4, 17, 20, 13]);
    for (name, &offset) in names.iter().zip(&offsets) {
        assert_eq!(get(&table, offset), Some(*name));
    }
}

#[test]
fn reversed_order_matches_llvm_multikey_sort() {
    // A suffix of a string sorts after it; bytes compare unsigned.
    assert_eq!(reversed_descending(b"abc", b"bc"), Ordering::Less);
    assert_eq!(reversed_descending(b"bc", b"abc"), Ordering::Greater);
    assert_eq!(reversed_descending(b"ab", b"ac"), Ordering::Greater);
    assert_eq!(reversed_descending(b"a\xFF", b"a\x01"), Ordering::Less);
    assert_eq!(reversed_descending(b"same", b"same"), Ordering::Equal);
    assert_eq!(reversed_descending(b"", b"x"), Ordering::Greater);
}

#[test]
fn a_recorded_table_is_kept_as_it_was() {
    // Quirks of real tables: a duplicate entry (MSVC), a string nothing
    // uses, a name used from another's middle (LLVM), and a size field of
    // 0 (cvtres).
    let recorded = b"\0\0\0\0dup\0dup\0unused\0tail_merged\0";
    let r = [
        Request {
            name: b"dup",
            hint: Some(8),
        },
        Request {
            name: b"dup",
            hint: Some(4),
        },
        Request {
            name: b"merged",
            hint: Some(24),
        },
    ];
    let (table, offsets) = build(&r, Some(recorded), Style::Msvc);
    assert_eq!(table, recorded);
    assert_eq!(offsets, [8, 4, 24]);
}

#[test]
fn names_a_recorded_table_lacks_are_appended() {
    let recorded = b"\x0F\0\0\0first\0second";
    let r = [
        // A stale hint: the name moved.
        Request {
            name: b"second",
            hint: Some(4),
        },
        Request {
            name: b"new_name",
            hint: None,
        },
        Request {
            name: b"first",
            hint: Some(4),
        },
        Request {
            name: b"new_name",
            hint: Some(99),
        },
    ];
    let (table, offsets) = build(&r, Some(recorded), Style::Llvm);
    // The last string gets the NUL it lacked before anything follows it.
    assert_eq!(&table[4..], b"first\0second\0new_name\0");
    assert_eq!(&table[..4], &(table.len() as u32).to_le_bytes());
    assert_eq!(offsets, [10, 17, 4, 17]);
}

#[test]
fn appended_names_reuse_whole_entries_only() {
    let recorded = b"\x0C\0\0\0abc\0xbc\0";
    let (table, offsets) = build(&requests(&[b"bc", b"xbc"]), Some(recorded), Style::Msvc);
    assert_eq!(offsets, [12, 8]);
    assert_eq!(&table[4..], b"abc\0xbc\0bc\0");
    let (table, offsets) = build(&requests(&[]), Some(recorded), Style::Msvc);
    assert_eq!((&table[..], offsets), (&recorded[..], vec![]));
}
