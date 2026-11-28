//! `compare` on the Debug programs `tools/turbolib/equiv/build.cmd` links
//! (`/Od /Z7 /RTC1`: switch tables, inline thread_locals, a lambda,
//! exceptions, RTTI) against a library and against it folded: equivalent;
//! and each change made here to the folded program, or to its library, as a
//! fault could, caught.

use super::*;

fn equiv(name: &str) -> Files {
    (
        fixture(&format!("equiv/{name}.exe")),
        fixture(&format!("equiv/{name}.pdb")),
    )
}

/// The original program and the folded one, linked by `linker`.
fn pair(linker: &str) -> (Files, Files) {
    (
        equiv(&format!("{linker}-original")),
        equiv(&format!("{linker}-folded")),
    )
}

/// A procedure's code: its RVA and length.
fn procedure(p: &Files, name: &[u8]) -> (u32, u32) {
    let rva = public(p, name);
    let l = Linked::read(&p.0, &p.1, "the program").unwrap();
    let c = l.code.iter().find(|c| c.rva == rva).expect("a procedure");
    (c.rva, c.len)
}

/// The label (a public or a procedure) whose name holds all of `parts`.
fn label_like(p: &Files, parts: &[&[u8]]) -> Vec<u8> {
    let l = Linked::read(&p.0, &p.1, "the program").unwrap();
    l.labels
        .iter()
        .filter(|x| x.priority <= priority::PROCEDURE)
        .find(|x| {
            parts
                .iter()
                .all(|s| x.name.windows(s.len()).any(|w| w == *s))
        })
        .map_or_else(|| panic!("a label of {parts:?}"), |x| x.name.to_vec())
}

fn read(p: &Files, rva: u32, len: usize) -> Vec<u8> {
    let at = offset(p, rva);
    p.0[at..at + len].to_vec()
}

fn u32_at(p: &Files, rva: u32) -> u32 {
    u32::from_le_bytes(read(p, rva, 4).try_into().unwrap())
}

/// `p` with `bytes` written at `rva`.
fn patched(p: &Files, rva: u32, bytes: &[u8]) -> Files {
    let at = offset(p, rva);
    let mut image = p.0.clone();
    image[at..at + bytes.len()].copy_from_slice(bytes);
    (image, p.1.clone())
}

/// Asserts that `b` compared with `a` fails, in `category` if one is given.
fn caught(a: &Files, b: &Files, category: Option<&str>, what: &str) {
    let r = run(a, b);
    match category {
        Some(c) => assert!(r.failed(c), "{what}: not caught as {c}:\n{}", r.text()),
        None => assert!(!r.passed(), "{what}: not caught:\n{}", r.text()),
    }
}

/// The RVA of a switch's jump table in a procedure: the first run of
/// `count` aligned words that are RVAs in it.
fn jump_table(p: &Files, (rva, len): (u32, u32), count: usize) -> u32 {
    let start = rva.next_multiple_of(4);
    (start..=rva + len - 4 * count as u32)
        .step_by(4)
        .find(|&t| (0..count as u32).all(|k| (rva..rva + len).contains(&u32_at(p, t + 4 * k))))
        .expect("a jump table")
}

/// Where code in `(rva, len)` calls `target` (`E8 rel32`): the RVA of
/// the rel32.
fn call_of(p: &Files, (rva, len): (u32, u32), target: u32) -> u32 {
    let code = read(p, rva, len as usize);
    (0..code.len() - 5)
        .find(|&k| {
            code[k] == 0xE8 && {
                let d = i32::from_le_bytes(code[k + 1..k + 5].try_into().unwrap());
                i64::from(rva) + k as i64 + 5 + i64::from(d) == i64::from(target)
            }
        })
        .map(|k| rva + k as u32 + 1)
        .expect("a call")
}

/// `p`'s `.pdata` entry for the function at `begin`: its RVA.
fn pdata(p: &Files, begin: u32) -> u32 {
    let l = Linked::read(&p.0, &p.1, "the program").unwrap();
    let s = l.sections.iter().find(|s| s.name == b".pdata").unwrap();
    (s.rva..s.rva + s.virtual_size)
        .step_by(12)
        .find(|&e| u32_at(p, e) == begin)
        .expect("a .pdata entry")
}

/// The handler data's first word of the function at `begin` (C++'s
/// handlers keep the RVA of their `FuncInfo` there): its RVA.
fn handler_data(p: &Files, begin: u32) -> u32 {
    let unwind = u32_at(p, pdata(p, begin) + 8);
    let head = read(p, unwind, 4);
    assert!(head[0] >> 3 & 3 != 0, "a handler");
    let codes = u32::from(head[2]).next_multiple_of(2);
    // (The handler's RVA, then its data.)
    unwind + 4 + 2 * codes + 4
}

#[test]
fn debug_programs_against_a_folded_library_are_equivalent() {
    for linker in ["link", "lld"] {
        let (a, b) = pair(linker);
        let r = run(&a, &b);
        assert!(r.passed(), "{linker}:\n{}", r.text());
        assert!(
            r.finding(
                Severity::Expected,
                "turbolib's synthesized members loaded by the second link"
            )
            .is_some(),
            "{linker}: nothing folded:\n{}",
            r.text()
        );
    }
}

#[test]
fn a_jump_table_entry_sent_elsewhere_is_caught() {
    let (a, b) = pair("link");
    let f = procedure(&b, b"?classify@@YAHH@Z");
    let t = jump_table(&b, f, 8);
    // The first case's entry naming the second's.
    let second = read(&b, t + 4, 4);
    caught(
        &a,
        &patched(&b, t, &second),
        None,
        "an entry sent to another case",
    );
    // Naming where the first program's entry points, its bytes now the
    // first program's: equal bytes that name another place.
    let fa = procedure(&a, b"?classify@@YAHH@Z");
    let ta = jump_table(&a, fa, 8);
    assert_eq!(ta - fa.0, t - f.0);
    assert_ne!(fa.0, f.0, "the layouts differ");
    let theirs = read(&a, ta + 4, 4);
    caught(
        &a,
        &patched(&b, t + 4, &theirs),
        None,
        "an entry with the first program's bytes",
    );
}

#[test]
fn changed_code_in_a_procedure_with_a_switch_table_is_caught() {
    let (a, b) = pair("link");
    for name in [&b"?classify@@YAHH@Z"[..], b"?sparse@@YAHH@Z"] {
        let f = procedure(&b, name);
        let t = jump_table(&b, f, 4);
        // The byte just before the tables, and one in the middle of the code.
        for at in [t - 1, f.0 + (t - f.0) / 2] {
            let old = read(&b, at, 1)[0];
            caught(
                &a,
                &patched(&b, at, &[old ^ 0x01]),
                None,
                &format!("{} +{:#x}", show(name), at - f.0),
            );
        }
        // And a byte of the tables past the jump table (sparse's index
        // table; classify's padding).
        let at = t + 4 * 4;
        let old = read(&b, at, 1)[0];
        caught(&a, &patched(&b, at, &[old ^ 0x01]), None, "a table's byte");
    }
}

#[test]
fn a_tls_read_of_another_variable_is_caught() {
    let (a, b) = pair("link");
    let f = procedure(&b, b"?tls_mix@@YAHH@Z");
    let code = read(&b, f.0, f.1 as usize);
    // tls_mix's `mov eax, SECREL` loads: tl_first's, then tl_second's.
    let read_array = [0x65, 0x48, 0x8B, 0x14, 0x25, 0x58, 0, 0, 0];
    let loads: Vec<usize> = (0..code.len() - 14)
        .filter(|&k| code[k] == 0xB8 && code[k + 5..].windows(9).take(12).any(|w| w == read_array))
        .collect();
    assert!(loads.len() >= 2, "{loads:?}: {code:02x?}");
    let first = &code[loads[0] + 1..loads[0] + 5];
    let other = (1..loads.len())
        .map(|i| &code[loads[i] + 1..loads[i] + 5])
        .find(|s| *s != first)
        .expect("another variable's SECREL");
    caught(
        &a,
        &patched(&b, f.0 + loads[0] as u32 + 1, other),
        None,
        "tl_first read as tl_second",
    );
}

#[test]
fn a_call_of_a_function_of_the_same_code_is_caught() {
    let (a, b) = pair("link");
    let f = procedure(&b, b"?shared_all@@YAHH@Z");
    let twin_a = public(&b, b"?twin_a@@YAHH@Z");
    let twin_b = public(&b, b"?twin_b@@YAHH@Z");
    let at = call_of(&b, f, twin_b);
    let d = (i64::from(twin_a) - i64::from(at + 4)) as i32;
    caught(
        &a,
        &patched(&b, at, &d.to_le_bytes()),
        Some("code whose operands name other functions or data"),
        "twin_b called as twin_a",
    );
}

#[test]
fn a_lambda_of_other_code_under_its_name_is_caught() {
    let (a, b) = pair("link");
    let name = label_like(&b, &[b"??R<lambda_"]);
    let f = procedure(&b, &name);
    // `x * y`: the imul's operand register.
    let code = read(&b, f.0, f.1 as usize);
    let k = code
        .windows(2)
        .position(|w| w[0] == 0x0F && w[1] == 0xAF)
        .expect("an imul");
    let modrm = code[k + 2];
    caught(
        &a,
        &patched(&b, f.0 + k as u32 + 2, &[modrm ^ 0x08]),
        Some("functions or data whose bytes differ"),
        "the lambda's code",
    );
}

#[test]
fn a_changed_string_is_caught() {
    let (a, b) = pair("link");
    let l = Linked::read(&b.0, &b.1, "the program").unwrap();
    let s = l.sections.iter().find(|s| s.name == b".rdata").unwrap();
    let at = |what: &[u8]| {
        s.rva
            + s.data
                .windows(what.len())
                .position(|w| w == what)
                .unwrap_or_else(|| panic!("{}", show(what))) as u32
    };
    // A string literal, labeled.
    let hello = at(b"hello from a shared");
    caught(&a, &patched(&b, hello, b"j"), None, "the greeting");
    // A `/RTC1` variable's name (risky's `n`), unlabeled: text that the
    // naming by content doesn't read as an RVA.
    for name in [&b"\0n\0"[..], b"\0g\0"] {
        let n = at(name) + 1;
        caught(&a, &patched(&b, n, b"m"), None, &show(name));
    }
}

#[test]
fn changed_unwind_entries_are_caught() {
    let (a, b) = pair("link");
    let classify = procedure(&b, b"?classify@@YAHH@Z");
    let e = pdata(&b, classify.0);
    // The function's end.
    let end = u32_at(&b, e + 4);
    caught(
        &a,
        &patched(&b, e + 4, &(end - 1).to_le_bytes()),
        Some("unwind entries for functions of another extent"),
        "classify's end",
    );
    // Its unwind information: risky's (a handler, another frame).
    let risky = public(&b, b"?risky@@YAHH@Z");
    let other = u32_at(&b, pdata(&b, risky) + 8);
    caught(
        &a,
        &patched(&b, e + 8, &other.to_le_bytes()),
        None,
        "classify's unwind information",
    );
}

#[test]
fn changed_exception_tables_are_caught() {
    let (a, b) = pair("link");
    // risky's `FuncInfo4` (`__CxxFrameHandler4`'s, compressed: a header
    // byte, then RVAs at any byte).
    let risky = public(&b, b"?risky@@YAHH@Z");
    let info = u32_at(&b, handler_data(&b, risky));
    let head = read(&b, info, 13);
    // Its first RVA naming its second's table.
    let second = head[5..9].to_vec();
    caught(
        &a,
        &patched(&b, info + 1, &second),
        None,
        "FuncInfo4's first table",
    );
    // And its tables' RVAs, each naming the next byte.
    for k in [1usize, 5, 9] {
        let v = u32::from_le_bytes(head[k..k + 4].try_into().unwrap());
        if v == 0 {
            continue;
        }
        caught(
            &a,
            &patched(&b, info + k as u32, &(v + 1).to_le_bytes()),
            None,
            &format!("FuncInfo4 +{k}"),
        );
    }
}

#[test]
fn changed_rtti_is_caught() {
    let (a, b) = pair("link");
    // Derived's complete object locator names Base's type descriptor.
    let col = public(&b, b"??_R4Derived@@6B@");
    let base = public(&b, b"??_R0?AUBase@@@8");
    caught(
        &a,
        &patched(&b, col + 12, &base.to_le_bytes()),
        None,
        "Derived's type descriptor",
    );
}

#[test]
fn changed_data_the_pdb_gives_no_crc_is_caught() {
    let (a, b) = pair("link");
    let l = Linked::read(&b.0, &b.1, "the program").unwrap();
    let mut n = 0;
    for c in &l.contributions {
        if c.data_crc != 0
            || c.size < 4
            || c.characteristics & (scn::CNT_CODE | scn::CNT_UNINITIALIZED_DATA) != 0
            || l.in_tables(c.rva, c.size)
            || !l.sections.iter().any(|s| {
                (s.name == b".rdata" || s.name == b".data")
                    && (s.rva..s.rva + s.virtual_size).contains(&c.rva)
            })
        {
            continue;
        }
        // A byte that no base relocation covers.
        let relocs = l.relocations_in(c.rva, c.size);
        let Some(at) = (c.rva..c.rva + c.size)
            .find(|&r| relocs.iter().all(|&(x, s)| r < x || r >= x + u32::from(s)))
        else {
            continue;
        };
        let old = read(&b, at, 1)[0];
        caught(
            &a,
            &patched(&b, at, &[old ^ 0x10]),
            None,
            &format!("{} at {:#x}", show(&c.label), at),
        );
        n += 1;
        if n == 8 {
            break;
        }
    }
    assert!(n > 0, "contributions without CRCs");
}

#[test]
fn programs_of_a_broken_folded_library_are_caught() {
    let a = equiv("link-original");
    // classify's constant changed in the synthesized member.
    caught(
        &a,
        &equiv("link-broken"),
        Some("functions or data whose bytes differ"),
        "the broken library",
    );
    // shared_all's relocation naming twin_a for twin_b.
    caught(
        &a,
        &equiv("link-retarget"),
        Some("code whose operands name other functions or data"),
        "the retargeted relocation",
    );
}
