//! What a section, segment, or COFF group holds, in a few words: what a
//! report shows beside its name.
//!
//! The COFF groups (`.text$mn`, `.CRT$XCU`, ...) are the names the compiler
//! and the CRT give input sections, which the linker sorts by and merges
//! into one output section; SizeBench's "decoder ring" of them (MIT
//! License, Copyright (c) Microsoft Corporation) was the starting point
//! here, corrected (`.CRT$XCU` holds the C++ dynamic initializers' pointers,
//! say) and extended. The sections are every format's: PE, ELF, Mach-O
//! (`__SEGMENT,__section`), and WebAssembly (as bloaty names them: `Code`,
//! `Data`, ...).

use turbonop_image::Format;

/// What the section (or segment) `name` of a file of `format` holds, if it
/// is one whose purpose is known.
pub fn section(format: Format, name: &str) -> Option<&'static str> {
    match format {
        Format::Pe | Format::Coff => pe_section(name),
        Format::Elf => elf_section(name),
        Format::MachO => macho_section(name),
        Format::Wasm => wasm_section(name),
        _ => None,
    }
}

/// What the COFF group `name` (`.text$mn`) holds, if known.
pub fn group(name: &str) -> Option<&'static str> {
    if let Some(d) = exact_group(name) {
        return Some(d);
    }
    // Families named by a prefix (PGO's orderings, numbered phases).
    let prefixed: &[(&str, &str)] = &[
        (
            ".text$lp00",
            "the hottest code in PGO's training (loader phase 0)",
        ),
        (
            ".text$lp01",
            "code PGO found hot after phase 0 (loader phase 1)",
        ),
        (".text$lp", "code PGO found warm or hot (a loader phase)"),
        (
            ".data$dk",
            "variables PGO saw only read, but couldn't prove constant",
        ),
        (".data$zz", "read/write data PGO's training found dead"),
        (".rdata$zz", "read-only data PGO's training found dead"),
        (".bss$zz", "zero-initialized data PGO's training found dead"),
        (".rdata$zETW", "TraceLogging's event metadata"),
        (
            ".CRT$XC",
            "C++ dynamic initializers (pointers to them, run in name order)",
        ),
        (
            ".CRT$XI",
            "C initializers (pointers to them, run in name order)",
        ),
        (
            ".CRT$XL",
            "TLS callbacks (pointers the loader calls for each thread)",
        ),
        (
            ".CRT$XD",
            "dynamic TLS initializers (thread_local variables' constructors)",
        ),
        (
            ".CRT$XP",
            "pre-terminators (run at exit, before the terminators)",
        ),
        (".CRT$XT", "terminators (run at exit)"),
        (
            ".tls$",
            "the thread-local storage template (each thread's initial copy)",
        ),
        (".idata$", "the import table"),
        (".didat$", "the delay-load import table"),
        (".edata", "the export table"),
        (".gfids", "CFG: the functions that may be called indirectly"),
        (".giats", "CFG: the imports whose addresses are taken"),
        (".gljmp", "CFG: the targets longjmp may return to"),
        (".gehcont", "EH continuation targets (/guard:ehcont)"),
        (
            ".rsrc$01",
            "the resource directory: the tree of types, names, and languages",
        ),
        (".rsrc$02", "the resources' data"),
    ];
    for &(prefix, d) in prefixed {
        if name.starts_with(prefix) {
            return Some(d);
        }
    }
    // PGO's training scenarios, named as suffixes of `.text$`.
    if let Some(rest) = name.strip_prefix(".text$") {
        for (suffix, d) in [
            ("_pri7", "code run during PGO's Pri7 training"),
            ("_clientonly", "code PGO's training ran only on clients"),
            ("_serveronly", "code PGO's training ran only on servers"),
            ("_coldboot", "code run during cold boot in PGO's training"),
            (
                "_hybridboot",
                "code run during cold or hybrid boot in PGO's training",
            ),
        ] {
            if rest.ends_with(suffix) {
                return Some(d);
            }
        }
    }
    // A group named for its section alone: zero-fill data is its own kind;
    // otherwise it holds what the compiler put in the section without a
    // group of its own.
    match name {
        ".bss" => pe_section(name),
        _ => pe_section(name)
            .map(|_| "what the compiler put in the section without a group of its own"),
    }
}

fn exact_group(name: &str) -> Option<&'static str> {
    Some(match name {
        // Code.
        ".text$mn" => "ordinary code (not placed by PGO)",
        ".text$mn$00" | ".text$mn$01" => "ordinary code, in a subgroup of .text$mn",
        ".text$di" => "C++ dynamic initializers: global objects' constructors",
        ".text$yd" => "C++ dynamic atexit destructors: global objects' destructors",
        ".text$x" => "exception handling funclets (catch blocks, __finally, unwinding)",
        ".text$np" => {
            "code PGO can't optimize: compiled without optimization (#pragma optimize off), or assembly"
        }
        ".text$yz" => "blocks of functions PGO found cold",
        ".text$zy" => "blocks of functions PGO found dead",
        ".text$zs" => "functions PGO found rarely called",
        ".text$zz" => "whole functions PGO's training never ran",
        ".text$a" | ".text$z" => "markers bounding the section's code",
        // The CRT's lists: each is the pointers between an A and a Z marker.
        ".CRT$XCA" => "the first C++ initializer (a marker)",
        ".CRT$XCAA" => "the CRT's own startup C++ initializer",
        ".CRT$XCC" => "C++ initializers of #pragma init_seg(compiler)",
        ".CRT$XCL" => "C++ initializers of #pragma init_seg(lib)",
        ".CRT$XCU" => "C++ dynamic initializers: pointers to the code that constructs globals",
        ".CRT$XCZ" => "the last C++ initializer (a marker)",
        ".CRT$XIA" => "the first C initializer (a marker)",
        ".CRT$XIAA" => "the CRT's own startup C initializer",
        ".CRT$XIC" => "the CRT's C initializers",
        ".CRT$XIZ" => "the last C initializer (a marker)",
        ".CRT$XLA" => "the first TLS callback (a marker)",
        ".CRT$XLC" => "the CRT's TLS constructor callback",
        ".CRT$XLD" => "the CRT's TLS destructor callback",
        ".CRT$XLZ" => "the last TLS callback (a marker)",
        ".CRT$XDA" => "the first dynamic TLS initializer (a marker)",
        ".CRT$XDZ" => "the last dynamic TLS initializer (a marker)",
        ".CRT$XPA" => "the first pre-terminator (a marker)",
        ".CRT$XPB" => "the Concurrency Runtime's pre-terminator",
        ".CRT$XPX" => "the CRT's pre-terminators",
        ".CRT$XPXA" => "the CRT's stdio pre-terminator (flushes streams)",
        ".CRT$XPZ" => "the last pre-terminator (a marker)",
        ".CRT$XTA" => "the first terminator (a marker)",
        ".CRT$XTZ" => "the last terminator (a marker)",
        ".CRTMA$XCA" => "the first managed C++ initializer (a marker)",
        ".CRTMA$XCZ" => "the last managed C++ initializer (a marker)",
        ".CRTVT$XCA" => "the first managed C++ vtable initializer (a marker)",
        ".CRTVT$XCZ" => "the last managed C++ vtable initializer (a marker)",
        ".rtc$IAA" => "the first run-time check initializer (/RTC; a marker)",
        ".rtc$IZZ" => "the last run-time check initializer (/RTC; a marker)",
        ".rtc$TAA" => "the first run-time check terminator (/RTC; a marker)",
        ".rtc$TZZ" => "the last run-time check terminator (/RTC; a marker)",
        // Read-only data.
        ".rdata$r" => "RTTI: type descriptors and class hierarchies (for dynamic_cast, typeid, EH)",
        ".rdata$brc" => "base relocations clustered for the loader",
        ".rdata$sxdata" => "SafeSEH's table of exception handlers (x86)",
        ".rdata$voltmd" => "volatile metadata (/volatileMetadata), for emulators' memory ordering",
        ".rdata$zzzdbg" => "the debug directory's data (the PDB's name and signature, ...)",
        ".rdata$wbrd" => "Warbird obfuscation's read-only data",
        ".rdata$00" | ".rdata$01" => "read-only data ordered by PGO's Pri7 phases",
        ".rdata$T" => "the thread-local storage directory's data",
        ".rdata$zETW0" => "TraceLogging's fixed header",
        ".rdata$zETW1" => "TraceLogging: one entry per TraceLoggingWrite",
        ".rdata$zETW2" => "TraceLogging: one entry per TRACELOGGING_DEFINE_PROVIDER",
        ".rdata$zETW9" => "TraceLogging's fixed footer",
        // Read/write data.
        ".data$r" => "RTTI that is written to (type descriptors' cached names)",
        ".data$d" => "RTTI's read/write data",
        ".data$pr" => "variables PGO's training only read",
        ".data$00" | ".data$01" => "read/write data ordered by PGO's Pri7 phases",
        // Imports.
        ".idata$2" => "import descriptors: one per imported DLL",
        ".idata$3" => "the import descriptors' null terminator",
        ".idata$4" => "the import name table (INT): what each import is",
        ".idata$5" => {
            "the import address table (IAT): where the loader writes each import's address"
        }
        ".idata$6" => "imported functions' names (hint/name entries) and DLL names",
        ".didat$2" => "delay-load import descriptors",
        ".didat$3" => "the delay-load descriptors' null terminator",
        ".didat$4" => "the delay-load import name table",
        ".didat$5" => "the delay-load import address table",
        ".didat$6" => "delay-loaded functions' names",
        ".didat$7" => "delay-loaded DLLs' module handles",
        // Exception data.
        ".pdata" => "function tables (RUNTIME_FUNCTION): where each function's unwind data is",
        ".xdata" => "unwind data: how to unwind each function's frame",
        ".xdata$x" => "C++ exception handling data (FuncInfo, unwind maps, try maps)",
        // Thread-local storage.
        ".tls" => "the thread-local storage template's start (a marker)",
        ".tls$ZZZ" => "the thread-local storage template's end (a marker)",
        // Control Flow Guard.
        ".gfids" => "CFG: the functions that may be called indirectly",
        ".giats" => "CFG: the imports whose addresses are taken",
        ".gljmp" => "CFG: the targets longjmp may return to",
        ".gehcont" => "EH continuation targets (/guard:ehcont)",
        ".00cfg" => "CFG's check and dispatch function pointers",
        _ => return None,
    })
}

fn pe_section(name: &str) -> Option<&'static str> {
    Some(match name {
        ".text" => "code",
        ".textbss" => "Edit and Continue's space for code (incremental linking)",
        ".rdata" => "read-only data: constants, strings, vtables, import and debug tables",
        ".data" => "read/write data (and, after its file bytes, zero-initialized data)",
        ".bss" => "zero-initialized data (no bytes in the file)",
        ".idata" => "the import table",
        ".edata" => "the export table",
        ".didat" => "the delay-load import address table",
        ".pdata" => "function tables: where each function's unwind data is",
        ".xdata" => "unwind and exception handling data",
        ".reloc" => "base relocations: what the loader patches if the image moves",
        ".rsrc" => "Win32 resources: icons, dialogs, strings, the version, the manifest",
        ".tls" => "the thread-local storage template",
        ".CRT" => "the CRT's lists of initializers, terminators, and TLS callbacks",
        ".gfids" => "CFG: the functions that may be called indirectly",
        ".00cfg" => "CFG's check and dispatch function pointers",
        ".retplne" => "retpoline metadata (/guard:retpoline)",
        ".voltbl" => "volatile metadata",
        ".fptable" => "a table of function pointers (/guard:cf on ARM64EC)",
        ".msvcjmc" => "Just My Code's per-file flags (/JMC, for the debugger)",
        ".hexpthk" => "ARM64EC's x64 entry thunks",
        ".a64xrm" => "ARM64X's relocations",
        ".sxdata" => "SafeSEH's table of exception handlers (x86)",
        ".debug" => "debug information (an object's, or an old image's)",
        _ => {
            // Objects' sections: debug information and directives.
            return if name.starts_with(".debug$") {
                Some("CodeView debug information")
            } else if name == ".drectve" {
                Some("linker directives (/DEFAULTLIB, /EXPORT, ...)")
            } else {
                None
            };
        }
    })
}

fn elf_section(name: &str) -> Option<&'static str> {
    Some(match name {
        ".text" => "code",
        ".init" => "code run when the image is loaded, before its initializers (DT_INIT)",
        ".fini" => "code run when it is unloaded (DT_FINI)",
        ".plt" => "the procedure linkage table: stubs that call other images' functions",
        ".plt.got" | ".plt.sec" => "procedure linkage stubs (with -z now or IBT)",
        ".iplt" => "stubs for IFUNC functions (chosen at load time)",
        ".rodata" => "read-only data: constants and strings",
        ".data" => "read/write data",
        ".data.rel.ro" => "data the loader relocates, then makes read-only (RELRO)",
        ".bss" => "zero-initialized data (no bytes in the file)",
        ".tdata" => "thread-local storage's initialized data",
        ".tbss" => "thread-local storage's zero-initialized data",
        ".eh_frame" => "unwind information (for exceptions, backtraces, and debuggers)",
        ".eh_frame_hdr" => "the index of .eh_frame, by address",
        ".gcc_except_table" => "C++ exception handling's tables (call sites, actions, types)",
        ".sframe" => "SFrame unwind information (simple stack traces)",
        ".init_array" => "pointers to initializers (C++ global constructors, ...)",
        ".fini_array" => "pointers to finalizers (run at exit)",
        ".preinit_array" => "pointers to the program's pre-initializers",
        ".ctors" => "pointers to initializers (the old layout)",
        ".dtors" => "pointers to finalizers (the old layout)",
        ".got" => "the global offset table: addresses the loader fills in",
        ".got.plt" => "the PLT's part of the global offset table",
        ".dynamic" => "the dynamic table: what the loader needs (libraries, tables)",
        ".dynsym" => "the dynamic symbols: what the image exports and imports",
        ".dynstr" => "the dynamic symbols' names",
        ".hash" => "the dynamic symbols' hash table (SysV)",
        ".gnu.hash" => "the dynamic symbols' hash table (GNU)",
        ".gnu.version" => "each dynamic symbol's version",
        ".gnu.version_r" => "the symbol versions the image needs",
        ".gnu.version_d" => "the symbol versions the image defines",
        ".rela.dyn" | ".rel.dyn" => "dynamic relocations: what the loader patches",
        ".rela.plt" | ".rel.plt" => "the PLT's relocations: imports bound lazily or at load",
        ".relr.dyn" => "relative relocations, packed (RELR)",
        ".interp" => "the program interpreter (the dynamic loader's path)",
        ".note.gnu.build-id" => "the build ID: what identifies this build",
        ".note.ABI-tag" => "the ABI the image is for",
        ".note.gnu.property" => "properties (CET's IBT and shadow stacks, ...)",
        ".note.package" => "the package the image came from",
        ".note.stapsdt" => "SystemTap's probes",
        ".stapsdt.base" => "SystemTap's base address",
        ".comment" => "the compilers' and linkers' versions",
        ".symtab" => "the symbol table (removed by strip)",
        ".strtab" => "the symbol table's names (removed by strip)",
        ".shstrtab" => "the section names",
        ".gnu_debuglink" => "the separate debug file's name and checksum",
        ".gnu_debugdata" => "MiniDebugInfo: a compressed symbol table for backtraces",
        ".ARM.exidx" => "32-bit ARM's exception index: each function's unwind entry",
        ".ARM.extab" => "32-bit ARM's exception tables",
        ".ARM.attributes" => "32-bit ARM's build attributes",
        ".riscv.attributes" => "RISC-V's build attributes",
        ".tm_clone_table" => "transactional memory's clone table",
        ".llvm_addrsig" => "the symbols whose addresses are taken (for ICF; objects)",
        ".group" => "a COMDAT group's members (objects)",
        _ => {
            return if name.starts_with(".debug_") || name.starts_with(".zdebug_") {
                Some("DWARF debug information")
            } else if name.starts_with(".rela.") || name.starts_with(".rel.") {
                Some("relocations (objects): what the linker patches")
            } else if name.starts_with(".note") {
                Some("a note: metadata for tools and the loader")
            } else if name.starts_with(".rodata.") {
                Some("read-only data: constants and strings")
            } else if name.starts_with(".text.") {
                Some("code (a function's own section: -ffunction-sections)")
            } else if name.starts_with(".data.") {
                Some("read/write data (a variable's own section)")
            } else if name.starts_with(".bss.") {
                Some("zero-initialized data (a variable's own section)")
            } else {
                None
            };
        }
    })
}

fn macho_section(name: &str) -> Option<&'static str> {
    Some(match name {
        "__PAGEZERO" => "the unmapped first page (catches null pointers; no bytes)",
        "__TEXT" => "the read-only, executable segment: code and constants",
        "__DATA" => "the read/write segment",
        "__DATA_CONST" => "data the loader fixes up, then makes read-only",
        "__DATA_DIRTY" => "read/write data that is usually written to",
        "__AUTH" | "__AUTH_CONST" => "arm64e's signed pointers",
        "__LINKEDIT" => "what the loader and tools read: symbols, fixups, signatures",
        "__TEXT,__text" => "code",
        "__TEXT,__stubs" | "__TEXT,__auth_stubs" => "stubs that call other images' functions",
        "__TEXT,__stub_helper" => "the helper of lazily bound stubs",
        "__TEXT,__cstring" => "C string literals",
        "__TEXT,__const" => "read-only constants",
        "__TEXT,__literal4" | "__TEXT,__literal8" | "__TEXT,__literal16" => {
            "fixed-size constants, merged"
        }
        "__TEXT,__ustring" => "UTF-16 string literals",
        "__TEXT,__unwind_info" => "compact unwind information: how to unwind each function",
        "__TEXT,__eh_frame" => "DWARF unwind information, where compact unwind can't say",
        "__TEXT,__gcc_except_tab" => "C++ exception handling's tables",
        "__TEXT,__init_offsets" => "the initializers' offsets (C++ global constructors, ...)",
        "__TEXT,__oslogstring" => "os_log's format strings",
        "__TEXT,__objc_methname" => "Objective-C's method names (selectors)",
        "__TEXT,__objc_classname" => "Objective-C's class names",
        "__TEXT,__objc_methtype" => "Objective-C's method type encodings",
        "__TEXT,__swift5_typeref" => "Swift's type references (mangled names)",
        "__TEXT,__swift5_reflstr" => "Swift's reflection strings (field names)",
        "__TEXT,__swift5_fieldmd" => "Swift's field metadata",
        "__TEXT,__swift5_types" => "Swift's type metadata records",
        "__TEXT,__swift5_protos" => "Swift's protocol descriptors",
        "__TEXT,__swift5_proto" => "Swift's protocol conformances",
        "__TEXT,__swift5_capture" => "Swift's closure capture descriptors",
        "__TEXT,__swift5_assocty" => "Swift's associated type descriptors",
        "__TEXT,__swift5_builtin" => "Swift's builtin type descriptors",
        "__TEXT,__swift5_entry" => "Swift's entry point",
        "__DATA_CONST,__got" | "__DATA,__got" | "__DATA_CONST,__auth_got" => {
            "the global offset table: addresses of other images' symbols"
        }
        "__DATA_CONST,__const" | "__DATA,__const" => "constants the loader fixes up",
        "__DATA_CONST,__mod_init_func" | "__DATA,__mod_init_func" => "pointers to initializers",
        "__DATA_CONST,__mod_term_func" | "__DATA,__mod_term_func" => "pointers to terminators",
        "__DATA_CONST,__cfstring" | "__DATA,__cfstring" => "CoreFoundation string constants",
        "__DATA,__data" => "read/write data",
        "__DATA,__la_symbol_ptr" => "pointers lazily bound to other images' functions",
        "__DATA,__nl_symbol_ptr" => "pointers bound at load to other images' symbols",
        "__DATA,__bss" => "zero-initialized data (no bytes in the file)",
        "__DATA,__common" => "uninitialized globals (common symbols; no bytes in the file)",
        "__DATA,__thread_vars" => "thread-local variables' descriptors",
        "__DATA,__thread_data" => "thread-local storage's initialized data",
        "__DATA,__thread_bss" => "thread-local storage's zero-initialized data",
        _ => {
            let section = name.split_once(',').map_or(name, |(_, s)| s);
            return if section.starts_with("__objc_") {
                Some("Objective-C's runtime metadata")
            } else if section.starts_with("__swift5_") {
                Some("Swift's runtime metadata")
            } else if name.starts_with("__DWARF,") {
                Some("DWARF debug information (objects)")
            } else {
                None
            };
        }
    })
}

fn wasm_section(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "type" => "the function signatures",
        "import" => "what the module imports: functions, memories, tables, globals",
        "function" => "each function's signature (by index)",
        "table" => "the tables (of function references, for indirect calls)",
        "memory" => "the linear memories' sizes",
        "global" => "the globals and their initial values",
        "export" => "what the module exports",
        "start" => "the function run when the module is instantiated",
        "elem" | "element" => "the tables' initial contents (function references)",
        "datacount" => "the number of data segments",
        "code" => "code: each function's body",
        "data" => "data segments: linear memory's initial contents",
        "tag" => "exception tags",
        "name" => "the names of functions, locals, and more (for debuggers and profilers)",
        "producers" => "the languages and tools that produced the module",
        "target_features" => "the WebAssembly features the module uses",
        "sourcemappingurl" => "where the module's source map is",
        "linking" => "the linker's metadata (objects)",
        "dylink.0" | "dylink" => "dynamic linking's metadata",
        other => {
            return if other.starts_with(".debug_") {
                Some("DWARF debug information")
            } else if other.starts_with("reloc.") {
                Some("relocations (objects)")
            } else {
                // Data segments named like ELF sections (`.rodata`).
                elf_section(name)
            };
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_name_and_family() {
        assert_eq!(group(".text$mn"), Some("ordinary code (not placed by PGO)"));
        assert!(group(".CRT$XCU").unwrap().contains("dynamic initializers"));
        // Families.
        assert!(group(".text$lp00foo").unwrap().contains("hottest"));
        assert!(group(".text$lp07").unwrap().contains("loader phase"));
        assert!(group(".rdata$zETW5").unwrap().contains("TraceLogging"));
        assert!(group(".text$abc_coldboot").unwrap().contains("cold boot"));
        assert!(group(".idata$7").unwrap().contains("import"));
        // A section's own name, as a group.
        assert!(group(".rdata").unwrap().contains("without a group"));
        assert!(group(".bss").unwrap().contains("zero-initialized"));
        assert!(group(".gfids$y").unwrap().contains("CFG"));
        assert!(group(".rsrc$01").unwrap().contains("resource directory"));
        assert_eq!(group(".weird$q"), None);
    }

    #[test]
    fn sections_by_format() {
        assert_eq!(section(Format::Pe, ".text"), Some("code"));
        assert!(section(Format::Pe, ".pdata").is_some());
        assert_eq!(section(Format::Elf, ".pdata"), None);
        assert!(
            section(Format::Elf, ".eh_frame")
                .unwrap()
                .contains("unwind")
        );
        assert!(
            section(Format::Elf, ".debug_info")
                .unwrap()
                .contains("DWARF")
        );
        assert!(
            section(Format::Elf, ".text.hot.f")
                .unwrap()
                .contains("code")
        );
        assert!(
            section(Format::MachO, "__TEXT,__unwind_info")
                .unwrap()
                .contains("unwind")
        );
        assert!(
            section(Format::MachO, "__DATA_CONST,__objc_classlist")
                .unwrap()
                .contains("Objective-C")
        );
        assert!(section(Format::MachO, "__LINKEDIT").is_some());
        assert_eq!(
            section(Format::Wasm, "Code"),
            Some("code: each function's body")
        );
        assert!(
            section(Format::Wasm, ".rodata")
                .unwrap()
                .contains("read-only")
        );
        assert!(
            section(Format::Wasm, ".debug_line")
                .unwrap()
                .contains("DWARF")
        );
    }
}
