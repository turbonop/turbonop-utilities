use super::*;

/// Roots that keep every record: each source's every index.
fn keep_all(maps: &[IndexMap]) -> (Vec<u32>, Vec<u32>) {
    let mut t = Vec::new();
    let mut i = Vec::new();
    for m in maps {
        if m.is_id.is_empty() {
            t.extend(&m.types);
            i.extend(&m.ids);
        } else {
            for (v, &id) in m.types.iter().zip(&m.is_id) {
                if id { i.push(*v) } else { t.push(*v) }
            }
        }
    }
    (t, i)
}

/// A whole record: length, kind, body (padded to 4 as compilers do).
fn rec(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let n = (4 + body.len()).next_multiple_of(4);
    v.extend_from_slice(&u16::try_from(n - 2).unwrap().to_le_bytes());
    v.extend_from_slice(&kind.to_le_bytes());
    v.extend_from_slice(body);
    let pad = n - v.len();
    for q in 0..pad {
        v.push(0xF0 + u8::try_from(pad - q).unwrap());
    }
    v
}

fn ptr(to: u32) -> Vec<u8> {
    // LF_POINTER: referent, attributes (64-bit near pointer).
    let mut b = to.to_le_bytes().to_vec();
    b.extend_from_slice(&0x0001_000Cu32.to_le_bytes());
    rec(0x1002, &b)
}

fn arglist(args: &[u32]) -> Vec<u8> {
    let mut b = u32::try_from(args.len()).unwrap().to_le_bytes().to_vec();
    for a in args {
        b.extend_from_slice(&a.to_le_bytes());
    }
    rec(0x1201, &b)
}

fn procedure(ret: u32, args: u32) -> Vec<u8> {
    let mut b = ret.to_le_bytes().to_vec();
    b.extend_from_slice(&[0, 0]); // call convention, attributes
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&args.to_le_bytes());
    rec(0x1008, &b)
}

fn func_id(ty: u32, name: &str) -> Vec<u8> {
    let mut b = 0u32.to_le_bytes().to_vec(); // scope
    b.extend_from_slice(&ty.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.push(0);
    rec(0x1601, &b)
}

#[test]
fn same_records_merge_and_indices_renumber() {
    // Object A: int*, (int*), int f(int*), f's id.
    let a = [
        ptr(0x74),
        arglist(&[0x1000]),
        procedure(0x74, 0x1001),
        func_id(0x1002, "f"),
    ];
    // Object B: char* first (new), then the same as A, and g's id.
    let b = [
        ptr(0x70),
        ptr(0x74),
        arglist(&[0x1001]),
        procedure(0x74, 0x1002),
        func_id(0x1003, "g"),
        func_id(0x1003, "f"),
    ];
    let sources = [
        Source::Object(a.iter().map(Vec::as_slice).collect()),
        Source::Object(b.iter().map(Vec::as_slice).collect()),
    ];
    let m = merge(&sources, keep_all);
    assert!(m.maps.iter().all(|x| x.error.is_none()));
    // Types: A's three, then B's char*; ids: f, g.
    assert_eq!(m.types.offsets.len(), 4);
    assert_eq!(m.ids.offsets.len(), 2);
    assert_eq!(m.maps[0].types, [0x1000, 0x1001, 0x1002, 0x1000]);
    assert_eq!(
        m.maps[1].types,
        [0x1003, 0x1000, 0x1001, 0x1002, 0x1001, 0x1000]
    );
    // The procedure's argument list index, renumbered: B's (0x1002) is
    // A's (0x1001).
    let p = m.types.offsets[2] as usize;
    let args = u32::from_le_bytes(m.types.records[p + 12..p + 16].try_into().unwrap());
    assert_eq!(args, 0x1001);
    // g's id refers to the procedure type 0x1002.
    let g = m.ids.offsets[1] as usize;
    let ty = u32::from_le_bytes(m.ids.records[g + 8..g + 12].try_into().unwrap());
    assert_eq!(ty, 0x1002);
    assert_eq!(&m.ids.records[g + 12..g + 13], b"g");
}

#[test]
fn a_forward_reference_is_an_error() {
    let a = [arglist(&[0x1001]), ptr(0x74)];
    let m = merge(
        &[Source::Object(a.iter().map(Vec::as_slice).collect())],
        keep_all,
    );
    assert!(m.maps[0].error.is_some());
    assert_eq!(m.maps[0].map(0x1000, false), 0);
    assert!(m.types.offsets.is_empty());
}

#[test]
fn pdb_sources_keep_their_spaces_apart() {
    let types = [ptr(0x74), arglist(&[0x1000]), procedure(0x74, 0x1001)];
    let ids = [func_id(0x1002, "f")];
    let obj = [ptr(0x74)];
    let sources = [
        Source::Object(obj.iter().map(Vec::as_slice).collect()),
        Source::Pdb {
            types: types.iter().map(Vec::as_slice).collect(),
            ids: ids.iter().map(Vec::as_slice).collect(),
            names: None,
        },
    ];
    let m = merge(&sources, keep_all);
    assert_eq!(m.maps[1].types, [0x1000, 0x1001, 0x1002]);
    assert_eq!(m.maps[1].ids, [0x1000]);
    assert_eq!(m.maps[1].map(0x1000, true), 0x1000);
    assert_eq!(m.types.offsets.len(), 3);
}
