//! The image's tables, compared by what their entries name: imports,
//! exports, initializers, TLS callbacks, the load configuration, the
//! `/guard` tables, and the unwind data, function by function.

use turbonop_core::Rva;
use turbonop_pe::exception::RuntimeFunction;
use turbonop_pe::exports::ExportTarget;
use turbonop_pe::imports::ImportName;
use turbonop_pe::load_config::field;

use super::bytes::{self, Isa, Side};
use super::program::Linked;
use crate::report::{Report, show};

/// A name as text.
fn named(n: Option<(&[u8], u32)>) -> String {
    match n {
        Some((name, 0)) => show(name),
        Some((name, off)) => format!("{}+{off:#x}", show(name)),
        None => "nothing".into(),
    }
}

/// What a virtual address names, as text (an address outside the image is
/// its value).
fn va_name(p: &Linked<'_>, va: u64) -> String {
    if va == 0 {
        return "0".into();
    }
    match p.name_va(va) {
        Some(n) => named(Some(n)),
        None => format!("{va:#x}"),
    }
}

fn rva_name(p: &Linked<'_>, rva: u32) -> String {
    named(p.name(rva))
}

/// Compares two lists, reporting the first difference: entries only in
/// one (as a failure), or the same entries in another order (`order`'s
/// severity: a failure if `order_fails`, otherwise expected).
fn lists(a: &[String], b: &[String], what: &str, order_fails: bool, rep: &mut Report) {
    rep.checked(what, a.len() as u64);
    if a == b {
        return;
    }
    let mut sa = a.to_vec();
    let mut sb = b.to_vec();
    sa.sort();
    sb.sort();
    if sa == sb {
        let category = format!("{what}: the same, in another order");
        let i = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(0);
        let example = || format!("entry {i}: {} and {}", a[i], b[i]);
        if order_fails {
            rep.fail(&category, example);
        } else {
            rep.expect(&category, example);
        }
        return;
    }
    let only = |x: &[String], y: &[String]| -> Vec<String> {
        let mut y: Vec<&String> = y.iter().collect();
        y.sort();
        x.iter()
            .filter(|e| y.binary_search(e).is_err())
            .cloned()
            .collect()
    };
    for e in only(a, b) {
        rep.fail(&format!("{what} only in the first"), || e);
    }
    for e in only(b, a) {
        rep.fail(&format!("{what} only in the second"), || e);
    }
    if only(a, b).is_empty() && only(b, a).is_empty() {
        rep.fail(&format!("{what} that differ in number"), || {
            format!("{} and {} entries", a.len(), b.len())
        });
    }
}

/// Imports and delay imports: each DLL's functions, in order.
pub(crate) fn imports(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    let name = |i: &ImportName<'_>| match i {
        ImportName::Name { name, .. } => show(name),
        ImportName::Ordinal(o) => format!("#{o}"),
    };
    let read = |p: &Linked<'_>| -> (Vec<String>, Vec<String>) {
        let mut normal = Vec::new();
        for dll in p.pe.imports() {
            let d = show(dll.name).to_ascii_lowercase();
            normal.extend(dll.thunks().map(|t| format!("{d}!{}", name(&t.import))));
        }
        let mut delay = Vec::new();
        for dll in p.pe.delay_imports() {
            let d = show(dll.name).to_ascii_lowercase();
            delay.extend(dll.thunks().map(|t| format!("{d}!{}", name(&t.import))));
        }
        (normal, delay)
    };
    let (na, da) = read(a);
    let (nb, db) = read(b);
    lists(&na, &nb, "imports", false, rep);
    lists(&da, &db, "delay imports", false, rep);
}

/// Exports: each one's ordinal, name, and what it names.
pub(crate) fn exports(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    let read = |p: &Linked<'_>| -> Vec<String> {
        let Some(e) = p.pe.exports() else {
            return Vec::new();
        };
        e.iter()
            .map(|x| {
                let target = match x.target {
                    ExportTarget::Rva(r) => rva_name(p, r.get()),
                    ExportTarget::Forwarder(f) => format!("forwarded to {}", show(f)),
                };
                format!(
                    "#{} {}: {target}",
                    x.ordinal,
                    x.name.map_or_else(|| "(no name)".into(), show)
                )
            })
            .collect()
    };
    lists(&read(a), &read(b), "exports", true, rep);
}

/// The pointers from the label `first` to the label `last` (exclusive),
/// zeros left out, as what each names.
fn table(p: &Linked<'_>, first: &[u8], last: &[u8]) -> Option<Vec<String>> {
    let find = |n: &[u8]| {
        p.labels
            .iter()
            .find(|l| *l.name == *n && !l.renamed)
            .map(|l| l.rva)
    };
    let (start, end) = (find(first)?, find(last)?);
    let width = if p
        .pe
        .optional_header()
        .is_some_and(turbonop_pe::OptionalHeader::is_64)
    {
        8
    } else {
        4
    };
    let bytes = p.bytes(start, end.checked_sub(start)?);
    Some(
        bytes
            .chunks_exact(width)
            .map(|c| {
                let mut v = [0u8; 8];
                v[..width].copy_from_slice(c);
                u64::from_le_bytes(v)
            })
            .filter(|&v| v != 0)
            .map(|v| va_name(p, v))
            .collect(),
    )
}

/// The C runtime's tables of initializers and terminators (`.CRT$XC*`,
/// `.CRT$XI*`, `.CRT$XP*`, `.CRT$XT*`, between the symbols that bound
/// them) and the TLS callbacks, in order.
pub(crate) fn initializers(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    for (first, last, what) in [
        (
            &b"__xc_a"[..],
            &b"__xc_z"[..],
            "C++ initializers (.CRT$XC*)",
        ),
        (b"__xi_a", b"__xi_z", "C initializers (.CRT$XI*)"),
        (b"__xp_a", b"__xp_z", "pre-terminators (.CRT$XP*)"),
        (b"__xt_a", b"__xt_z", "terminators (.CRT$XT*)"),
    ] {
        match (table(a, first, last), table(b, first, last)) {
            (Some(x), Some(y)) => lists(&x, &y, what, true, rep),
            (None, None) => {
                rep.unchecked("initializer tables without their bounding symbols", || {
                    format!(
                        "{what}: neither program has {} and {}",
                        show(first),
                        show(last)
                    )
                });
            }
            _ => rep.fail("initializer tables in only one program", || {
                what.to_string()
            }),
        }
    }
    let callbacks = |p: &Linked<'_>| -> Vec<String> {
        p.pe.tls_callbacks().map(|v| va_name(p, v.get())).collect()
    };
    lists(&callbacks(a), &callbacks(b), "TLS callbacks", true, rep);
}

/// Every load configuration field: equal, or (pointer-sized fields) both
/// naming the same thing.
const LOAD_CONFIG: &[(&str, turbonop_pe::load_config::Field)] = &[
    ("Size", field::SIZE),
    ("TimeDateStamp", field::TIME_DATE_STAMP),
    ("MajorVersion", field::MAJOR_VERSION),
    ("MinorVersion", field::MINOR_VERSION),
    ("GlobalFlagsClear", field::GLOBAL_FLAGS_CLEAR),
    ("GlobalFlagsSet", field::GLOBAL_FLAGS_SET),
    (
        "CriticalSectionDefaultTimeout",
        field::CRITICAL_SECTION_DEFAULT_TIMEOUT,
    ),
    (
        "DeCommitFreeBlockThreshold",
        field::DE_COMMIT_FREE_BLOCK_THRESHOLD,
    ),
    (
        "DeCommitTotalFreeThreshold",
        field::DE_COMMIT_TOTAL_FREE_THRESHOLD,
    ),
    ("LockPrefixTable", field::LOCK_PREFIX_TABLE),
    ("MaximumAllocationSize", field::MAXIMUM_ALLOCATION_SIZE),
    ("VirtualMemoryThreshold", field::VIRTUAL_MEMORY_THRESHOLD),
    ("ProcessHeapFlags", field::PROCESS_HEAP_FLAGS),
    ("ProcessAffinityMask", field::PROCESS_AFFINITY_MASK),
    ("CSDVersion", field::CSD_VERSION),
    ("DependentLoadFlags", field::DEPENDENT_LOAD_FLAGS),
    ("EditList", field::EDIT_LIST),
    ("SecurityCookie", field::SECURITY_COOKIE),
    ("SEHandlerTable", field::SE_HANDLER_TABLE),
    ("SEHandlerCount", field::SE_HANDLER_COUNT),
    (
        "GuardCFCheckFunctionPointer",
        field::GUARD_CF_CHECK_FUNCTION_POINTER,
    ),
    (
        "GuardCFDispatchFunctionPointer",
        field::GUARD_CF_DISPATCH_FUNCTION_POINTER,
    ),
    ("GuardCFFunctionTable", field::GUARD_CF_FUNCTION_TABLE),
    ("GuardCFFunctionCount", field::GUARD_CF_FUNCTION_COUNT),
    ("GuardFlags", field::GUARD_FLAGS),
    ("CodeIntegrityFlags", field::CODE_INTEGRITY_FLAGS),
    ("CodeIntegrityCatalog", field::CODE_INTEGRITY_CATALOG),
    (
        "CodeIntegrityCatalogOffset",
        field::CODE_INTEGRITY_CATALOG_OFFSET,
    ),
    (
        "GuardAddressTakenIatEntryTable",
        field::GUARD_ADDRESS_TAKEN_IAT_ENTRY_TABLE,
    ),
    (
        "GuardAddressTakenIatEntryCount",
        field::GUARD_ADDRESS_TAKEN_IAT_ENTRY_COUNT,
    ),
    (
        "GuardLongJumpTargetTable",
        field::GUARD_LONG_JUMP_TARGET_TABLE,
    ),
    (
        "GuardLongJumpTargetCount",
        field::GUARD_LONG_JUMP_TARGET_COUNT,
    ),
    ("DynamicValueRelocTable", field::DYNAMIC_VALUE_RELOC_TABLE),
    ("CHPEMetadataPointer", field::CHPE_METADATA_POINTER),
    ("GuardRFFailureRoutine", field::GUARD_RF_FAILURE_ROUTINE),
    (
        "GuardRFFailureRoutineFunctionPointer",
        field::GUARD_RF_FAILURE_ROUTINE_FUNCTION_POINTER,
    ),
    (
        "DynamicValueRelocTableOffset",
        field::DYNAMIC_VALUE_RELOC_TABLE_OFFSET,
    ),
    (
        "DynamicValueRelocTableSection",
        field::DYNAMIC_VALUE_RELOC_TABLE_SECTION,
    ),
    (
        "GuardRFVerifyStackPointerFunctionPointer",
        field::GUARD_RF_VERIFY_STACK_POINTER_FUNCTION_POINTER,
    ),
    ("HotPatchTableOffset", field::HOT_PATCH_TABLE_OFFSET),
    (
        "EnclaveConfigurationPointer",
        field::ENCLAVE_CONFIGURATION_POINTER,
    ),
    ("VolatileMetadataPointer", field::VOLATILE_METADATA_POINTER),
    (
        "GuardEHContinuationTable",
        field::GUARD_EH_CONTINUATION_TABLE,
    ),
    (
        "GuardEHContinuationCount",
        field::GUARD_EH_CONTINUATION_COUNT,
    ),
    (
        "GuardXFGCheckFunctionPointer",
        field::GUARD_XFG_CHECK_FUNCTION_POINTER,
    ),
    (
        "GuardXFGDispatchFunctionPointer",
        field::GUARD_XFG_DISPATCH_FUNCTION_POINTER,
    ),
    (
        "GuardXFGTableDispatchFunctionPointer",
        field::GUARD_XFG_TABLE_DISPATCH_FUNCTION_POINTER,
    ),
    (
        "CastGuardOsDeterminedFailureMode",
        field::CAST_GUARD_OS_DETERMINED_FAILURE_MODE,
    ),
    (
        "GuardMemcpyFunctionPointer",
        field::GUARD_MEMCPY_FUNCTION_POINTER,
    ),
    ("UmaFunctionPointers", field::UMA_FUNCTION_POINTERS),
];

/// The load configuration, field by field, and the `/guard` tables (CFG
/// functions, address-taken IAT entries, long jump targets, EH
/// continuations) and SafeSEH handlers, as sets of what they name.
pub(crate) fn load_config(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    let (ca, cb) = match (a.pe.load_config(), b.pe.load_config()) {
        (Some(x), Some(y)) => (x, y),
        (None, None) => return,
        _ => {
            rep.fail("load configurations in only one program", || "".into());
            return;
        }
    };
    for &(name, f) in LOAD_CONFIG {
        let (va, vb) = (ca.get(f), cb.get(f));
        rep.checked("load configuration fields", u64::from(va.is_some()));
        if va == vb {
            continue;
        }
        let pointer = f.2 == 0;
        let same = pointer
            && match (va, vb) {
                (Some(x), Some(y)) => {
                    let (nx, ny) = (a.name_va(x), b.name_va(y));
                    nx.is_some() && nx == ny
                }
                _ => false,
            };
        if !same {
            rep.fail("load configuration fields that differ", || {
                let show_v = |p: &Linked<'_>, v: Option<u64>| match v {
                    Some(v) if pointer => va_name(p, v),
                    Some(v) => format!("{v:#x}"),
                    None => "absent".into(),
                };
                format!("{name}: {} and {}", show_v(a, va), show_v(b, vb))
            });
        }
    }
    let guard = |c: &turbonop_pe::LoadConfig<'_>, p: &Linked<'_>, which: usize| -> Vec<String> {
        let t = match which {
            0 => c.guard_cf_functions(),
            1 => c.guard_address_taken_iat_entries(),
            2 => c.guard_long_jump_targets(),
            _ => c.guard_eh_continuations(),
        };
        let mut v: Vec<String> = t
            .iter()
            .map(|e| format!("{} (flags {:#x})", rva_name(p, e.rva.get()), e.flags))
            .collect();
        v.sort();
        v
    };
    for (which, what) in [
        "CFG function table entries",
        "CFG address-taken IAT entries",
        "CFG long jump targets",
        "EH continuation targets",
    ]
    .iter()
    .enumerate()
    {
        lists(
            &guard(&ca, a, which),
            &guard(&cb, b, which),
            what,
            true,
            rep,
        );
    }
    let seh = |c: &turbonop_pe::LoadConfig<'_>, p: &Linked<'_>| -> Vec<String> {
        let mut v: Vec<String> = c
            .safe_seh_handlers()
            .iter()
            .map(|r| rva_name(p, r.get()))
            .collect();
        v.sort();
        v
    };
    lists(&seh(&ca, a), &seh(&cb, b), "SafeSEH handlers", true, rep);
}

/// One function's unwind data, comparable across images: what its begin
/// and end name, and its unwind information's region.
struct Unwind {
    begin: String,
    end: String,
    /// The unwind information (x64 `UNWIND_INFO` or ARM64 `.xdata`): its
    /// RVA and size, or (ARM64 packed) the word itself.
    info: Result<(u32, u32), u32>,
}

/// The regions of unwind information `.pdata` names (each compared with
/// its function's): sorted, those that touch merged.
pub(crate) fn unwind_ranges(p: &Linked<'_>) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = unwinds(p)
        .into_iter()
        .filter_map(|u| u.info.ok())
        .map(|(rva, size)| (rva, rva.saturating_add(size)))
        .collect();
    v.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(v.len());
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Whether `[rva, rva + len)` is wholly unwind information (see
/// [`unwind_ranges`]).
pub(crate) fn covered(ranges: &[(u32, u32)], rva: u32, len: u32) -> bool {
    let i = ranges.partition_point(|r| r.0 <= rva);
    i.checked_sub(1)
        .is_some_and(|i| rva.saturating_add(len) <= ranges[i].1 && len > 0)
}

fn unwinds(p: &Linked<'_>) -> Vec<Unwind> {
    p.pe.runtime_functions()
        .filter_map(|e| match e.function {
            RuntimeFunction::X64 { begin, end, unwind } => {
                let info = p.pe.x64_unwind_info(unwind)?;
                // The record, and the handler's first word of data (C++'s
                // handlers keep an RVA there).
                let size = info.size() + if info.handler().is_some() { 4 } else { 0 };
                Some(Unwind {
                    begin: rva_name(p, begin.get()),
                    end: named(p.name(end.get().wrapping_sub(1)).map(|(n, o)| (n, o + 1))),
                    info: Ok((unwind.get(), size)),
                })
            }
            RuntimeFunction::Arm { begin, unwind } => {
                let info = if unwind & 3 != 0 {
                    Err(unwind)
                } else {
                    let (size, _) =
                        turbonop_pe::exception::arm_xdata_extent(&p.pe, Rva(unwind), false)?;
                    Ok((unwind, size))
                };
                Some(Unwind {
                    begin: rva_name(p, begin.get()),
                    end: String::new(),
                    info,
                })
            }
        })
        .collect()
}

/// The unwind data, function by function: each `.pdata` entry, matched by
/// what its function's start names, with its function's extent and its
/// unwind information compared as bytes (handlers and chained entries by
/// what they name).
pub(crate) fn unwind(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    let ua = unwinds(a);
    let ub = unwinds(b);
    rep.checked("unwind entries", ua.len() as u64);
    let mut by_begin: turbonop_hash::Map<&str, Vec<&Unwind>> = turbonop_hash::Map::default();
    for u in &ub {
        by_begin.entry(u.begin.as_str()).or_default().push(u);
    }
    let mut matched: turbonop_hash::Map<&str, usize> = turbonop_hash::Map::default();
    for u in &ua {
        let n = matched.entry(u.begin.as_str()).or_insert(0);
        let Some(v) = by_begin.get(u.begin.as_str()).and_then(|v| v.get(*n)) else {
            rep.fail("unwind entries only in the first program", || {
                u.begin.clone()
            });
            continue;
        };
        *n += 1;
        if u.end != v.end {
            rep.fail("unwind entries for functions of another extent", || {
                format!("{}: ends at {} and {}", u.begin, u.end, v.end)
            });
        }
        match (u.info, v.info) {
            (Ok((ra, sa)), Ok((rb, sb))) => {
                if sa != sb {
                    rep.fail("unwind information of another size", || u.begin.clone());
                    continue;
                }
                let (xa, xb) = (a.bytes(ra, sa), b.bytes(rb, sb));
                let mut sub = Report::new();
                bytes::compare(
                    &Side {
                        image: a,
                        rva: ra,
                        bytes: &xa,
                    },
                    &Side {
                        image: b,
                        rva: rb,
                        bytes: &xb,
                    },
                    Isa::Data,
                    format!("unwind information of {}", u.begin).as_bytes(),
                    &mut sub,
                );
                if !sub.passed() {
                    rep.fail("unwind information that differs", || u.begin.clone());
                }
            }
            (Err(wa), Err(wb)) if wa == wb => {}
            _ => rep.fail("unwind information that differs", || u.begin.clone()),
        }
    }
    let mut counts: turbonop_hash::Map<&str, isize> = turbonop_hash::Map::default();
    for u in &ua {
        *counts.entry(u.begin.as_str()).or_insert(0) += 1;
    }
    for u in &ub {
        let c = counts.entry(u.begin.as_str()).or_insert(0);
        *c -= 1;
        if *c < 0 {
            rep.fail("unwind entries only in the second program", || {
                u.begin.clone()
            });
        }
    }
}
