//! Finding images from their keys: on this machine (the hints, and
//! symbol directories laid out as a store), checked against the key.

use std::path::{Path, PathBuf};

use turbonop_image::{DebugId, Image};
use turbonop_program::locate::{ImageKey, find_images, find_local, local_candidates};

fn testdata(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(path)
}

fn pe_key(path: &Path) -> ImageKey {
    let data = std::fs::read(path).unwrap();
    let image = Image::parse(&data).unwrap();
    let pe = image.pe().unwrap();
    ImageKey::Pe {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        time_date_stamp: pe.file_header().time_date_stamp.get(),
        size_of_image: pe.optional_header().unwrap().size_of_image(),
    }
}

#[test]
fn pe_by_time_stamp_and_size() {
    let dll = testdata("init/initdll.dll");
    let key = pe_key(&dll);
    let found = find_local(&key, std::slice::from_ref(&dll)).unwrap();
    assert_eq!(found.path, dll);
    assert_eq!(found.slice, 0..found.data.len());
    assert_eq!(found.bytes(), &std::fs::read(&dll).unwrap()[..]);
    // Another image at the hinted path isn't it.
    let ImageKey::Pe {
        name,
        time_date_stamp,
        size_of_image,
    } = key
    else {
        unreachable!()
    };
    let other = ImageKey::Pe {
        name,
        time_date_stamp: time_date_stamp ^ 1,
        size_of_image,
    };
    let nf = find_local(&other, std::slice::from_ref(&dll)).unwrap_err();
    assert!(
        nf.tried.iter().any(|t| t.ends_with("(not this image)")),
        "{:?}",
        nf.tried
    );
    let size = ImageKey::Pe {
        name: "initdll.dll".into(),
        time_date_stamp,
        size_of_image: size_of_image + 0x1000,
    };
    assert!(find_local(&size, &[dll]).is_err());
}

#[test]
fn elf_by_build_id() {
    let so = testdata("init/libinit-x86_64-linux-gnu.so");
    let data = std::fs::read(&so).unwrap();
    let image = Image::parse(&data).unwrap();
    let id = image
        .debug_ids()
        .into_iter()
        .find_map(|d| match d {
            DebugId::BuildId(b) => Some(b.to_vec()),
            _ => None,
        })
        .unwrap();
    let key = ImageKey::BuildId {
        name: "/usr/lib/libinit.so".into(),
        build_id: id.clone(),
    };
    assert!(find_local(&key, std::slice::from_ref(&so)).is_ok());
    // Older Breakpad's first 16 bytes match too.
    let prefix = ImageKey::BuildId {
        name: "libinit.so".into(),
        build_id: id[..16].to_vec(),
    };
    assert!(find_local(&prefix, std::slice::from_ref(&so)).is_ok());
    let wrong = ImageKey::BuildId {
        name: "libinit.so".into(),
        build_id: vec![0; 20],
    };
    assert!(find_local(&wrong, &[so]).is_err());
}

#[test]
fn macho_slices_by_uuid() {
    let fat = testdata("macho/program-universal");
    let data = std::fs::read(&fat).unwrap();
    let archs = turbonop_macho::fat_archs(&data).unwrap();
    assert!(archs.len() >= 2);
    for a in &archs {
        let start = usize::try_from(a.offset).unwrap();
        let end = start + usize::try_from(a.size).unwrap();
        let uuid = turbonop_macho::MachO::parse(&data[start..end])
            .unwrap()
            .uuid()
            .unwrap();
        let key = ImageKey::Uuid {
            name: "program".into(),
            uuid,
        };
        let found = find_local(&key, std::slice::from_ref(&fat)).unwrap();
        assert_eq!(found.slice, start..end);
    }
    let thin = testdata("macho/program-arm64");
    let uuid = turbonop_macho::MachO::parse(&std::fs::read(&thin).unwrap())
        .unwrap()
        .uuid()
        .unwrap();
    let key = ImageKey::Uuid {
        name: "program-arm64".into(),
        uuid,
    };
    assert!(find_local(&key, &[thin]).is_ok());
}

#[test]
fn symbol_store_layout() {
    // A store: name/<index>/name, in a directory of its own.
    let dll = testdata("init/initdll.dll");
    let key = pe_key(&dll);
    let store = std::env::temp_dir().join(format!("turbonop-locate-{}", std::process::id()));
    let dir = store.join("initdll.dll").join(key.index());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&dll, dir.join("initdll.dll")).unwrap();
    let candidates = local_candidates(&key, &[], std::slice::from_ref(&store));
    assert_eq!(candidates[0], dir.join("initdll.dll"));
    assert!(candidates.contains(&store.join("initdll.dll")));
    // find_images, with the store as a hint's directory: found locally,
    // nothing asked of a server.
    let got = find_images(&[
        (key.clone(), vec![dir.join("initdll.dll")]),
        (
            ImageKey::Pe {
                name: "missing.dll".into(),
                time_date_stamp: 1,
                size_of_image: 2,
            },
            vec![store.join("nothing-here.dll")],
        ),
    ]);
    assert!(got[0].is_ok());
    assert!(got[1].is_err());
    let _ = std::fs::remove_dir_all(&store);
}
