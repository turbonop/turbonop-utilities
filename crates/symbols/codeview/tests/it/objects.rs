//! The corpus example's checks on objects built by `tools/codeview/build.cmd`
//! (kinds LLVM's /Z7 build lacks: optimized code's live ranges and inline
//! sites, frame cookies, jump tables, frame data, hot-patch functions,
//! managed code's tokens, a precompiled header and its user) and on
//! `testdata/coff` (an import library's C11 descriptor objects): every
//! field found and in range, every relocation on a field, everything
//! written back byte for byte, and renumbering undone exactly.

use crate::check::{Stats, check_object, pch_of};
use std::collections::HashMap;
use std::sync::Arc;
use turbonop_codeview::lines::subsection;
use turbonop_codeview::symbols::kind as sk;
use turbonop_codeview::types::leaf;

fn testdata(dir: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(dir)
}

/// The objects in `dir` (and the members of its libraries), checked.
fn checked(dirs: &[&str]) -> Stats {
    let mut objects: Vec<(String, Vec<u8>)> = Vec::new();
    for dir in dirs {
        let mut paths: Vec<_> = std::fs::read_dir(testdata(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        for p in paths {
            let data = std::fs::read(&p).unwrap();
            let name = p.display().to_string();
            match p.extension().and_then(|e| e.to_str()) {
                Some("obj") => objects.push((name, data)),
                Some("lib") => {
                    let a = turbonop_archive::Archive::parse(&data).unwrap();
                    for m in a.files() {
                        objects.push((
                            format!("{name}({})", String::from_utf8_lossy(&m.name)),
                            m.data.to_vec(),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    let mut pchs = HashMap::new();
    for (_, data) in &objects {
        if let Some((sig, kinds)) = pch_of(data) {
            pchs.insert(sig, Arc::new(kinds));
        }
    }
    let mut st = Stats::default();
    for (name, data) in &objects {
        let mut one = Stats::default();
        check_object(data, name, &pchs, &mut one);
        st.merge(one);
    }
    st
}

#[test]
fn fixtures_check_clean() {
    let st = checked(&["codeview", "coff", "coff/model"]);
    let failures: Vec<_> = st
        .failures
        .iter()
        .map(|(k, (n, ex))| format!("{n} {k}: {ex:?}"))
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(st.objects >= 11);
    // The kinds the fixtures exist for are still there.
    for k in [
        sk::S_LOCAL,
        sk::S_DEFRANGE_REGISTER,
        sk::S_DEFRANGE_FRAMEPOINTER_REL,
        sk::S_DEFRANGE_SUBFIELD_REGISTER,
        sk::S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE,
        sk::S_DEFRANGE_REGISTER_REL,
        sk::S_INLINESITE,
        sk::S_INLINESITE_END,
        sk::S_INLINEES,
        sk::S_CALLEES,
        sk::S_FRAMECOOKIE,
        sk::S_ARMSWITCHTABLE,
        sk::S_HEAPALLOCSITE,
        sk::S_CALLSITEINFO,
        sk::S_ANNOTATION,
        sk::S_THUNK32,
        sk::S_FILESTATIC,
        sk::S_GTHREAD32,
        sk::S_UNAMESPACE,
        sk::S_HOTPATCHFUNC,
        sk::S_GMANPROC,
        sk::S_MANSLOT,
        sk::S_GMANDATA,
        sk::S_LABEL32,
        sk::S_GPROC32,
        sk::S_BLOCK32,
        sk::S_OBJNAME_ST,
        sk::S_COMPILE2_ST,
    ] {
        assert!(st.symbol_kinds.contains_key(&k), "no {}", sk::name(k));
    }
    for k in [
        subsection::DEBUG_S_SYMBOLS,
        subsection::DEBUG_S_LINES,
        subsection::DEBUG_S_STRINGTABLE,
        subsection::DEBUG_S_FILECHKSMS,
        subsection::DEBUG_S_FRAMEDATA,
        subsection::DEBUG_S_INLINEELINES,
    ] {
        assert!(st.subsection_kinds.contains_key(&k), "no subsection {k:#x}");
    }
    for k in [leaf::LF_PRECOMP, leaf::LF_ENDPRECOMP] {
        assert!(st.type_kinds.contains_key(&k), "no {}", leaf::name(k));
    }
    assert_eq!(st.precomp_users, 1);
    for f in ["token", "rva", "annotation file", "string", "checksum"] {
        assert!(st.fields.contains_key(f), "no {f} fields");
    }
}
