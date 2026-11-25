//! The fuzzer's targets: each takes an input's bytes and runs everything
//! that reads that kind of input. File formats run every `hexcavator` command
//! that reads them (in-process, on a scratch file); the rest (demanglers,
//! decoders, text formats) are called directly.
//!
//! A target must return for every input: a panic, a hang, a stack
//! overflow, or a runaway allocation is a finding (see `main.rs`).

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

/// One target: what it reads, where its seeds come from, and how to run
/// an input.
pub struct Target {
    /// Its name (`pe`, `pdb`, `demangle`, ...).
    pub name: &'static str,
    /// What it reads, in a phrase.
    pub about: &'static str,
    /// Which of the corpus's files seed it.
    pub seeds: fn(&Path, &[u8]) -> bool,
    /// Runs an input.
    pub run: fn(&[u8], &Scratch),
}

/// Where file targets write their input.
pub struct Scratch {
    /// The directory.
    pub dir: PathBuf,
}

impl Scratch {
    /// Writes `data` as the scratch file named `name`, and its path.
    fn file(&self, name: &str, data: &[u8]) -> String {
        let path = self.dir.join(name);
        std::fs::write(&path, data).expect("writing the scratch file");
        path.to_string_lossy().into_owned()
    }
}

/// Runs `hexcavator` with `args`, as the command line would (its output goes
/// to the child's standard output, which the supervisor discards), but
/// freeing what it is done with: a command left for the exit would keep
/// the scratch file mapped, and the next input couldn't be written to it.
fn hexcavator(args: &[&str]) {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let _ = hexcavator::run_in_process(&args);
}

fn starts(data: &[u8], magic: &[u8]) -> bool {
    data.starts_with(magic)
}

fn is_pe(_: &Path, d: &[u8]) -> bool {
    starts(d, b"MZ")
}
fn is_coff_object(p: &Path, d: &[u8]) -> bool {
    p.extension().is_some_and(|e| e == "obj") && !starts(d, b"MZ")
}
fn is_ltcg(_: &Path, d: &[u8]) -> bool {
    turbonop_pe::ltcg::LtcgObject::is_ltcg(d)
}
fn is_archive(_: &Path, d: &[u8]) -> bool {
    starts(d, b"!<arch>\n") || starts(d, b"!<thin>\n")
}
fn is_elf(_: &Path, d: &[u8]) -> bool {
    starts(d, b"\x7fELF")
}
fn is_macho(_: &Path, d: &[u8]) -> bool {
    [
        b"\xcf\xfa\xed\xfe",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xfe\xed\xfa\xce",
        b"\xca\xfe\xba\xbe",
    ]
    .iter()
    .any(|m| starts(d, *m))
}
fn is_minidump(_: &Path, d: &[u8]) -> bool {
    starts(d, b"MDMP")
}
fn is_core(_: &Path, d: &[u8]) -> bool {
    // ET_CORE, either byte order.
    starts(d, b"\x7fELF") && (d.get(16..18) == Some(&[4, 0]) || d.get(16..18) == Some(&[0, 4]))
}
fn is_wasm(_: &Path, d: &[u8]) -> bool {
    starts(d, b"\0asm")
}
fn is_legacy(_: &Path, d: &[u8]) -> bool {
    turbonop_mz::MzFile::parse(d)
        .ok()
        .and_then(|m| m.new_header())
        .is_some_and(|h| h.kind != turbonop_mz::NewHeaderKind::Pe)
}
fn is_split_dwarf(p: &Path, d: &[u8]) -> bool {
    is_elf(p, d) && p.extension().is_some_and(|e| e == "dwo" || e == "dwp")
}
fn is_pdb(_: &Path, d: &[u8]) -> bool {
    starts(d, b"Microsoft C/C++ MSF 7.00")
}
fn is_text(p: &Path, _: &[u8]) -> bool {
    p.extension()
        .is_some_and(|e| e == "txt" || e == "map" || e == "log")
}
fn is_code(_: &Path, d: &[u8]) -> bool {
    // Any binary: its bytes are a stream of instructions as good as any.
    !d.is_empty() && d.len() < (8 << 20)
}
fn is_etl(p: &Path, _: &[u8]) -> bool {
    p.extension().is_some_and(|e| e == "etl")
}
fn is_hxtrace(p: &Path, _: &[u8]) -> bool {
    p.extension().is_some_and(|e| e == "hxtrace")
}
fn is_xml(p: &Path, _: &[u8]) -> bool {
    p.extension().is_some_and(|e| e == "xml")
}
fn is_any(_: &Path, _: &[u8]) -> bool {
    true
}
fn is_der(p: &Path, d: &[u8]) -> bool {
    starts(d, &[0x30, 0x82])
        && p.extension()
            .is_some_and(|e| e == "cer" || e == "der" || e == "p7b")
}

/// DER from a signature: as a certificate, a CMS SignedData (and its
/// signers' checks), and a code requirement.
fn der(data: &[u8], _: &Scratch) {
    use turbonop_signature::{cms, verify, x509};
    if let Ok(c) = x509::Certificate::from_der(data) {
        let _ = verify::CertInfo::of(&c);
        let _ = (
            c.extended_key_usage(),
            c.key_usage(),
            c.is_ca(),
            c.subject_key_id(),
            c.authority_key_id(),
        );
    }
    if let Ok(sd) = cms::SignedData::from_der(data) {
        let content = sd.content.as_ref().map(|c| c.digested().into_owned());
        for s in &sd.signers {
            let mut signer = verify::signer(&sd, s, content.as_deref());
            let stamps = verify::timestamps(&sd, s);
            let (at, what) = verify::judged_at(&stamps);
            verify::finish(
                &mut signer,
                at,
                what,
                turbonop_signature::oid::EKU_CODE_SIGNING,
            );
        }
        if let Some(c) = &sd.content {
            let _ = cms::IndirectData::parse(c.tlv);
            let _ = cms::TstInfo::parse(&c.digested());
        }
    }
    let _ = turbonop_signature::requirement::decode(data);
    let _ = turbonop_signature::requirement::decode_set(data);
}

const SIZE_SOURCES: &str = "sections,segments,symbols,rawsymbols,fullsymbols,compileunits,armembers,inputfiles,inlines,archs";

/// The format-independent view: everything it reads.
fn image(data: &[u8]) {
    let Ok(image) = turbonop_image::Image::parse(data) else {
        return;
    };
    let _ = (
        image.arch(),
        image.kind(),
        image.hybrid(),
        image.debug_ids(),
        image.symbols(),
        image.extent(),
    );
    for s in image.sections() {
        // Its start, its whole extent, and one past it.
        let _ = image.bytes_at(s.addr, s.vm_size.min(1 << 20));
        let _ = image.bytes_at(s.addr.saturating_add(s.vm_size), 1);
    }
}

fn pe(data: &[u8], s: &Scratch) {
    image(data);
    let f = s.file("input-pe.bin", data);
    hexcavator(&["dump", "/all", "/nopdb", &f]);
    hexcavator(&["size", "-d", "sections,segments", "-n", "0", &f]);
    hexcavator(&["info", &f]);
    hexcavator(&["batch", &f]);
    hexcavator(&["checksec", "-json", &f]);
    hexcavator(&["signature", "-json", &f]);
    hexcavator(&["signature", &f]);
    hexcavator(&["deps", "-json", "-imports", &f]);
    hexcavator(&[
        "symbolize",
        &f,
        "0x1000",
        "0x140001000",
        &format!("{}+0x10", "input-pe"),
    ]);
}

fn coff(data: &[u8], s: &Scratch) {
    image(data);
    // (Its CodeView, as the PDB a linker would write of it.)
    if let Some(pdb) = turbonop_debuginfo::coff_object::pdb_of(data) {
        debug_info(&pdb);
    }
    let f = s.file("input-coff.bin", data);
    hexcavator(&["dump", "/all", &f]);
    hexcavator(&["size", "-d", "sections,symbols,armembers", "-n", "0", &f]);
    hexcavator(&["query", &f, "kind!=public"]);
}

fn ltcg(data: &[u8], s: &Scratch) {
    if let Ok(o) = turbonop_pe::ltcg::LtcgObject::parse(data) {
        let _ = o.backend_args();
        let _ = o.compiler();
        let _ = o.globals();
        let _ = o.types();
    }
    let f = s.file("input-ltcg.bin", data);
    hexcavator(&["ltcg", "-all", &f]);
    hexcavator(&["dump", "/all", &f]);
    hexcavator(&["info", &f]);
}

fn archive(data: &[u8], s: &Scratch) {
    let f = s.file("input-archive.bin", data);
    hexcavator(&["dump", "/all", &f]);
    hexcavator(&["size", "-d", "armembers,sections,symbols", "-n", "0", &f]);
}

fn unix(name: &str, data: &[u8], s: &Scratch) {
    image(data);
    debug_info(data);
    let f = s.file(name, data);
    hexcavator(&["size", "-d", SIZE_SOURCES, "-n", "0", &f]);
    hexcavator(&["size", "-d", "symbols", "-n", "0", "-csv", &f]);
    hexcavator(&["info", &f]);
    hexcavator(&["info", "-json", &f]);
    hexcavator(&["checksec", "-audit", &f]);
    hexcavator(&["dump", "-all", &f]);
    hexcavator(&["checksec", "-json", &f]);
    hexcavator(&["signature", "-json", &f]);
    hexcavator(&["dwarf", "-all", &f]);
    hexcavator(&["dwarf", "-lookup", "0x1000", &f]);
    hexcavator(&["dwarf", "-find", "main", &f]);
    hexcavator(&["deps", "-json", &f]);
    hexcavator(&[
        "symbolize",
        "-json",
        &f,
        "0x1000",
        "0x401000",
        "0x100000f00",
        "0x10",
    ]);
    hexcavator(&["init", "-json", &f]);
    hexcavator(&["init", "-depth", "8", &f]);
    hexcavator(&["resources", "-all", &f]);
    hexcavator(&["resources", "-all", "-json", &f]);
    // Each function's code, by the names its symbols give.
    for name in ["main", "compute", "jumps", "_main", "add", "square"] {
        hexcavator(&["size", "-disassemble", name, &f]);
        hexcavator(&["size", "-disassemble", name, "-json", &f]);
    }
}

fn is_xz(_: &Path, d: &[u8]) -> bool {
    starts(d, b"\xFD7zXZ\0")
}

fn xz(data: &[u8], _: &Scratch) {
    // (A small limit: what matters is that damage is refused cleanly.)
    let _ = turbonop_debuginfo::decompress::xz(data, 1 << 24);
}

fn elf(data: &[u8], s: &Scratch) {
    unix("input-elf.bin", data, s);
}

fn macho(data: &[u8], s: &Scratch) {
    if let Ok(m) = turbonop_macho::MachO::parse(data) {
        let _ = (m.objc(), m.fixups(), m.exports());
        let _ = (m.function_starts(), m.unwind_info(), m.code_signature());
        let _ = m.debug_map();
    }
    unix("input-macho.bin", data, s);
}

fn wasm(data: &[u8], s: &Scratch) {
    if let Ok(m) = turbonop_wasm::Module::parse(data) {
        let _ = (
            m.imports(),
            m.linking_symbols(),
            m.segment_info(),
            m.relocations(),
        );
    }
    unix("input-wasm.bin", data, s);
}

fn pdb(data: &[u8], s: &Scratch) {
    let f = s.file("input-pdb.bin", data);
    hexcavator(&["pdb", "-all", &f]);
    hexcavator(&["types", "-n", "0", &f]);
    debug_info(data);
}

/// A 16-bit NE or linear (LE/LX) executable: every table, and hexcavator info.
fn legacy(data: &[u8], s: &Scratch) {
    if let Ok(ne) = turbonop_ne::NeFile::parse(data) {
        for seg in ne.segments() {
            let _ = (ne.segment_data(&seg), ne.relocations(&seg));
        }
        for r in ne.resources() {
            let _ = ne.resource_data(&r);
        }
        let _ = (
            ne.resident_names(),
            ne.nonresident_names(),
            ne.modules(),
            ne.entries(),
        );
    }
    if let Ok(lx) = turbonop_lx::LxFile::parse(data) {
        let _ = (
            lx.objects(),
            lx.resources(),
            lx.entries(),
            lx.import_modules(),
        );
        let _ = (lx.resident_names(), lx.nonresident_names());
        for (i, p) in (0u32..).zip(lx.pages().iter().take(4096)) {
            let _ = (lx.page_data(p), lx.fixups(i));
        }
    }
    let f = s.file("input-legacy.bin", data);
    hexcavator(&["info", &f]);
    hexcavator(&["dump", &f]);
}

/// A split DWARF file (a `.dwo`, or a `.dwp`), read with the skeletons of
/// the programs it was split from (tools/elf/split.cmd), DWARF 4 and 5.
fn split_dwarf(data: &[u8], _: &Scratch) {
    static IMAGES: [&[u8]; 2] = [
        include_bytes!("../../../testdata/elf/split-dwarf4"),
        include_bytes!("../../../testdata/elf/split-dwarf5"),
    ];
    for image in IMAGES {
        let Ok(mut debug) = turbonop_debuginfo::DebugInfo::parse(image) else {
            continue;
        };
        if let Some(d) = debug.dwarf_mut()
            && d.add_split(data).is_ok()
        {
            queries(&debug);
        }
    }
}

/// A Mach-O object file of a debug map, read as each object of the
/// debug maps of testdata/macho/oso's images (its addresses moved to the
/// image's by that object's map), and each query asked.
fn debug_map_object(data: &[u8], _: &Scratch) {
    use turbonop_debuginfo::dwarf_backend::DwarfInfo;
    static IMAGES: [&[u8]; 2] = [
        include_bytes!("../../../testdata/macho/oso/oso-arm64"),
        include_bytes!("../../../testdata/macho/oso/oso-x86_64"),
    ];
    for image in IMAGES {
        let Ok(m) = turbonop_macho::MachO::parse(image) else {
            continue;
        };
        for o in m.debug_map() {
            let Ok(mut d) = DwarfInfo::from_debug_map(image) else {
                continue;
            };
            if d.add_object(data, &o.symbols, "", "").is_ok() {
                queries(&turbonop_debuginfo::DebugInfo::Dwarf(Box::new(d)));
            }
        }
    }
}

/// The debug information's every query (a PDB's, or DWARF's).
fn debug_info(data: &[u8]) {
    if let Ok(debug) = turbonop_debuginfo::DebugInfo::parse(data) {
        queries(&debug);
    }
}

fn queries(debug: &turbonop_debuginfo::DebugInfo<'_>) {
    let _ = debug.compilands();
    let _ = debug.contributions();
    let _ = debug.inline_sites();
    let _ = debug.source_lines();
    let _ = debug.annotations();
    let _ = debug.groups();
    let _ = debug.classes();
    let _ = debug.linker_info();
    // Frames: at symbols' addresses and inside them, and anywhere.
    let mut locator = debug.locator();
    for addr in [0, 1, 0x1000, 0x1_0000, u64::from(u32::MAX), u64::MAX] {
        let _ = locator.frames(addr);
    }
    if let Ok(symbols) = debug.symbols() {
        for sym in symbols.iter().take(256) {
            let _ = locator.frames(sym.addr);
            let _ = locator.frames(sym.addr.saturating_add(sym.size.unwrap_or(1) / 2));
        }
    }
    if let Ok(symbols) = debug.symbols() {
        for sym in symbols.iter().take(64) {
            let _ = debug.variables(sym.compiland, sym.addr);
            if sym.type_index != 0 {
                let ty = turbonop_debuginfo::TypeRef(sym.type_index);
                let _ = (
                    debug.type_size(ty),
                    debug.type_shape(ty),
                    debug.type_name(ty),
                );
                let _ = (debug.enumerators(ty), debug.layout(ty));
                let _ = turbonop_debuginfo::value::read(
                    debug,
                    ty,
                    &[0xA5; 256],
                    turbonop_debuginfo::value::Limits::default(),
                );
            }
        }
    }
    if let Ok(layouts) = debug.layouts() {
        for l in layouts.iter().take(256) {
            let _ = debug.layout(l.ty);
            let _ = turbonop_debuginfo::layout::flat_fields(debug, l.ty);
            let _ = turbonop_debuginfo::value::read(
                debug,
                l.ty,
                &[0xA5; 256],
                turbonop_debuginfo::value::Limits::default(),
            );
        }
    }
}

fn text(data: &[u8], s: &Scratch) {
    let text = String::from_utf8_lossy(data);
    let _ = turbonop_linkmap::parse(&text);
    let _ = turbonop_linkerlog::parse(&text);
    let f = s.file("input-text.bin", data);
    hexcavator(&["linkerlog", &f]);
}

/// Everything a target says, and its whole address space read.
fn examine(t: &dyn turbonop_target::Target) {
    let _ = (t.arch(), t.os(), t.abi(), t.snapshot(), t.capabilities());
    for th in t.threads() {
        let _ = th.registers.to_unwind();
    }
    let _ = t.exception();
    let mut images = Vec::new();
    for m in t.modules() {
        let _ = (m.file_name(), t.module_at(m.base));
        images.push(m.base);
    }
    let space = turbonop_target::AddressSpace::new(t.memory(), Vec::new());
    for th in t.threads() {
        if let Some(stack) = &th.stack {
            let _ = space.runs(stack.start, (stack.end - stack.start).min(1 << 20));
        }
    }
    for base in images {
        let _ = turbonop_target::build_id::elf_in_memory(t.memory(), base);
        let _ = space.runs(base, 1 << 16);
    }
}

fn minidump(data: &[u8], _: &Scratch) {
    if let Ok(t) = turbonop_target::MinidumpTarget::new(data) {
        examine(&t);
    }
}

fn macho_core(data: &[u8], _: &Scratch) {
    // (Any Mach-O file, made a core: its type changed.)
    let mut d = data.to_vec();
    if d.len() >= 16 && d.starts_with(&[0xCF, 0xFA, 0xED, 0xFE]) {
        d[12..16].copy_from_slice(&4u32.to_le_bytes());
    }
    if let Ok(t) = turbonop_target::MachCoreTarget::new(&d) {
        examine(&t);
        let _ = t.addressable_bits();
    }
}

fn core(data: &[u8], _: &Scratch) {
    if let Ok(t) = turbonop_target::CoreTarget::new(data) {
        examine(&t);
        let c = t.core();
        let _ = (
            c.process_info(),
            c.siginfo(),
            c.auxv(),
            c.files(),
            c.pac_masks(),
        );
        for th in c.threads() {
            let _ = th.note(turbonop_elf::coredump::NT_PRFPREG);
        }
    }
}

fn etl(data: &[u8], _: &Scratch) {
    let Ok(etl) = turbonop_etl::Etl::parse(data) else {
        return;
    };
    for buffer in etl.buffers() {
        let Ok(buffer) = buffer else { break };
        if let Ok(records) = buffer.records() {
            for event in records.flatten() {
                for _ in event.extended_data() {}
            }
        }
    }
    if let Ok(mut cursor) = etl.events() {
        while let Some(event) = cursor.next() {
            if let Ok(event) = event {
                for _ in event.extended_data() {}
            }
        }
    }
}

fn etl_import(data: &[u8], _: &Scratch) {
    if let Ok(etl) = turbonop_etl::Etl::parse(data) {
        let _ = turbonop_trace::etw::import(&etl);
    }
}

fn hxtrace(data: &[u8], _: &Scratch) {
    let Ok(trace) = turbonop_trace::index::read(data, None) else {
        return;
    };
    for s in trace.samples.stack.iter().chain(&trace.switches.stack) {
        let _ = trace.stacks.frames(*s).count();
    }
}

fn manifest(data: &[u8], _: &Scratch) {
    use turbonop_etw::manifest::{Manifest, decode, wevt_template};
    // The resource of an image, or the input as a resource itself.
    let wevt = wevt_template(data).unwrap_or(data);
    if let Ok(m) = Manifest::parse(wevt) {
        for p in &m.providers {
            for e in &p.events {
                if let Some(t) = p.template(e) {
                    let _ = decode(t, data, 8);
                }
            }
        }
    }
}

fn stacktags(data: &[u8], _: &Scratch) {
    let text = String::from_utf8_lossy(data);
    if let Ok(tags) = turbonop_trace_analysis::StackTags::from_xml(&text) {
        let _ = tags.frame_tag("ntdll.dll", Some("RtlAllocateHeap"));
    }
    if let Ok(folds) = turbonop_trace_analysis::Folds::from_text(&text) {
        let _ = folds.frame_rule("ntdll.dll", None);
    }
}

fn demangle(data: &[u8], _: &Scratch) {
    let name = String::from_utf8_lossy(data);
    let _ = turbonop_demangle::msvc::demangle(&name, 0);
    let _ = turbonop_demangle::itanium::demangle(&name);
    let mut out = String::new();
    let _ = turbonop_demangle::rust::demangle_into(&name, &mut out);
    // Swift: as is, and as a Swift name whatever its start (most inputs
    // would stop at the prefix check), in both printings.
    let mut swift = turbonop_demangle::swift::Demangler::new();
    for name in [name.to_string(), format!("$s{name}"), format!("_T{name}")] {
        out.clear();
        let _ = swift.demangle_into(&name, &mut out);
        let _ = swift.demangle_short_into(&name, &mut out);
    }
    let mut tree = turbonop_demangle::swift::Tree::default();
    let _ = turbonop_demangle::swift::demangle_type_as_node(&mut tree, &name);
}

fn x86(data: &[u8], _: &Scratch) {
    for mode in [
        turbonop_x86::Mode::Bits16,
        turbonop_x86::Mode::Bits32,
        turbonop_x86::Mode::Bits64,
    ] {
        let mut at = 0;
        while at < data.len().min(1 << 16) {
            match turbonop_x86::decode(&data[at..], mode, at as u64) {
                Ok(i) => {
                    let _ = turbonop_x86::format::to_string(&i);
                    at += i.len().max(1);
                }
                Err(_) => at += 1,
            }
        }
    }
}

fn arm64(data: &[u8], _: &Scratch) {
    for (i, w) in data.as_chunks::<4>().0.iter().take(1 << 14).enumerate() {
        let word = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        if let Some(inst) = turbonop_arm64::decode(word, i as u64 * 4) {
            let _ = turbonop_arm64::format(&inst);
        }
        let _ = turbonop_arm64::explain(word);
    }
}

/// A32 and T32 (and Thumb functions' IT blocks): decoded and written,
/// with branch targets at the top of the address space.
#[allow(clippy::cast_possible_truncation)]
fn arm32(data: &[u8], _: &Scratch) {
    use turbonop_arm32::format::{Targets, format, format_in_it};
    for (i, w) in data.as_chunks::<4>().0.iter().take(1 << 14).enumerate() {
        let word = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let at = u64::from(u32::MAX).wrapping_sub(i as u64 * 4);
        if let Some(inst) = turbonop_arm32::decode_a32(word) {
            let _ = format(&inst, at, Targets::Addresses);
        }
        let first = (word >> 16) as u16;
        if let Some(inst) = turbonop_arm32::decode_t32(first, Some(word as u16)) {
            let mut s = String::new();
            format_in_it(&inst, at, Targets::Offsets, Some(word & 15), &mut s);
            let _ = turbonop_arm32::format::branch_target(&inst, at);
        }
    }
    let _ = turbonop_code::sweep(turbonop_code::Arch::Arm, data, 1);
}

/// The decoders generated from LLVM's TableGen records (RISC-V,
/// LoongArch, PowerPC, MIPS and microMIPS), each in its configurations,
/// with their formatters; and code read as each, its control flow.
fn tablegen(data: &[u8], _: &Scratch) {
    let data = &data[..data.len().min(1 << 15)];
    // Each decoder over every offset (a halfword's: RISC-V's compressed
    // instructions, microMIPS's), each instruction written at an address
    // near the top of memory.
    let at = |k: usize| u64::MAX.wrapping_sub(k as u64 * 2);
    for rv64 in [false, true] {
        let f = turbonop_riscv::Features::standard(rv64);
        for k in (0..data.len()).step_by(2) {
            if let Some(i) = turbonop_riscv::decode(&data[k..], &f) {
                let _ = turbonop_riscv::format::format(&i, at(k), &f);
            }
        }
    }
    for f in [
        turbonop_loongarch::Features::la64(),
        turbonop_loongarch::Features::la32(),
    ] {
        for k in (0..data.len()).step_by(4) {
            if let Some(i) = turbonop_loongarch::decode(&data[k..], &f) {
                let _ = turbonop_loongarch::format::format(&i, at(k), &f);
            }
        }
    }
    for (ppc64, big) in [(true, true), (true, false), (false, true)] {
        let f = turbonop_powerpc::Features::standard(ppc64);
        for k in (0..data.len()).step_by(4) {
            if let Some(i) = turbonop_powerpc::decode(&data[k..], big, &f) {
                let _ = turbonop_powerpc::format::format(&i, at(k), &f);
            }
        }
    }
    // MIPS: R2, R6, 64-bit, Octeon; microMIPS R2 and R6.
    for (flags, mips64, step) in [
        (0x7000_0000, false, 4),
        (0x9000_0000, false, 4),
        (0x8000_0000, true, 4),
        (0x808B_0000, true, 4),
        (0x7200_0000, false, 2),
        (0x9200_0000, false, 2),
    ] {
        let f = turbonop_mips::Features::from_elf(flags, mips64);
        for big in [true, false] {
            for k in (0..data.len()).step_by(step) {
                if let Some(i) = turbonop_mips::decode(&data[k..], big, &f) {
                    let _ = turbonop_mips::format::format(&i, at(k), &f);
                    let _ = turbonop_mips::branch_target(&i, at(k));
                }
            }
        }
    }
    // Their code's flow, as each reads it.
    use turbonop_code::{Arch, Mode};
    for (arch, mode) in [
        (Arch::RiscV64, Mode::default()),
        (Arch::LoongArch64, Mode::default()),
        (Arch::PowerPc64, Mode::endian(true)),
        (
            Arch::Mips,
            Mode {
                mips_flags: Some(0x7200_0000),
                ..Mode::endian(true)
            },
        ),
        (Arch::Mips64, Mode::endian(false)),
    ] {
        let _ = turbonop_code::sweep_in(arch, mode, data, 0x1000);
    }
}

fn query(data: &[u8], _: &Scratch) {
    let text = String::from_utf8_lossy(data);
    let _ = turbonop_query::parse(&text, &turbonop_size::table::COLUMNS);
}

fn sourcemap(data: &[u8], _: &Scratch) {
    let _ = turbonop_size::sourcemap::parse(data);
}

/// The harness's own check (`turbonop-fuzz selftest`): an input's first
/// byte says what to do wrong: panic, overflow the stack, allocate too
/// much, or never return.
fn selftest(data: &[u8], _: &Scratch) {
    fn deep(n: u64) -> u64 {
        let pad = [n; 64];
        if n == u64::MAX {
            0
        } else {
            std::hint::black_box(deep(n + 1)) + pad[0]
        }
    }
    match data.first() {
        Some(b'P') => panic!("selftest: a panic"),
        Some(b'S') => {
            std::hint::black_box(deep(0));
        }
        Some(b'A') => {
            // (A terabyte on 64-bit hosts: past any cap. Written so that it
            // still compiles for 32-bit targets, which never run it.)
            let v: Vec<u8> = vec![1; 1 << (usize::BITS - 24)];
            std::hint::black_box(v);
        }
        Some(b'L') => loop {
            std::hint::black_box(0);
        },
        _ => {}
    }
}

/// The harness's self-test target (not in [`TARGETS`]).
pub const SELFTEST: Target = Target {
    name: "selftest",
    about: "the harness's own check",
    seeds: is_any,
    run: selftest,
};

/// Every target.
pub const TARGETS: &[Target] = &[
    Target {
        name: "pe",
        about: "PE images: every dump, size by sections, info, batch",
        seeds: is_pe,
        run: pe,
    },
    Target {
        name: "coff",
        about: "COFF objects: dump, size",
        seeds: is_coff_object,
        run: coff,
    },
    Target {
        name: "ltcg",
        about: "LTCG (/GL) objects: hexcavator ltcg, dump, info",
        seeds: is_ltcg,
        run: ltcg,
    },
    Target {
        name: "archive",
        about: "archives (.lib, .a): dump, size by member",
        seeds: is_archive,
        run: archive,
    },
    Target {
        name: "elf",
        about: "ELF files: size by every source, disassembly",
        seeds: is_elf,
        run: elf,
    },
    Target {
        name: "macho",
        about: "Mach-O files: size by every source, disassembly",
        seeds: is_macho,
        run: macho,
    },
    Target {
        name: "wasm",
        about: "WebAssembly modules: size by every source, disassembly",
        seeds: is_wasm,
        run: wasm,
    },
    Target {
        name: "pdb",
        about: "PDBs: the whole dump, types, and every debug-information query",
        seeds: is_pdb,
        run: pdb,
    },
    Target {
        name: "minidump",
        about: "minidumps as targets: threads, contexts, modules and identities, maps, memory",
        seeds: is_minidump,
        run: minidump,
    },
    Target {
        name: "core",
        about: "ELF core files as targets: notes, registers, mapped files, build IDs in memory",
        seeds: is_core,
        run: core,
    },
    Target {
        name: "macho-core",
        about: "Mach-O core files as targets: thread states, exception states, notes, images",
        seeds: is_macho,
        run: macho_core,
    },
    Target {
        name: "etl",
        about: "event trace logs: buffers, records, decompression, the merge",
        seeds: is_etl,
        run: etl,
    },
    Target {
        name: "etl-import",
        about: "trace import from ETL: kernel event decoding, stack matching, the model",
        seeds: is_etl,
        run: etl_import,
    },
    Target {
        name: "hxtrace",
        about: "trace index files",
        seeds: is_hxtrace,
        run: hxtrace,
    },
    Target {
        name: "manifest",
        about: "instrumentation manifests (WEVT_TEMPLATE) and payloads decoded by them",
        seeds: is_pe,
        run: manifest,
    },
    Target {
        name: "stacktags",
        about: "stack tag XML and folding rules",
        seeds: is_xml,
        run: stacktags,
    },
    Target {
        name: "legacy",
        about: "16-bit NE and linear (LE/LX) executables: every table, info, dump",
        seeds: is_legacy,
        run: legacy,
    },
    Target {
        name: "dwo",
        about: "split DWARF (.dwo files, .dwp packages), with their skeletons",
        seeds: is_split_dwarf,
        run: split_dwarf,
    },
    Target {
        name: "debugmap",
        about: "Mach-O objects of a debug map (no dSYM): their DWARF, addresses moved to the image's",
        seeds: is_macho,
        run: debug_map_object,
    },
    Target {
        name: "text",
        about: "link maps and linker logs",
        seeds: is_text,
        run: text,
    },
    Target {
        name: "demangle",
        about: "the MSVC, Itanium, Rust, and Swift demanglers",
        seeds: is_text,
        run: demangle,
    },
    Target {
        name: "x86",
        about: "the x86 decoder and formatter, in 16-, 32-, and 64-bit modes",
        seeds: is_code,
        run: x86,
    },
    Target {
        name: "tablegen",
        about: "the RISC-V, LoongArch, PowerPC, MIPS, and microMIPS decoders and formatters, and their code's flow",
        seeds: is_code,
        run: tablegen,
    },
    Target {
        name: "arm64",
        about: "the ARM64 decoder, formatter, and explainer",
        seeds: is_code,
        run: arm64,
    },
    Target {
        name: "arm32",
        about: "the AArch32 (A32 and T32) decoder and formatter",
        seeds: is_code,
        run: arm32,
    },
    Target {
        name: "query",
        about: "the query language's parser",
        seeds: is_any,
        run: query,
    },
    Target {
        name: "sourcemap",
        about: "source maps (JSON)",
        seeds: is_any,
        run: sourcemap,
    },
    Target {
        name: "der",
        about: "DER from signatures: certificates, CMS SignedData (checked), code requirements",
        seeds: is_der,
        run: der,
    },
    Target {
        name: "xz",
        about: "xz streams (ELF MiniDebugInfo's container) and their LZMA2 data",
        seeds: is_xz,
        run: xz,
    },
];

/// The target named `name`.
pub fn target(name: &str) -> Option<&'static Target> {
    if name == SELFTEST.name {
        return Some(&SELFTEST);
    }
    TARGETS.iter().find(|t| t.name == name)
}
