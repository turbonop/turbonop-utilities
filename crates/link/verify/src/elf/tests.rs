use super::*;

/// A symbol of a test image: name, value, size, binding and type, section.
struct Sym {
    name: &'static [u8],
    value: u64,
    size: u64,
    info: u8,
    shndx: u16,
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// A section header: name offset, type, flags, address, offset, size,
/// link, info, entry size.
type Header = (u32, u32, u64, u64, u64, u64, u32, u32, u64);

/// A little x86-64 executable: `.text` at 0x1000 and `.data` at 0x2000
/// (one loadable segment each), with `syms` in `.symtab`, entered at
/// `entry`.
fn image(text: &[u8], data: &[u8], syms: &[Sym], entry: u64) -> Vec<u8> {
    let mut out = vec![0u8; 0x1000];
    out.extend_from_slice(text);
    out.resize(0x2000, 0);
    out.extend_from_slice(data);
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 24];
    for s in syms {
        let at = strtab.len() as u32;
        strtab.extend_from_slice(s.name);
        strtab.push(0);
        put32(&mut symtab, at);
        symtab.push(s.info);
        symtab.push(0);
        put16(&mut symtab, s.shndx);
        put64(&mut symtab, s.value);
        put64(&mut symtab, s.size);
    }
    let shstrtab = b"\0.text\0.data\0.symtab\0.strtab\0.shstrtab\0".to_vec();
    let symtab_off = out.len() as u64;
    out.extend_from_slice(&symtab);
    let strtab_off = out.len() as u64;
    out.extend_from_slice(&strtab);
    let shstr_off = out.len() as u64;
    out.extend_from_slice(&shstrtab);
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    let shoff = out.len() as u64;
    let headers: [Header; 6] = [
        (0, 0, 0, 0, 0, 0, 0, 0, 0),
        (1, 1, 6, 0x1000, 0x1000, text.len() as u64, 0, 0, 0),
        (7, 1, 3, 0x2000, 0x2000, data.len() as u64, 0, 0, 0),
        (13, 2, 0, 0, symtab_off, symtab.len() as u64, 4, 1, 24),
        (21, 3, 0, 0, strtab_off, strtab.len() as u64, 0, 0, 0),
        (29, 3, 0, 0, shstr_off, shstrtab.len() as u64, 0, 0, 0),
    ];
    for (name, kind, flags, addr, off, size, link, info, entsize) in headers {
        put32(&mut out, name);
        put32(&mut out, kind);
        put64(&mut out, flags);
        put64(&mut out, addr);
        put64(&mut out, off);
        put64(&mut out, size);
        put32(&mut out, link);
        put32(&mut out, info);
        put64(&mut out, 8);
        put64(&mut out, entsize);
    }
    // The file header and two program headers.
    let mut h = Vec::new();
    h.extend_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put16(&mut h, 2);
    put16(&mut h, 62);
    put32(&mut h, 1);
    put64(&mut h, entry);
    put64(&mut h, 64);
    put64(&mut h, shoff);
    put32(&mut h, 0);
    put16(&mut h, 64);
    put16(&mut h, 56);
    put16(&mut h, 2);
    put16(&mut h, 64);
    put16(&mut h, 6);
    put16(&mut h, 5);
    for (off, size, flags) in [
        (0x1000u64, text.len() as u64, 5u32),
        (0x2000, data.len() as u64, 6),
    ] {
        put32(&mut h, 1);
        put32(&mut h, flags);
        put64(&mut h, off);
        put64(&mut h, off);
        put64(&mut h, off);
        put64(&mut h, size);
        put64(&mut h, size);
        put64(&mut h, 0x1000);
    }
    out[..h.len()].copy_from_slice(&h);
    out
}

const FUNC: u8 = (1 << 4) | 2;
const OBJECT: u8 = (1 << 4) | 1;

/// `f` calls `g`; `d` holds `g`'s address. With `swapped`, `g` comes
/// first, so every address differs. `ret` is `g`'s one byte.
fn program(swapped: bool, ret: u8) -> Vec<u8> {
    let (f, g) = if swapped {
        (0x1001u64, 0x1000u64)
    } else {
        (0x1000, 0x1006)
    };
    let rel = (g as i64 - (f as i64 + 5)) as i32;
    let mut fbytes = vec![0xE8];
    fbytes.extend_from_slice(&rel.to_le_bytes());
    fbytes.push(0xC3);
    let text = if swapped {
        let mut t = vec![ret];
        t.extend_from_slice(&fbytes);
        t
    } else {
        let mut t = fbytes;
        t.push(ret);
        t
    };
    let data = g.to_le_bytes();
    image(
        &text,
        &data,
        &[
            Sym {
                name: b"f",
                value: f,
                size: 6,
                info: FUNC,
                shndx: 1,
            },
            Sym {
                name: b"g",
                value: g,
                size: 1,
                info: FUNC,
                shndx: 1,
            },
            Sym {
                name: b"d",
                value: 0x2000,
                size: 8,
                info: OBJECT,
                shndx: 2,
            },
        ],
        f,
    )
}

#[test]
fn a_program_matches_itself() {
    let a = program(false, 0xC3);
    let r = compare(&a, &a).unwrap();
    assert!(r.passed(), "{}", r.text());
    assert_eq!(r.checked_count("functions and data"), 3);
    assert_eq!(r.checked_count("PC-relative operands matched"), 1);
}

#[test]
fn functions_moved_match() {
    let r = compare(&program(false, 0xC3), &program(true, 0xC3)).unwrap();
    assert!(r.passed(), "{}", r.text());
    assert_eq!(r.checked_count("PC-relative operands matched"), 1);
    assert_eq!(r.checked_count("other words read as addresses, matched"), 1);
}

#[test]
fn a_changed_byte_is_caught() {
    let r = compare(&program(false, 0xC3), &program(true, 0x90)).unwrap();
    assert!(
        r.failed("functions or data whose bytes differ"),
        "{}",
        r.text()
    );
}

#[test]
fn a_missing_function_is_caught() {
    let a = program(false, 0xC3);
    let mut b = program(false, 0xC3);
    // Rename `g` (its name in .strtab) to `h`.
    let at = b.windows(3).position(|w| w == b"\0g\0").unwrap();
    b[at + 1] = b'h';
    let r = compare(&a, &b).unwrap();
    assert!(r.failed("functions or data only in the first program"));
    assert!(r.failed("symbols only in the second program"));
}
