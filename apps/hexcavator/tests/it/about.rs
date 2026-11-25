//! `hexcavator about` end to end: where a function is and what is known of
//! it (PGO's facts, folding, inlining, its class), and why a function the
//! image doesn't have isn't there.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn testdata(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(rel)
}

fn about(image: &Path, name: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(["about", "-json"])
        .arg(image)
        .arg(name)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn text(image: &Path, name: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .arg("about")
        .arg(image)
        .arg(name)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn pgo_facts() {
    let pgo = testdata("pgo/pgo.exe");
    // Run 2000 times in training: optimized, among the hottest code.
    let hot = about(&pgo, "hot(int)");
    assert_eq!(hot["found"], true);
    assert_eq!(hot["placement"]["section"], ".text");
    assert_eq!(hot["placement"]["group"], ".text$lp00pgo");
    assert_eq!(hot["pgo"]["optimized"], true);
    assert_eq!(hot["pgo"]["invocations"], 2000);
    assert!(hot["pgo"]["dynamic_instructions"].as_u64().unwrap() > 2000);
    assert!(text(&pgo, "hot(int)").contains("Optimized by PGO, for speed: called 2000 times"));
    // Run once: counted, but not optimized with the profile.
    let cold = about(&pgo, "cold(int)");
    assert_eq!(cold["pgo"]["optimized"], false);
    assert_eq!(cold["pgo"]["counts_valid"], true);
    // Never run: optimized, with no counts, among whole functions
    // training never ran. (By its name without parameters, too.)
    let never = about(&pgo, "never");
    assert_eq!(never["name"], "never(int)");
    assert_eq!(never["placement"]["group"], ".text$zz");
    assert_eq!(
        never["placement"]["group_description"],
        "whole functions PGO's training never ran"
    );
    assert_eq!(never["pgo"]["optimized"], true);
    assert!(never["pgo"]["invocations"].is_null());
    assert!(text(&pgo, "never(int)").contains("never run in training"));
}

#[test]
fn folding_both_ways() {
    let image = testdata("sizebench/bin/CppTestCases_BasicDiffObjectsAfter.dll");
    let folded = about(&image, "__scrt_stub_for_acrt_thread_attach()");
    assert_eq!(folded["file"], 0);
    let canonical = folded["folded_into"].as_str().unwrap().to_string();
    let it = about(&image, &canonical);
    assert!(it["file"].as_u64().unwrap() > 0);
    let names: Vec<&str> = it["folded"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        names.contains(&"__scrt_stub_for_acrt_thread_attach()"),
        "{names:?}"
    );
}

#[test]
fn inlined_into_and_inlined_everywhere() {
    let image = testdata("sizebench/bin/SizeBenchV2.AnalysisEngine.Tests.InlineSites.dll");
    // Inlined into DllMain, with no copy of its own.
    let gone = about(&image, "forceInlinedFunction");
    assert_eq!(gone["found"], false);
    assert_eq!(gone["inlined_copies"]["copies"], 1);
    let into = gone["inlined_copies"]["into"][0].as_str().unwrap();
    assert!(into.starts_with("DllMain("), "{into}");
    // And DllMain says so.
    let host = about(&image, into);
    let inlined: Vec<&str> = host["inlined"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["name"].as_str())
        .collect();
    assert!(inlined.contains(&"forceInlinedFunction"), "{inlined:?}");
    assert!(
        inlined.contains(&"anotherForceInlinedFunction"),
        "{inlined:?}"
    );
}

#[test]
fn a_class_s_view_and_what_it_lacks() {
    let image = testdata("sizebench/bin/SizeBenchV2.AnalysisEngine.Tests.CppTestCasesBefore.dll");
    let what = about(&image, "std::exception::what() const");
    assert_eq!(what["member_of"], "std::exception");
    assert_eq!(what["declared"][0], "virtual, introduced here");
    assert!(
        what["placement"]["compiland"]
            .as_str()
            .unwrap()
            .ends_with("throw_bad_alloc.obj")
    );
    assert!(what["source"]["line"].as_u64().unwrap() > 0);
    // Declared, but with no code in the image.
    let assign = about(&image, "std::exception::operator=");
    assert_eq!(assign["found"], false);
    assert_eq!(assign["member_of"], "std::exception");
    assert!(assign["reason"].as_str().unwrap().starts_with("declared"));
    // Neither: names like it, offered.
    let typo = about(&image, "std::exception::wha");
    assert_eq!(typo["found"], false);
    assert!(
        typo["similar"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "std::exception::what() const"),
        "{typo}"
    );
}
