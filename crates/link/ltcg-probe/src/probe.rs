//! The probe: proxies for every interface pointer passed between the
//! linker and the back end, each call through one logged and forwarded.
//!
//! # How a call is forwarded
//!
//! Every proxy shares one table of generic slots (`thunk::<N>`), each taking
//! `this` and twelve integer arguments and calling the real object's slot N
//! with the same twelve. On x64 Windows that is exact for these interfaces:
//! every parameter of every method is an integer or a pointer (no floating
//! point, no structure by value), so the first four travel in RCX, RDX, R8,
//! R9 and the rest on the stack, where a callee reads only those it takes;
//! the eight read past a shorter method's own are the caller's stack (its
//! frame, above the arguments), read and passed on, never used. Rust calls
//! this undefined (a function pointer called with another signature); the
//! ABI makes it defined, and nothing else here could forward a call whose
//! signature is only known at run time. That, the Windows calls, and reading
//! the objects' memory (each read checked with `VirtualQuery` first) are the
//! unsafe code, all of it in this module.
//!
//! # Which pointer each side sees
//!
//! A pointer passed into a module is the real object if that module owns it
//! (its vtable is in that module: the back end casts its own interfaces to
//! its classes), else a proxy (so that the calls it makes on it are
//! logged). Any argument that is a proxy whose object is the callee's is
//! unwrapped, whatever its declared type.

#![allow(unsafe_code)]
// (x64 only: usize and u64 are one size; the casts between them are exact.)
#![allow(clippy::cast_possible_truncation)]

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::sig::{self, Method, Param};

// ---------------------------------------------------------------------------
// Windows.

#[repr(C)]
#[derive(Default)]
struct MemoryBasicInformation {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, addr: *const c_void, out: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, len: u32) -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn GetCommandLineW() -> *const u16;
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
}

const FROM_ADDRESS: u32 = 4;
const UNCHANGED_REFCOUNT: u32 = 2;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_GUARD: u32 = 0x100;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Whether `len` bytes at `addr` can be read.
fn readable(addr: usize, len: usize) -> bool {
    if addr < 0x10000 || addr.checked_add(len).is_none() {
        return false;
    }
    let mut at = addr;
    let end = addr + len.max(1);
    while at < end {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: VirtualQuery writes at most `size_of` bytes of `info`.
        let got = unsafe {
            VirtualQuery(
                at as *const c_void,
                &raw mut info,
                size_of::<MemoryBasicInformation>(),
            )
        };
        if got == 0
            || info.state != MEM_COMMIT
            || info.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            || info.protect == 0
        {
            return false;
        }
        at = info.base + info.region_size;
    }
    true
}

/// The word at `addr`, if it can be read.
fn word(addr: usize) -> Option<usize> {
    // SAFETY: checked readable.
    readable(addr, 8).then(|| unsafe { (addr as *const usize).read_unaligned() })
}

/// `len` bytes at `addr`, if they can be read.
fn bytes(addr: usize, len: usize) -> Option<Vec<u8>> {
    // SAFETY: checked readable.
    readable(addr, len)
        .then(|| unsafe { std::slice::from_raw_parts(addr as *const u8, len) }.to_vec())
}

/// The NUL-terminated wide string at `addr` (up to `max` units).
fn wstr(addr: usize, max: usize) -> Option<String> {
    let mut units = Vec::new();
    while units.len() < max {
        let at = addr + 2 * units.len();
        if !readable(at, 2) {
            return None;
        }
        // SAFETY: checked readable.
        let u = unsafe { (at as *const u16).read_unaligned() };
        if u == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(u);
    }
    Some(String::from_utf16_lossy(&units) + "...")
}

/// The NUL-terminated narrow string at `addr` (up to `max` bytes).
fn cstr(addr: usize, max: usize) -> Option<String> {
    let mut out = Vec::new();
    while out.len() < max {
        let b = *bytes(addr + out.len(), 1)?.first()?;
        if b == 0 {
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        out.push(b);
    }
    Some(String::from_utf8_lossy(&out).into_owned() + "...")
}

/// The module `addr` is in: its base and its file's name (lowercase, the
/// real back end's as `c2.dll`).
fn module_of(addr: usize) -> Option<(usize, String)> {
    let mut module = std::ptr::null_mut();
    // SAFETY: a lookup by address, the handle not kept counted.
    let ok = unsafe {
        GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            addr as *const c_void,
            &raw mut module,
        )
    };
    if ok == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    // SAFETY: writes at most `buf.len()` units.
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let name = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
    let name = if name == "c2real.dll" {
        "c2.dll".to_string()
    } else {
        name
    };
    Some((module as usize, name))
}

/// This DLL's directory.
fn own_dir() -> PathBuf {
    let mut module = std::ptr::null_mut();
    // SAFETY: a lookup by the address of this function.
    unsafe {
        GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            own_dir as *const c_void,
            &raw mut module,
        );
    }
    let mut buf = [0u16; 1024];
    // SAFETY: writes at most `buf.len()` units.
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The interfaces: each vtable's methods, from `hexcavator vtables -json`.

/// A vtable's interface: a name to show and its methods.
struct Table {
    /// The interface (`IObjFile2`, or the class's last name:
    /// `XObjHandler10`).
    label: String,
    methods: Vec<Method>,
}

/// Every known vtable, by (module, RVA).
struct Tables {
    by: HashMap<(String, u64), Arc<Table>>,
}

impl Tables {
    fn load() -> Self {
        let dir = std::env::var_os("C2PROBE_META").map_or_else(own_dir, PathBuf::from);
        let mut by = HashMap::new();
        for module in ["c2.dll", "link.exe"] {
            let path = dir.join(format!("{module}.vtables.json"));
            let Ok(text) = std::fs::read_to_string(&path) else {
                log(&format!(
                    "(no {}: its objects are not wrapped)",
                    path.display()
                ));
                continue;
            };
            let text = text.trim_start_matches('\u{feff}');
            let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
                log(&format!("({}: not JSON)", path.display()));
                continue;
            };
            for t in json.as_array().into_iter().flatten() {
                let Some(rva) = t["rva"]
                    .as_str()
                    .and_then(|r| u64::from_str_radix(r.trim_start_matches("0x"), 16).ok())
                else {
                    continue;
                };
                let class = t["class"].as_str().unwrap_or("?");
                let label = t["for"].as_str().map_or_else(
                    || class.rsplit("::").next().unwrap_or(class).to_string(),
                    |b| format!("{}:{b}", class.rsplit("::").next().unwrap_or(class)),
                );
                let methods = t["slots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(slot_method)
                    .collect();
                by.insert(
                    (module.to_string(), rva),
                    Arc::new(Table { label, methods }),
                );
            }
        }
        Self { by }
    }
}

/// A slot's method: its function's name and parameters, or (folded with
/// others) its candidates' names.
fn slot_method(slot: &serde_json::Value) -> Method {
    if let Some((name, params)) = slot["name"].as_str().and_then(sig::parse) {
        return Method {
            name,
            params,
            known: true,
        };
    }
    let own = slot["own"].as_u64().unwrap_or(0) as usize;
    let names: Vec<&str> = slot["names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let shown = if own > 0 { &names[..own] } else { &names[..] };
    let name = shown
        .iter()
        .filter_map(|n| sig::parse(n).map(|(m, _)| m))
        .collect::<Vec<_>>()
        .join("|");
    Method {
        name: if name.is_empty() { "?".into() } else { name },
        params: Vec::new(),
        known: false,
    }
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(Tables::load)
}

// ---------------------------------------------------------------------------
// The log.

fn log_file() -> &'static Mutex<Option<File>> {
    static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();
    LOG.get_or_init(|| {
        let path = std::env::var_os("C2PROBE_LOG").map_or_else(
            || {
                // SAFETY: no arguments.
                let pid = unsafe { GetCurrentProcessId() };
                own_dir().join(format!("c2probe-{pid}.log"))
            },
            PathBuf::from,
        );
        Mutex::new(File::create(path).ok())
    })
}

fn log(line: &str) {
    // SAFETY: no arguments.
    let tid = unsafe { GetCurrentThreadId() };
    let mut g = log_file().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(f) = g.as_mut() {
        let _ = writeln!(f, "[{tid:>5}] {line}");
    }
}

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

// ---------------------------------------------------------------------------
// Proxies.

type Real = unsafe extern "system" fn(
    *mut c_void,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
) -> u64;

type Thunk = unsafe extern "system" fn(
    *mut Proxy,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
) -> u64;

/// A proxy: what the other side holds in place of an object.
#[repr(C)]
struct Proxy {
    /// Its vtable: the generic slots (first, as COM has it).
    vtbl: *const Thunk,
    /// The object.
    real: usize,
    /// The object's vtable (to tell a reused address from the object).
    real_vtbl: usize,
    /// The module that owns the object.
    owner: String,
    /// Its interface.
    table: Arc<Table>,
}

/// Every proxy, by its object, and the set of proxies.
#[derive(Default)]
struct Proxies {
    by_real: HashMap<usize, usize>,
    all: HashSet<usize>,
}

fn proxies() -> &'static Mutex<Proxies> {
    static P: OnceLock<Mutex<Proxies>> = OnceLock::new();
    P.get_or_init(Mutex::default)
}

/// The proxy `addr` is, if it is one.
fn as_proxy(addr: usize) -> Option<&'static Proxy> {
    let p = proxies().lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: proxies are leaked boxes, never freed.
    p.all
        .contains(&addr)
        .then(|| unsafe { &*(addr as *const Proxy) })
}

/// The owner of the object at `addr` (by its vtable's module), and its
/// table, if known.
fn identify(addr: usize) -> Option<(usize, String, Option<Arc<Table>>)> {
    let vtbl = word(addr)?;
    let (base, module) = module_of(vtbl)?;
    let table = tables()
        .by
        .get(&(module.clone(), (vtbl - base) as u64))
        .cloned();
    Some((vtbl, module, table))
}

/// A proxy for the object at `addr` (the same one each time), if its
/// interface is known.
fn wrap(addr: usize) -> Option<usize> {
    let (vtbl, owner, table) = identify(addr)?;
    let Some(table) = table else {
        log(&format!(
            "(not wrapped: an object at {addr:#x} of an unknown vtable in {owner})"
        ));
        return None;
    };
    let mut p = proxies().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&px) = p.by_real.get(&addr) {
        // SAFETY: proxies are leaked boxes, never freed.
        if unsafe { &*(px as *const Proxy) }.real_vtbl == vtbl {
            return Some(px);
        }
    }
    let label = table.label.clone();
    let px = Box::into_raw(Box::new(Proxy {
        vtbl: THUNKS.as_ptr(),
        real: addr,
        real_vtbl: vtbl,
        owner: owner.clone(),
        table,
    })) as usize;
    p.by_real.insert(addr, px);
    p.all.insert(px);
    drop(p);
    log(&format!("(new {label} of {owner}: {addr:#x}, as {px:#x})"));
    Some(px)
}

/// What to pass a callee owned by `callee` for the object or proxy `v`.
fn into(v: usize, callee: &str) -> usize {
    if v == 0 {
        return 0;
    }
    if let Some(px) = as_proxy(v) {
        return if px.owner == callee { px.real } else { v };
    }
    match identify(v) {
        Some((_, owner, _)) if owner != callee => wrap(v).unwrap_or(v),
        _ => v,
    }
}

/// What to give the caller of a callee owned by `callee` for the object or
/// proxy `v` it returned.
fn out_of(v: usize, callee: &str) -> usize {
    if v == 0 {
        return 0;
    }
    if let Some(px) = as_proxy(v) {
        return if px.owner == callee { v } else { px.real };
    }
    match identify(v) {
        Some((_, owner, _)) if owner == callee => wrap(v).unwrap_or(v),
        _ => v,
    }
}

/// Fixes up an interface returned through `slot` (a `T**`).
fn fix_out(slot: usize, callee: &str) -> Option<usize> {
    let v = word(slot)?;
    let w = out_of(v, callee);
    if w != v {
        // SAFETY: the slot was read (so is mapped), and the callee wrote it.
        unsafe { (slot as *mut usize).write_unaligned(w) };
    }
    Some(w)
}

/// A name for an object or proxy: its interface and address.
fn object(v: usize) -> String {
    if v == 0 {
        return "null".into();
    }
    if let Some(px) = as_proxy(v) {
        return format!("{}@{:#x}", px.table.label, px.real);
    }
    match identify(v) {
        Some((_, _, Some(t))) => format!("{}@{v:#x}", t.label),
        Some((vtbl, owner, None)) => format!("?{owner}+vtbl{vtbl:#x}@{v:#x}"),
        None => format!("{v:#x}"),
    }
}

/// A link data object's bytes, summarized (its `GetDataPointer`, slot 3).
fn link_data(v: usize) -> String {
    let real = as_proxy(v).map_or(v, |p| p.real);
    let Some(vtbl) = word(real) else {
        return String::new();
    };
    let Some(f) = word(vtbl + 3 * 8) else {
        return String::new();
    };
    let mut pb: usize = 0;
    let mut cb: i32 = 0;
    // SAFETY: GetDataPointer(this, BYTE**, int*), the object's own slot 3.
    let hr = unsafe {
        let f: unsafe extern "system" fn(usize, *mut usize, *mut i32) -> i32 =
            std::mem::transmute::<usize, _>(f);
        f(real, &raw mut pb, &raw mut cb)
    };
    if hr < 0 {
        return format!(" data:hr={hr:#x}");
    }
    let n = usize::try_from(cb).unwrap_or(0);
    let strings = pointed_strings(pb, n);
    format!(" data[{n}]={}{strings}", preview(pb, n))
}

/// The back end's symbol handles (`IPublicSym::ISym`'s) and names
/// (`SzName`'s), by the public symbol object each came from, and the names
/// by handle once both are known.
#[derive(Default)]
struct Handles {
    handle: HashMap<usize, usize>,
    name: HashMap<usize, String>,
    named: HashMap<usize, String>,
}

fn handles() -> &'static Mutex<Handles> {
    static H: OnceLock<Mutex<Handles>> = OnceLock::new();
    H.get_or_init(Mutex::default)
}

/// Notes a public symbol object's handle or name.
fn note_symbol(object: usize, handle: Option<usize>, name: Option<String>) {
    let mut h = handles().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(v) = handle {
        h.handle.insert(object, v);
    }
    if let Some(n) = name {
        h.name.insert(object, n);
    }
    if let (Some(&v), Some(n)) = (h.handle.get(&object), h.name.get(&object).cloned()) {
        h.named.insert(v, n);
    }
}

/// A link data object's bytes (its `GetDataPointer`, slot 3).
fn link_data_bytes(v: usize) -> Option<Vec<u8>> {
    let real = as_proxy(v).map_or(v, |p| p.real);
    let f = word(word(real)? + 3 * 8)?;
    let mut pb: usize = 0;
    let mut cb: i32 = 0;
    // SAFETY: GetDataPointer(this, BYTE**, int*), the object's own slot 3.
    let hr = unsafe {
        let f: unsafe extern "system" fn(usize, *mut usize, *mut i32) -> i32 =
            std::mem::transmute::<usize, _>(f);
        f(real, &raw mut pb, &raw mut cb)
    };
    if hr < 0 {
        return None;
    }
    bytes(pb, usize::try_from(cb).ok()?.min(1 << 20))
}

/// The names of an array of the back end's symbol handles, if every word of
/// it is one, as ` -> [a b]`.
fn pointed_strings(addr: usize, n: usize) -> String {
    if n == 0 || !n.is_multiple_of(8) {
        return String::new();
    }
    let h = handles().lock().unwrap_or_else(PoisonError::into_inner);
    let mut out = Vec::new();
    for i in 0..(n / 8).min(4096) {
        let Some(p) = word(addr + 8 * i) else {
            return String::new();
        };
        match h.named.get(&p) {
            Some(s) => out.push(s.clone()),
            None => return String::new(),
        }
    }
    format!(" -> symbols [{}]", out.join(" "))
}

/// The first bytes at `addr` of `n`: as text if they are, else as hex.
fn preview(addr: usize, n: usize) -> String {
    let take = n.min(64);
    let Some(b) = bytes(addr, take) else {
        return format!("{addr:#x}(unreadable)");
    };
    let text = b
        .iter()
        .take_while(|&&c| c != 0)
        .all(|&c| (0x20..0x7f).contains(&c) || c == b'\t');
    let shown = if text && !b.is_empty() && b[0] != 0 {
        let s: String = b
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as char)
            .collect();
        format!("{s:?}")
    } else {
        b.iter().fold(String::new(), |mut s, c| {
            let _ = write!(s, "{c:02x}");
            s
        })
    };
    if n > take {
        format!("{shown}...")
    } else {
        shown
    }
}

fn guid(addr: usize) -> String {
    match bytes(addr, 16) {
        Some(b) => format!(
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        ),
        None => format!("{addr:#x}"),
    }
}

/// An argument, shown before the call.
fn show_in(p: &Param, v: usize, prev: Option<usize>, method: &str) -> String {
    match p {
        Param::Scalar => format!("{v:#x}"),
        Param::Guid => guid(v),
        Param::WStr | Param::WStrOrOut => match wstr(v, 520) {
            Some(s) if v != 0 => format!("{s:?}"),
            _ => format!("{v:#x}"),
        },
        Param::WStrs if method.starts_with("CodeGen") || method == "Init" => {
            // An argument vector of `prev` strings.
            let n = prev.unwrap_or(0).min(256);
            let mut s = String::from("[");
            for i in 0..n {
                let a = word(v + 8 * i).and_then(|p| wstr(p, 1024));
                let _ = write!(
                    s,
                    "{}{:?}",
                    if i > 0 { " " } else { "" },
                    a.unwrap_or_default()
                );
            }
            s.push(']');
            s
        }
        Param::Str => match cstr(v, 520) {
            Some(s) if v != 0 => format!("{s:?}"),
            _ => format!("{v:#x}"),
        },
        Param::Iface(name) => {
            let data = if name.starts_with("ILinkData") && v != 0 {
                link_data(v)
            } else {
                String::new()
            };
            format!("{}{data}", object(v))
        }
        Param::WStrs | Param::OutIface(_) | Param::OutScalar(_) | Param::Bytes | Param::Raw => {
            format!("{v:#x}")
        }
    }
}

/// An argument, shown after the call (what it returned, for an out
/// parameter), if anything.
fn show_out(p: &Param, v: usize, next: Option<usize>, callee: &str) -> Option<String> {
    if v == 0 {
        return None;
    }
    match p {
        Param::OutScalar(w) => {
            let b = bytes(v, usize::from(*w))?;
            let mut x = [0u8; 8];
            x[..b.len()].copy_from_slice(&b);
            Some(format!("{:#x}", u64::from_le_bytes(x)))
        }
        Param::WStrOrOut => bytes(v, 2).map(|b| format!("{:#x}", u16::from_le_bytes([b[0], b[1]]))),
        Param::WStrs => word(v)
            .and_then(|s| wstr(s, 1024))
            .map(|s| format!("{s:?}")),
        Param::OutIface(name) => {
            let w = fix_out(v, callee)?;
            let data = if name.starts_with("ILinkData") && w != 0 {
                link_data(w)
            } else {
                String::new()
            };
            Some(format!("{}{data}", object(w)))
        }
        Param::Bytes => {
            let n = next.filter(|&n| n > 0 && n < 1 << 30).unwrap_or(18);
            Some(preview(v, n))
        }
        _ => None,
    }
}

/// A call through slot `slot` of a proxy: logged and forwarded.
///
/// # Safety
///
/// `this` is a proxy, and the caller called its slot `slot` with `a`.
unsafe fn call(slot: usize, this: *mut Proxy, a: [u64; 12]) -> u64 {
    // SAFETY: a proxy (the caller holds it: it is never freed).
    let px = unsafe { &*this };
    let callee = px.owner.as_str();
    let unknown = Method {
        name: format!("slot{slot}"),
        params: Vec::new(),
        known: false,
    };
    let method = px.table.methods.get(slot).unwrap_or(&unknown);
    let mut args = a.map(|v| v as usize);
    // Proxies of the callee's own objects, unwrapped; interfaces of the
    // caller's given, wrapped.
    for v in &mut args {
        if let Some(p) = as_proxy(*v)
            && p.owner == callee
        {
            *v = p.real;
        }
    }
    let is_qi = method.name == "QueryInterface";
    for (i, p) in method.params.iter().enumerate().take(12) {
        if let Param::Iface(_) = p {
            args[i] = into(args[i], callee);
        }
    }
    // The call, shown.
    let depth = DEPTH.with(|d| {
        let v = d.get();
        d.set(v + 1);
        v
    });
    let indent = "  ".repeat(depth.min(40));
    let shown_args = if method.known {
        method
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| show_in(p, args[i], i.checked_sub(1).map(|j| args[j]), &method.name))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        args[..6]
            .iter()
            .map(|v| format!("{v:#x}"))
            .collect::<Vec<_>>()
            .join(", ")
            + ", ..."
    };
    log(&format!(
        "{indent}> {}.{}({shown_args})  [{callee}]",
        px.table.label, method.name
    ));
    // SAFETY: the object's slot `slot`, called as the caller called ours
    // (see the module's documentation).
    let r = unsafe {
        let vtbl = (px.real as *const *const Real).read();
        let f = vtbl.add(slot).read();
        f(
            px.real as *mut c_void,
            args[0] as u64,
            args[1] as u64,
            args[2] as u64,
            args[3] as u64,
            args[4] as u64,
            args[5] as u64,
            args[6] as u64,
            args[7] as u64,
            args[8] as u64,
            args[9] as u64,
            args[10] as u64,
            args[11] as u64,
        )
    };
    DEPTH.with(|d| d.set(depth));
    let hr = r as u32 as i32;
    // What it returned: interfaces wrapped or unwrapped for the caller.
    let mut outs = Vec::new();
    if is_qi {
        if hr >= 0 {
            outs.push(format!(
                "{} -> {}",
                guid(args[0]),
                object(fix_out(args[1], callee).unwrap_or(0))
            ));
        } else {
            outs.push(guid(args[0]));
        }
    } else if method.name == "Next"
        && matches!(
            method.params.as_slice(),
            [Param::Scalar, Param::OutIface(_), Param::OutScalar(_)]
        )
    {
        // An enumerator's: `n` asked for, `*fetched` given.
        let fetched = if args[2] == 0 {
            if hr == 0 { args[0] } else { 0 }
        } else {
            bytes(args[2], 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        };
        let mut got = Vec::new();
        for i in 0..fetched.min(4096) {
            if let Some(w) = fix_out(args[1] + 8 * i, callee) {
                got.push(object(w));
            }
        }
        outs.push(format!("[{}]", got.join(" ")));
    } else if method.known {
        for (i, p) in method.params.iter().enumerate() {
            if hr < 0 && matches!(p, Param::OutIface(_)) {
                continue;
            }
            if let Some(s) = show_out(p, args[i], args.get(i + 1).copied(), callee) {
                outs.push(format!("#{i}={s}"));
            }
        }
    }
    // A public symbol's handle and name, remembered (to name the handles
    // the linker passes back).
    if hr >= 0 && px.table.label.starts_with("XPublicSym") {
        match method.name.as_str() {
            "ISym" => note_symbol(px.real, word(args[0]), None),
            "SzName" => {
                let name = word(args[0]).and_then(link_data_bytes).map(|b| {
                    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned()
                });
                note_symbol(px.real, None, name);
            }
            _ => {}
        }
    }
    let result = if matches!(method.name.as_str(), "AddRef" | "Release") {
        format!("{}", r as u32)
    } else {
        format!("{hr:#x}")
    };
    log(&format!(
        "{indent}< {}.{} = {result}{}{}",
        px.table.label,
        method.name,
        if outs.is_empty() { "" } else { "  " },
        outs.join(", ")
    ));
    r
}

/// The generic slot `N`.
unsafe extern "system" fn thunk<const N: usize>(
    this: *mut Proxy,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
    a12: u64,
) -> u64 {
    // SAFETY: called through a proxy's vtable, as its slot N.
    unsafe { call(N, this, [a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12]) }
}

macro_rules! thunks {
    ($($n:literal)*) => { [$(thunk::<$n> as Thunk),*] };
}

/// The proxies' vtable.
static THUNKS: [Thunk; 128] = thunks!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
    32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60
    61 62 63 64 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89
    90 91 92 93 94 95 96 97 98 99 100 101 102 103 104 105 106 107 108 109 110 111 112 113
    114 115 116 117 118 119 120 121 122 123 124 125 126 127
);

// ---------------------------------------------------------------------------
// The back end's exports.

/// The real back end (`c2real.dll`, beside this one).
fn real() -> usize {
    static REAL: OnceLock<usize> = OnceLock::new();
    *REAL.get_or_init(|| {
        let path = own_dir().join("c2real.dll");
        let w = wide(&path.to_string_lossy());
        // SAFETY: a NUL-terminated path.
        let m = unsafe { LoadLibraryW(w.as_ptr()) } as usize;
        // SAFETY: no arguments; the command line is NUL-terminated.
        let cmd = wstr(unsafe { GetCommandLineW() } as usize, 32768).unwrap_or_default();
        log(&format!("(probe: {cmd})"));
        log(&format!("(real back end {}: {m:#x})", path.display()));
        m
    })
}

/// The real back end's export `name` (NUL-terminated).
fn export(name: &[u8]) -> usize {
    let m = real();
    if m == 0 {
        return 0;
    }
    // SAFETY: a module handle and a NUL-terminated name.
    unsafe { GetProcAddress(m as *mut c_void, name.as_ptr()) as usize }
}

/// `HRESULT DllGetObjHandler(REFCLSID, REFIID, void**)`: the back end's
/// object handler, the linker's way in.
///
/// # Safety
///
/// As the real one's.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetObjHandler(
    clsid: *const c_void,
    iid: *const c_void,
    out: *mut *mut c_void,
) -> i32 {
    let f = export(b"DllGetObjHandler\0");
    log(&format!(
        "> DllGetObjHandler({}, {})",
        guid(clsid as usize),
        guid(iid as usize)
    ));
    if f == 0 {
        log("< DllGetObjHandler: no real back end");
        return 0x8000_4005_u32 as i32;
    }
    // SAFETY: the real export, with this one's arguments.
    let hr = unsafe {
        let f: unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> i32 =
            std::mem::transmute::<usize, _>(f);
        f(clsid, iid, out)
    };
    let got = if hr >= 0 {
        fix_out(out as usize, "c2.dll").map_or_else(|| "?".into(), object)
    } else {
        String::new()
    };
    log(&format!("< DllGetObjHandler = {hr:#x}  {got}"));
    hr
}

/// Forwards an export the linker doesn't call through the handler: the
/// compiler passes' entry points, and the rest, by their real ones.
macro_rules! forward {
    ($($name:ident)*) => {$(
        /// Forwarded to the real back end's export of the same name.
        ///
        /// # Safety
        ///
        /// As the real one's.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name(
            a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64, a7: u64, a8: u64,
        ) -> u64 {
            let f = export(concat!(stringify!($name), "\0").as_bytes());
            log(concat!("(forwarded ", stringify!($name), ")"));
            if f == 0 {
                return 0x8000_4005;
            }
            // SAFETY: the real export, with this one's arguments (see the
            // module's documentation: integers and pointers only).
            unsafe {
                let f: unsafe extern "system" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute::<usize, _>(f);
                f(a1, a2, a3, a4, a5, a6, a7, a8)
            }
        }
    )*};
}

forward!(
    AbortCompilerPass CloseTypeServerPDB InvokeCompilerPass InvokeCompilerPassW
    UtcBldCompileSerializedFile UtcBldGetFormatVersion UtcBldRustDemangleFree
    UtcBldRustDemangleWithBuffer
);
