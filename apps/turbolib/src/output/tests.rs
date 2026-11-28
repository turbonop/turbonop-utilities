use super::*;
use turbonop_archive::write::{Index, MemberHeader, NewMember, SizeLimit, Style};

/// The file written is the bytes the in-memory writer makes, for members
/// small, odd, and larger than a piece.
#[test]
fn the_same_bytes_as_the_in_memory_writer() {
    let dir = std::env::temp_dir().join(format!("turbolib-output-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let big: Vec<u8> = (0..(CHUNK * 2 + 12_345))
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let datas: Vec<Vec<u8>> = vec![
        b"odd".to_vec(),
        vec![1; CHUNK + 1],
        big,
        Vec::new(),
        b"even".to_vec(),
    ];
    let names: Vec<Vec<u8>> = (0..datas.len())
        .map(|i| format!("member-with-a-long-name-{i}.obj").into_bytes())
        .collect();
    let style = Style::LibExeBrepro;
    let fields = style.object_fields(0, false);
    let new: Vec<NewMember<'_>> = names
        .iter()
        .zip(&datas)
        .map(|(n, d)| NewMember {
            name: n,
            fields,
            data: d,
        })
        .collect();
    let index = Index::default();
    let expected = turbonop_archive::write::write(&new, &index, style, SizeLimit::FourGiB).unwrap();
    let headers: Vec<MemberHeader<'_>> = new
        .iter()
        .map(|m| MemberHeader {
            name: m.name,
            fields: m.fields,
            size: m.data.len() as u64,
        })
        .collect();
    let layout = Layout::new(&headers, &index, style, SizeLimit::FourGiB).unwrap();
    // Each member as bytes with holes: every third byte range of 1000 cut
    // out and given back as a piece.
    type Holes<'a> = Vec<(usize, &'a [u8])>;
    let skeletons: Vec<(Vec<u8>, Holes<'_>)> = datas
        .iter()
        .map(|d| {
            let mut bytes = Vec::new();
            let mut holes = Vec::new();
            let mut at = 7;
            let mut kept = 0;
            while at + 1000 <= d.len() {
                bytes.extend_from_slice(&d[kept..at]);
                holes.push((at, &d[at..at + 1000]));
                kept = at + 1000;
                at += 3000;
            }
            bytes.extend_from_slice(&d[kept..]);
            (bytes, holes)
        })
        .collect();
    let members: Vec<Bytes<'_>> = skeletons
        .iter()
        .map(|(b, h)| Bytes { data: b, holes: h })
        .collect();
    let out = dir.join("x.lib");
    let temp = write_beside(&out, &layout, &members, style.conventions().padding).unwrap();
    let at = temp.path().to_path_buf();
    temp.replace(&out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), expected);
    assert!(!at.exists());
    // Dropped without replacing (an error, a panic): removed.
    let temp = write_beside(&out, &layout, &members, style.conventions().padding).unwrap();
    let at = temp.path().to_path_buf();
    assert!(at.exists());
    drop(temp);
    assert!(!at.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rename that fails transiently is tried again until it works, or
/// until the limit; one that fails otherwise, or isn't worth retrying,
/// isn't.
#[test]
fn retries() {
    use std::cell::Cell;
    use std::time::Duration;
    let tries = Cell::new(0);
    let sharing = || io::Error::from_raw_os_error(32);
    let r = retry(
        Duration::from_secs(5),
        || {
            tries.set(tries.get() + 1);
            if tries.get() < 3 {
                Err(sharing())
            } else {
                Ok(())
            }
        },
        || true,
    );
    if cfg!(windows) {
        assert!(r.is_ok());
        assert_eq!(tries.get(), 3);
    } else {
        assert!(r.is_err());
        assert_eq!(tries.get(), 1);
    }
    tries.set(0);
    let r = retry(
        Duration::from_secs(5),
        || {
            tries.set(tries.get() + 1);
            Err(sharing())
        },
        || false,
    );
    assert!(r.is_err());
    assert_eq!(tries.get(), 1);
    tries.set(0);
    let start = std::time::Instant::now();
    let r = retry(
        Duration::from_millis(100),
        || {
            tries.set(tries.get() + 1);
            Err(sharing())
        },
        || true,
    );
    assert!(r.is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    tries.set(0);
    let r = retry(
        Duration::from_secs(5),
        || {
            tries.set(tries.get() + 1);
            Err(io::Error::from(io::ErrorKind::NotFound))
        },
        || true,
    );
    assert!(r.is_err());
    assert_eq!(tries.get(), 1);
}
