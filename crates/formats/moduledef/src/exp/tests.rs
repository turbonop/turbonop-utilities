extern crate std;

use std::vec::Vec;

use super::*;
use crate::exports::{Origin, resolve};
use crate::parse;

fn resolved(text: &[u8]) -> Vec<Resolved<'_>> {
    let def = parse(text).unwrap();
    let given: Vec<_> = def
        .exports()
        .into_iter()
        .map(|e| (e, Origin::Definition))
        .collect();
    resolve(&given, machine::AMD64, None).unwrap()
}

/// The ordinals lib.exe gave (measured), in the exports' order.
#[test]
fn ordinals_as_measured() {
    for (text, base, want) in [
        (&b"EXPORTS\n A @10\n B\n C\n"[..], 10, &[10u16, 11, 12][..]),
        (b"EXPORTS\n B\n A @2\n C\n D @5\n", 2, &[3, 2, 4, 5]),
        (b"EXPORTS\n A @65535\n B\n", 65534, &[65535, 65534]),
        (
            b"EXPORTS\n A @65534\n B\n C\n",
            65533,
            &[65534, 65533, 65535],
        ),
        (b"EXPORTS\n A @65535\n B @1\n C\n", 1, &[65535, 1, 2]),
        // By symbol, not by the exported name: y (as a) after x.
        (
            b"EXPORTS\n B=Z\n C\n y EXPORTAS a\n x\n m=Q @1\n",
            1,
            &[2, 3, 5, 4, 1],
        ),
    ] {
        let r = resolved(text);
        let got = ordinals(&r, &symbol_order(&r), None);
        assert_eq!(got, (base, want.to_vec()));
    }
}

#[test]
fn directives_as_measured() {
    let d = |t: &[u8]| {
        let def = parse(t).unwrap();
        std::string::String::from_utf8(directives(&def, b"f")).unwrap()
    };
    assert_eq!(
        d(b"NAME prog BASE=0x10000\nHEAPSIZE 0x1000,0x100\nSTACKSIZE 4096\nVERSION 3.7\nSECTIONS\n  .mydata READ WRITE SHARED\n  .x EXECUTE\n"),
        " /OUT:prog.exe /BASE:0x10000 /HEAP:4096,256 /STACK:4096 /VERSION:3.7 /SECTION:.mydata,RSW /SECTION:.x,E"
    );
    assert_eq!(
        d(b"LIBRARY \"a b\" BASE=16\n"),
        " /DLL /OUT:\"a b.dll\" /BASE:0x10"
    );
    assert_eq!(d(b"LIBRARY \"unclosed\n"), " /DLL /OUT:unclosed.dll");
    assert_eq!(
        d(b"LIBRARY a BASE=-1\nSTACKSIZE ,256\n"),
        " /DLL /OUT:a.dll /BASE:0xffffffffffffffff /STACK:,256"
    );
    assert_eq!(
        d(b"NAME a WINDOWAPI WINDOWCOMPAT\n"),
        " /OUT:a.exe /SUBSYSTEM:WINDOWS /SUBSYSTEM:CONSOLE"
    );
    assert_eq!(d(b"LIBRARY\nVERSION 0\n"), " /VERSION:0.0");
    assert_eq!(
        d(b"SECTIONS\n .a READ WRITE READONLY\n .b SHARED NONSHARED\n .c EXECUTEREAD EXECUTEONLY\n .h DISCARDABLE\n .n NONSHARED\n"),
        " /SECTION:.a,RW /SECTION:.b,S /SECTION:.c,ER /SECTION:.h,D"
    );
}
