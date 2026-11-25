//! `DebugSymbolizer` on images without PDBs (testdata/mingw, from
//! tools/mingw): a PE image with its own DWARF, one stripped with its
//! DWARF in the file its debug link names, and a DLL by its own symbols;
//! and an image file that isn't the one the trace saw.

use std::path::{Path, PathBuf};

use turbonop_symbolize::SymbolPath;
use turbonop_trace::{Clock, Image, ImageIx, ProcessIx, Trace};
use turbonop_trace_analysis::{DebugSymbolizer, Symbolizer};

fn mingw(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/mingw")
        .join(name)
}

/// A trace with each file loaded (of `size` in memory, with
/// `time_date_stamp`).
fn trace(images: &[(&str, u64, u32)]) -> Trace {
    let mut t = Trace::new(
        Clock {
            ticks_per_second: 1000,
            origin: 0,
        },
        1,
    );
    for (i, &(name, size, time_date_stamp)) in images.iter().enumerate() {
        t.images.push(Image {
            process: ProcessIx::NONE,
            base: 0x1_0000_0000 * (i as u64 + 1),
            size,
            path: mingw(name).to_string_lossy().into_owned(),
            time_date_stamp,
            checksum: 0,
            debug_id: None,
            load: None,
            unload: None,
        });
    }
    t
}

#[test]
fn images_without_pdbs() {
    let t = trace(&[
        ("program.exe", 0xA000, 0),
        ("program-stripped.exe", 0, 0),
        ("library.dll", 0, 0),
        // (Not the image the trace saw: another timestamp.)
        ("program.exe", 0xA000, 0x1234_5678),
        ("no-such-image.exe", 0, 0),
    ]);
    let s = DebugSymbolizer::new(SymbolPath::default());
    let function = |image: u32, offset: u64| s.function(&t, ImageIx(image), offset);
    // Its own DWARF: functions, and their lines.
    for image in [0, 1] {
        assert_eq!(function(image, 0x1000), Some((0x1000, "used".into())));
        assert_eq!(function(image, 0x1012), Some((0x1010, "unused".into())));
        assert_eq!(function(image, 0x1020), Some((0x1020, "start".into())));
        let (file, line) = s.line(&t, ImageIx(image), 0x1000).unwrap();
        assert!(file.ends_with("program.c"), "{file}");
        assert!(line > 0);
    }
    // Its exports and COFF symbols.
    assert_eq!(function(2, 0x1004), Some((0x1000, "add".into())));
    assert_eq!(function(2, 0x1010), Some((0x1010, "triple".into())));
    assert_eq!(
        function(2, 0x1020),
        Some((0x1020, "DllMainCRTStartup".into()))
    );
    assert_eq!(s.line(&t, ImageIx(2), 0x1000), None);
    // Neither.
    assert_eq!(function(3, 0x1000), None);
    assert_eq!(function(4, 0x1000), None);
    assert_eq!(s.loaded(), (5, 3));
}
