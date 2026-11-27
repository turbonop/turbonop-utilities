use super::*;
use crate::args::split_words;

fn config(line: &str) -> (Config, Vec<Diagnostic>) {
    let args: Vec<Arg> = split_words(line).iter().map(|w| Arg::parse(w)).collect();
    let mut d = Vec::new();
    let c = parse(&args, &mut d);
    (c, d)
}

fn clean(line: &str) -> Config {
    let (c, d) = config(line);
    assert!(d.is_empty(), "{line}: {d:?}");
    c
}

#[test]
fn numbers_read_as_strtoul_does() {
    assert_eq!(parse_number("4096"), Some(4096));
    assert_eq!(parse_number("0x1000"), Some(4096));
    assert_eq!(parse_number("0X10"), Some(16));
    assert_eq!(parse_number("010"), Some(8));
    assert_eq!(parse_number("0"), Some(0));
    assert_eq!(parse_number("09"), None);
    assert_eq!(parse_number("x"), None);
}

#[test]
fn defaults() {
    let c = clean("a.obj");
    assert_eq!(c.inputs, ["a.obj"]);
    assert!(c.dynamic_base && c.nx_compat && c.allow_bind && c.allow_isolation);
    assert_eq!(c.stack, (0x10_0000, None));
    assert_eq!(c.debug, Debug::None);
    // Without /DEBUG: REF and ICF; with it: neither, unless asked.
    assert!(c.gc_sections() && c.icf());
    let c = clean("/DEBUG");
    assert!(!c.gc_sections() && !c.icf());
    let c = clean("/DEBUG /OPT:REF");
    assert!(c.gc_sections() && c.icf());
    let c = clean("/OPT:REF,NOICF");
    assert!(c.gc_sections() && !c.icf());
    let c = clean("/opt:icf=3");
    assert_eq!((c.opt_icf, c.icf_iterations), (Some(true), Some(3)));
}

#[test]
fn common_options() {
    let c = clean(
        r#"/NOLOGO /OUT:"out dir\a.exe" /MACHINE:X64 /SUBSYSTEM:CONSOLE,6.02 /ENTRY:mainCRTStartup
           /LIBPATH:C:\lib /DEFAULTLIB:kernel32.lib /NODEFAULTLIB:LIBCMT /INCLUDE:sym
           /ALTERNATENAME:a=b /MERGE:.rdata=.text /STACK:0x200000,0x1000 /HEAP:65536
           /BASE:0x140000000 /DYNAMICBASE:NO /HIGHENTROPYVA:NO /LARGEADDRESSAWARE /VERSION:2.5
           /DEBUG:FULL /PDB:a.pdb /PDBALTPATH:%_PDB% /MAP /GUARD:CF /CETCOMPAT /Brepro
           /DELAYLOAD:user32.dll /DELAY:UNLOAD /WX /IGNORE:4099,4217 /INCREMENTAL:NO"#,
    );
    assert_eq!(c.out, Some(PathBuf::from(r"out dir\a.exe")));
    assert_eq!(c.machine, Some(Machine::X64));
    assert_eq!(c.subsystem, Some(Subsystem::Console));
    assert_eq!(c.subsystem_version, Some((6, 2)));
    assert_eq!(c.entry.as_deref(), Some("mainCRTStartup"));
    assert_eq!(c.lib_paths, [PathBuf::from(r"C:\lib")]);
    assert_eq!(c.default_libs, ["kernel32.lib"]);
    assert_eq!(c.no_default_lib, NoDefaultLib::These(vec!["libcmt".into()]));
    assert_eq!(c.includes, ["sym"]);
    assert_eq!(c.alternate_names, [("a".into(), "b".into())]);
    assert_eq!(c.merges, [(".rdata".into(), ".text".into())]);
    assert_eq!(c.stack, (0x20_0000, Some(0x1000)));
    assert_eq!(c.heap, (65536, None));
    assert_eq!(c.base, Some(0x1_4000_0000));
    assert!(!c.dynamic_base);
    assert_eq!(c.high_entropy_va, Some(false));
    assert_eq!(c.large_address_aware, Some(true));
    assert_eq!(c.version, (2, 5));
    assert_eq!(c.debug, Debug::Full);
    assert_eq!(c.pdb, Some(PathBuf::from("a.pdb")));
    assert_eq!(c.pdb_alt_path.as_deref(), Some("%_PDB%"));
    assert_eq!(c.map, Some(None));
    assert!(c.guard_cf && c.cet_compat && c.repro && c.delay_unload);
    assert_eq!(c.delay_load, ["user32.dll"]);
    assert_eq!(c.warnings_as_errors, Some(vec![]));
    assert_eq!(c.ignore_warnings, [4099, 4217]);
    assert_eq!(c.incremental, Some(false));
    // /NODEFAULTLIB alone wins over named ones.
    let c = clean("/NODEFAULTLIB:a /NODEFAULTLIB /NODEFAULTLIB:b");
    assert_eq!(c.no_default_lib, NoDefaultLib::All);
}

#[test]
fn exports() {
    let e = Export::parse("f").unwrap();
    assert_eq!(
        (e.name.as_str(), e.internal, e.ordinal, e.data),
        ("f", None, None, false)
    );
    let e = Export::parse("g=impl_g,@7,NONAME,DATA,PRIVATE").unwrap();
    assert_eq!(e.name, "g");
    assert_eq!(e.internal.as_deref(), Some("impl_g"));
    assert_eq!(e.ordinal, Some(7));
    assert!(e.noname && e.data && e.private);
    let e = Export::parse("h,EXPORTAS,h2").unwrap();
    assert_eq!(e.export_as.as_deref(), Some("h2"));
    assert!(Export::parse("").is_err());
    assert!(Export::parse("f,@0").is_err());
    assert!(Export::parse("f,@70000").is_err());
    assert!(Export::parse("f,BOGUS").is_err());
    assert!(Export::parse("f,EXPORTAS").is_err());
}

#[test]
fn section_attributes() {
    let s = SectionAttributes::parse(".shared,RWS").unwrap();
    assert_eq!(s.name, ".shared");
    assert_eq!(s.set, 0x4000_0000 | 0x8000_0000 | 0x1000_0000);
    assert_eq!(s.clear, 0);
    let s = SectionAttributes::parse(".x,!W,ALIGN=64").unwrap();
    assert_eq!((s.set, s.clear, s.align), (0, 0x8000_0000, Some(64)));
    assert!(SectionAttributes::parse(".x,Q").is_err());
    assert!(SectionAttributes::parse(".x,ALIGN=3").is_err());
    assert!(SectionAttributes::parse("").is_err());
}

#[test]
fn manifests() {
    assert_eq!(clean("/MANIFEST").manifest, Manifest::SideBySide);
    assert_eq!(
        clean("/MANIFEST:EMBED").manifest,
        Manifest::Embed { id: None }
    );
    assert_eq!(
        clean("/manifest:embed,id=2").manifest,
        Manifest::Embed { id: Some(2) }
    );
    assert_eq!(clean("/MANIFESTUAC:NO").manifest_uac, Some(None));
    let c = clean(r#""/MANIFESTUAC:level='asInvoker' uiAccess='false'""#);
    assert_eq!(
        c.manifest_uac,
        Some(Some("level='asInvoker' uiAccess='false'".into()))
    );
}

#[test]
fn diagnostics() {
    let (_, d) = config("/BOGUS /CLRIMAGETYPE:IJW /LTCG /WINMD /OUT /MACHINE:Z80");
    assert_eq!(
        d,
        [
            Diagnostic::Unknown("/BOGUS".into()),
            Diagnostic::Ignored("/CLRIMAGETYPE:IJW".into()),
            Diagnostic::NotYet("/LTCG".into()),
            Diagnostic::NotYet("/WINMD".into()),
            Diagnostic::Bad("option '/OUT' needs a value".into()),
            Diagnostic::Bad("invalid option '/MACHINE:Z80'".into()),
        ]
    );
    // /LTCG:OFF asks for nothing.
    let (c, d) = config("/LTCG:OFF");
    assert!(!c.ltcg && d.is_empty());
    // KNOWN is sorted and has no duplicates (a binary search could use it).
    assert!(KNOWN.windows(2).all(|w| w[0] < w[1]), "KNOWN is sorted");
    for name in IGNORED {
        assert!(KNOWN.contains(name), "{name} is in KNOWN");
    }
}

#[test]
fn directives_are_limited() {
    let mut c = Config::default();
    let mut d = Vec::new();
    let args = crate::args::directives(
        br#"/DEFAULTLIB:"LIBCMT" /FAILIFMISMATCH:"_MSC_VER=1900" /DISALLOWLIB:msvcrt.lib /OUT:x.exe"#,
    );
    apply_directives(&mut c, &args, &mut d);
    assert_eq!(c.default_libs, ["LIBCMT"]);
    assert_eq!(c.fail_if_mismatch, [("_MSC_VER".into(), "1900".into())]);
    assert_eq!(c.disallow_libs, ["msvcrt.lib"]);
    assert_eq!(c.out, None);
    assert_eq!(d.len(), 1);
}
