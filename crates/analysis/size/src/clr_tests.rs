//! A managed image's model and symbols, from testdata/clr/Sample.dll (see
//! tools/clr) and, where there's a .NET runtime, the framework's.

use super::*;

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../../testdata/clr/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn sample() -> (Vec<u8>, Model, Vec<Symbol>) {
    let data = testdata("Sample.dll");
    let image = Image::parse(&data).unwrap();
    let (model, symbols) = model(&image).unwrap();
    (data, model, symbols)
}

fn find<'s>(symbols: &'s [Symbol], name: &str) -> &'s Symbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

#[test]
fn recognizes_managed_images() {
    assert!(is_managed(&testdata("Sample.dll")));
    assert!(is_managed(&testdata("Hello.exe")));
    assert!(!is_managed(&testdata("Sample.pdb")));
    assert!(!is_managed(b"MZ"));
}

#[test]
fn types_are_compilands_in_namespaces() {
    let (_, model, _) = sample();
    let c = model
        .compilands
        .iter()
        .find(|c| c.name == "Sample.Circle")
        .unwrap();
    assert_eq!(model.libraries[c.library as usize].name, "Sample");
    let nested = model
        .compilands
        .iter()
        .find(|c| c.name == "Sample.Circle/Nested`1")
        .unwrap();
    assert_eq!(model.libraries[nested.library as usize].name, "Sample");
    let module = model
        .compilands
        .iter()
        .find(|c| c.name == "<Module>")
        .unwrap();
    assert_eq!(
        model.libraries[module.library as usize].name,
        GLOBAL_NAMESPACE
    );
    let i = model
        .compilands
        .iter()
        .position(|c| c.name == "Sample.Circle")
        .unwrap();
    assert_eq!(namespace_of(&model, i as u32), Some("Sample"));
    let s = model
        .compilands
        .iter()
        .position(|c| c.name == "[CLR heap: #Strings]")
        .unwrap();
    assert_eq!(namespace_of(&model, s as u32), None);
    let groups: Vec<&str> = model.groups.iter().map(|g| g.name.as_str()).collect();
    for g in [
        "[CLR header]",
        "[CLR IL and data]",
        "[CLR metadata]",
        "[CLR resources]",
        "[CLR strong-name signature]",
    ] {
        assert!(groups.contains(&g), "{groups:?}");
    }
}

#[test]
fn the_model_has_every_symbol() {
    // Every symbol's bytes are a piece of the model: none dropped for
    // lying outside every group.
    let (_, model, symbols) = sample();
    let pieces: u64 = model.pieces.iter().map(|p| p.range.len).sum();
    let symbols: u64 = symbols.iter().map(|s| s.size.file).sum();
    assert_eq!(pieces, symbols);
}

#[test]
fn methods_own_their_bodies_and_metadata() {
    let (data, _, symbols) = sample();
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    let code = md.find_type("Sample", "Code").unwrap();
    let switch = md
        .type_methods(code)
        .find(|&m| md.string(md.row::<MethodDefRow>(m).unwrap().name) == "Switch")
        .unwrap();
    let body = cli.method_body(switch).unwrap().unwrap();
    let s = find(&symbols, "Sample.Code::Switch(int32)");
    assert_eq!(s.kind, SymbolKind::Function);
    assert_eq!(s.size.file, u64::from(body.size));
    // Its MethodDef row, its parameter's, and its name: its metadata.
    let meta: u64 = symbols
        .iter()
        .filter(|s| s.name == "[metadata] Sample.Code::Switch(int32)")
        .map(|s| s.size.file)
        .sum();
    let rows = md.tables().layout(TableId::MethodDef).row_size
        + md.tables().layout(TableId::Param).row_size;
    assert!(meta >= u64::from(rows) + "Switch\0".len() as u64, "{meta}");
    // A generic method's name has its parameters.
    find(&symbols, "Sample.Code::Generic<T, U>(!!0, !!1)");
    // A vararg one, its sentinel.
    find(&symbols, "Sample.Code::VarArgs(int32)");
    // Strings() loads a user string only it loads.
    assert!(
        symbols
            .iter()
            .filter(|s| s.name == "[metadata] Sample.Code::Strings()")
            .map(|s| s.size.file)
            .sum::<u64>()
            > 60
    );
}

#[test]
fn fields_data_resources_and_structures() {
    let (data, _, symbols) = sample();
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    // Circle.Table's 17 bytes are a field of <PrivateImplementationDetails>.
    let data_syms: Vec<&Symbol> = symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Data)
        .collect();
    assert_eq!(data_syms.len(), 2, "{data_syms:?}");
    assert!(data_syms.iter().any(|s| s.size.file == 17));
    assert!(data_syms.iter().any(|s| s.size.file == 24));
    let r = find(&symbols, "[CLR resource: Sample.Strings.resources]");
    assert_eq!((r.kind, r.size.file), (SymbolKind::ClrResource, 4 + 0x332));
    find(&symbols, "[CLR resource: Sample.notes.txt]");
    let sn = find(&symbols, "[CLR strong-name signature]");
    assert_eq!(sn.size.file, 128);
    let h = find(&symbols, "[CLR header]");
    assert_eq!(h.size.file, 72);
    find(&symbols, "[CLR metadata: TypeRef]");
    find(&symbols, "[CLR metadata: AssemblyRef]");
    find(&symbols, "[CLR heap: #GUID]");
    // Every byte of the metadata is someone's, once.
    let md = cli.header().metadata;
    let (start, end) = (u64::from(md.rva), u64::from(md.rva) + u64::from(md.size));
    let mut inside: Vec<(u64, u64)> = symbols
        .iter()
        .filter(|s| s.addr >= start && s.addr < end)
        .map(|s| (s.addr, s.addr + s.size.file))
        .collect();
    inside.sort_unstable();
    assert_eq!(inside.first().unwrap().0, start);
    for w in inside.windows(2) {
        assert_eq!(w[0].1, w[1].0, "a gap or overlap at {:#x}", w[0].1);
    }
    assert_eq!(inside.last().unwrap().1, end);
}

#[test]
fn custom_attributes_belong_to_what_they_decorate() {
    let data = testdata("Sample.dll");
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    let owners = attribute_owners(md);
    let circle = md.find_type("Sample", "Circle").unwrap();
    // Circle's four attributes (Note three times, Serializable is a flag).
    assert!(owners.iter().filter(|&&o| o == Owner::Type(circle)).count() >= 3);
    // The assembly's: no type's.
    assert!(owners.contains(&Owner::Shared));
}

#[test]
fn lists_a_methods_il() {
    let data = testdata("Sample.dll");
    let text = listing(&data, "Sample.Code::Switch(int32)").unwrap();
    assert!(text.starts_with("Sample.Code::Switch(int32):\n"), "{text}");
    assert!(text.contains(": switch (IL_"), "{text}");
    // By its bare name too, and its type's.
    assert_eq!(listing(&data, "Switch").unwrap(), text);
    assert_eq!(listing(&data, "Code::Switch").unwrap(), text);
    let e = listing(&data, "Exceptions").unwrap();
    assert!(
        e.contains(".try IL_") && e.contains("catch [System.Runtime]System.Exception"),
        "{e}"
    );
    assert!(e.contains("// locals ("), "{e}");
    assert!(
        listing(&data, ".ctor")
            .unwrap_err()
            .contains("methods are named .ctor")
    );
    assert!(listing(&data, "NoSuchMethod").is_err());
    let abs = listing(&data, "Sample.IShape::get_Area()").unwrap();
    assert!(abs.contains("no body"), "{abs}");
}

#[test]
fn framework_assemblies_are_wholly_attributed() {
    // (Where there's a .NET runtime: CoreLib, ReadyToRun and large.)
    let Some(dir) = [
        "C:/Program Files/dotnet/shared/Microsoft.NETCore.App",
        "/usr/share/dotnet/shared/Microsoft.NETCore.App",
        "/usr/lib/dotnet/shared/Microsoft.NETCore.App",
        "/usr/local/share/dotnet/shared/Microsoft.NETCore.App",
    ]
    .iter()
    .filter_map(|r| std::fs::read_dir(r).ok())
    .flat_map(|d| d.flatten().map(|e| e.path()))
    .find(|p| p.join("System.Private.CoreLib.dll").is_file()) else {
        return;
    };
    for name in ["System.Private.CoreLib.dll", "System.Linq.dll"] {
        let data = std::fs::read(dir.join(name)).unwrap();
        let image = Image::parse(&data).unwrap();
        let (model, symbols) = model(&image).unwrap();
        assert!(model.compilands.len() > 10, "{name}");
        let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
        let md = cli.header().metadata;
        let (start, end) = (u64::from(md.rva), u64::from(md.rva) + u64::from(md.size));
        let total: u64 = symbols
            .iter()
            .filter(|s| s.addr >= start && s.addr < end)
            .map(|s| s.size.file)
            .sum();
        assert_eq!(total, end - start, "{name}");
        assert!(
            symbols.iter().any(|s| s.name == "[ReadyToRun header]"),
            "{name}"
        );
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function));
    }
}
