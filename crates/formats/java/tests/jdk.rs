//! The machine's JDK, when there is one: every class in its JMODs (or, for
//! Java 8, its `rt.jar`) read with no errors and its code decoded; and,
//! when it's JDK 21 (whose javap the renderer follows), a sample of its
//! classes against its javap, live.

use turbonop_global_alloc as _;

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{first_difference, java_home, javap_body, jdk_version};
use turbonop_java::{AttributesExt, ClassFile, bytecode};

/// The JDK's archives of classes: its JMODs, else its JARs.
fn class_archives(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [home.join("jmods"), home.join("jre/lib"), home.join("lib")] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "jmod" || x == "jar") {
                    out.push(p);
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out.sort();
    out
}

/// Reads `data` (a class), walks everything, and returns its errors.
fn check_class(data: &[u8]) -> Vec<String> {
    let cf = match ClassFile::parse(data) {
        Ok(cf) => cf,
        Err(e) => return vec![format!("class: {e}")],
    };
    let mut errors: Vec<String> = cf
        .malformed_attributes()
        .iter()
        .map(|e| format!("attribute: {e}"))
        .collect();
    for m in &cf.methods {
        if let Some(c) = m.attributes.code() {
            for i in bytecode::decode(c.code) {
                if let Err(e) = i {
                    errors.push(format!("code: {e}"));
                }
            }
        }
    }
    let _ = turbonop_java::javap::render(&cf, &Default::default());
    errors
}

#[test]
fn every_jdk_class_reads() {
    let Some(home) = java_home() else {
        eprintln!("no JDK: skipped");
        return;
    };
    let archives = class_archives(&home);
    let mut classes = 0;
    let mut failures = Vec::new();
    for p in &archives {
        let data = std::fs::read(p).unwrap();
        let Ok(a) = turbonop_zip::Archive::parse(&data) else {
            continue;
        };
        let results = a.par_map(|e| {
            let name = e.name();
            if !name.ends_with(".class") {
                return None;
            }
            let errors = match a.read(e) {
                Ok(d) => check_class(&d),
                Err(err) => vec![format!("entry: {err}")],
            };
            Some((name.into_owned(), errors))
        });
        for (name, errors) in results.into_iter().flatten() {
            classes += 1;
            if !errors.is_empty() {
                failures.push(format!("{}: {name}: {errors:?}", p.display()));
            }
        }
    }
    eprintln!("{} archives, {classes} classes", archives.len());
    assert!(
        failures.is_empty(),
        "{}",
        failures[..failures.len().min(20)].join("\n")
    );
}

/// Classes of every kind javap might write differently: generics,
/// annotations, modules, records, sealed hierarchies, enums, nests,
/// lambdas, switches.
const SAMPLE: &[&str] = &[
    "java.lang.Object",
    "java.lang.String",
    "java.lang.Integer",
    "java.lang.Math",
    "java.lang.Thread$State",
    "java.lang.Record",
    "java.lang.annotation.Retention",
    "java.lang.invoke.MethodHandles",
    "java.lang.runtime.ObjectMethods",
    "java.lang.constant.ConstantDescs",
    "java.util.HashMap",
    "java.util.HashMap$TreeNode",
    "java.util.ArrayList",
    "java.util.concurrent.ConcurrentHashMap",
    "java.util.stream.Collectors",
    "java.util.regex.Pattern",
    "java.time.LocalDate",
    "java.io.ObjectInputStream",
    "java.net.URI",
    "java.math.BigDecimal",
    "jdk.internal.misc.Unsafe",
    "java.lang.Character$UnicodeBlock",
];

#[test]
fn jdk_classes_match_its_javap() {
    let Some(home) = java_home() else {
        eprintln!("no JDK: skipped");
        return;
    };
    if jdk_version(&home) != Some(21) {
        eprintln!("not JDK 21 (the javap the renderer follows): skipped");
        return;
    }
    let exe = if cfg!(windows) { "javap.exe" } else { "javap" };
    let javap = home.join("bin").join(exe);
    let jmod = home.join("jmods/java.base.jmod");
    let (Ok(data), true) = (std::fs::read(&jmod), javap.is_file()) else {
        eprintln!("no javap or java.base.jmod: skipped");
        return;
    };
    let out = Command::new(&javap)
        .args([
            "-J-Dstdout.encoding=UTF-8",
            "-J-Duser.language=en",
            "-v",
            "-p",
        ])
        .args(SAMPLE)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap().replace("\r\n", "\n");
    // One class's output per `Classfile` line.
    let mut outputs: Vec<String> = Vec::new();
    for l in text.lines() {
        if l.starts_with("Classfile ") {
            outputs.push(String::new());
        }
        if let Some(o) = outputs.last_mut() {
            o.push_str(l);
            o.push('\n');
        }
    }
    assert_eq!(outputs.len(), SAMPLE.len());
    let a = turbonop_zip::Archive::parse(&data).unwrap();
    let mut failures = Vec::new();
    for (name, theirs) in SAMPLE.iter().zip(&outputs) {
        let path = format!("classes/{}.class", name.replace('.', "/"));
        let bytes = a.read_by_name(&path).unwrap().unwrap();
        let cf = ClassFile::parse(&bytes).unwrap();
        let ours = turbonop_java::javap::render(&cf, &Default::default());
        if let Some(d) = first_difference(&ours, &javap_body(theirs)) {
            failures.push(format!("{name}: {d}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
