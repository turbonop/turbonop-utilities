//! `ltcg-drive`: link-time code generation without `link.exe`. It drives
//! the compiler's back end (`c2.dll`) through the interface `link.exe`
//! uses (as `turbonop-ltcg-probe` logs it), and writes the code it
//! generates as an ordinary COFF object, for any linker to link:
//!
//!   ltcg-drive <c2.dll> <out.obj> [-root <name>]... [-arg <switch>]... <in.obj>...
//!
//! The roots are the IL definitions something outside the IL refers to
//! (`main` by default: the C runtime's startup code calls it); the
//! switches are the back end's (`-GA` by default: an EXE).
//!
//! A proof of concept: one link's worth of `/GL` objects (no libraries,
//! no native objects), its object written as the back end has it.

#![cfg_attr(all(windows, target_arch = "x86_64"), allow(unsafe_code))]

use turbonop_global_alloc as _;

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("ltcg-drive: Windows x64 only");
    std::process::exit(2);
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match drive::run(&args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("ltcg-drive: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod drive {
    //! Every call to the back end goes through its objects' vtables, by
    //! the slots `hexcavator vtables` reads from its public symbols; every
    //! object the back end calls back on is one of ours, with a vtable of
    //! the same shape as `link.exe`'s. That, and the Windows calls, are
    //! the unsafe code.

    // (x64 only: usize and u64 are one size.)
    #![allow(clippy::cast_possible_truncation)]

    use std::ffi::c_void;
    use std::fmt::Write as _;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetLastError() -> u32;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    type Hr = i32;
    const S_OK: Hr = 0;
    const E_NOINTERFACE: Hr = 0x8000_4002_u32 as i32;

    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Guid(u32, u16, u16, [u8; 8]);

    const CLSID_OBJ_HANDLER: Guid = Guid(
        0x0cb3_fe38,
        0xd9a5,
        0x4dab,
        [0xac, 0x9b, 0xd6, 0xb6, 0x22, 0x26, 0x53, 0xc2],
    );
    const IID_OBJ_HANDLER6: Guid = Guid(
        0x0737_f2d8,
        0x976c,
        0x4a67,
        [0xa9, 0x50, 0x45, 0x6c, 0xc5, 0xa6, 0x5d, 0xb4],
    );
    const IID_OBJ_HANDLER9: Guid = Guid(
        0x447e_3a28,
        0xb77b,
        0x4d2e,
        [0xbb, 0x4c, 0x6f, 0xdb, 0x16, 0x22, 0xc5, 0x7f],
    );
    const IID_OBJ_HANDLER10: Guid = Guid(
        0x5121_79b7,
        0xae57,
        0x40a2,
        [0xac, 0x1e, 0x59, 0x86, 0x58, 0x6a, 0x6e, 0x4d],
    );
    const IID_MEM_COORDINATOR: Guid = Guid(
        0x02e3_7a58,
        0xb410,
        0x44f5,
        [0x88, 0xe5, 0x83, 0xfe, 0xd6, 0xd9, 0x97, 0x11],
    );
    const IID_LINK_NOTIFY2: Guid = Guid(
        0xc0c2_1495,
        0x7642,
        0x4e6b,
        [0x81, 0x81, 0x94, 0xca, 0x77, 0x89, 0xb2, 0x41],
    );
    const IID_COFF_OBJ: Guid = Guid(
        0xa947_0bb0,
        0x911f,
        0x11d1,
        [0xa7, 0x12, 0x00, 0x60, 0x08, 0x3e, 0x8c, 0x78],
    );

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    // -----------------------------------------------------------------
    // Calls into the back end's objects.

    /// The function in slot `slot` of `obj`'s vtable, as `F`.
    ///
    /// # Safety
    ///
    /// `obj` is a COM object whose slot `slot` has the signature `F`.
    unsafe fn slot<F: Copy>(obj: *mut c_void, slot: usize) -> F {
        // SAFETY: the caller's.
        unsafe {
            let vtbl = *(obj as *const *const usize);
            let f = *vtbl.add(slot);
            std::mem::transmute_copy::<usize, F>(&f)
        }
    }

    type FnQi = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hr;
    type FnU32 = unsafe extern "system" fn(*mut c_void, *mut u32) -> Hr;
    type FnObj = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hr;

    fn qi(obj: *mut c_void, iid: &Guid) -> Result<*mut c_void, String> {
        let mut out = std::ptr::null_mut();
        // SAFETY: slot 0 of every COM object.
        let hr = unsafe { slot::<FnQi>(obj, 0)(obj, iid, &raw mut out) };
        if hr < 0 || out.is_null() {
            return Err(format!("QueryInterface: {hr:#x}"));
        }
        Ok(out)
    }

    fn get_u32(obj: *mut c_void, at: usize, what: &str) -> Result<u32, String> {
        let mut v = 0u32;
        // SAFETY: a getter of a `DWORD*` (by the interface's table).
        let hr = unsafe { slot::<FnU32>(obj, at)(obj, &raw mut v) };
        if hr < 0 {
            return Err(format!("{what}: {hr:#x}"));
        }
        Ok(v)
    }

    fn get_obj(obj: *mut c_void, at: usize, what: &str) -> Result<*mut c_void, String> {
        let mut v = std::ptr::null_mut();
        // SAFETY: a getter of an interface (by the interface's table).
        let hr = unsafe { slot::<FnObj>(obj, at)(obj, &raw mut v) };
        if hr < 0 || v.is_null() {
            return Err(format!("{what}: {hr:#x}"));
        }
        Ok(v)
    }

    /// A link data object's bytes (`ILinkDataRO::GetDataPointer`, slot 3).
    fn data(obj: *mut c_void) -> Result<Vec<u8>, String> {
        let mut pb: *const u8 = std::ptr::null();
        let mut cb: i32 = 0;
        // SAFETY: GetDataPointer(BYTE**, int*).
        let hr =
            unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *mut *const u8, *mut i32) -> Hr>(
                    obj, 3,
                )(obj, &raw mut pb, &raw mut cb)
            };
        if hr < 0 {
            return Err(format!("GetDataPointer: {hr:#x}"));
        }
        let n = usize::try_from(cb).unwrap_or(0);
        if n == 0 || pb.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: the back end's buffer of `cb` bytes.
        Ok(unsafe { std::slice::from_raw_parts(pb, n) }.to_vec())
    }

    /// A link data object's bytes as a NUL-terminated name.
    fn name(obj: *mut c_void) -> Result<String, String> {
        let d = data(obj)?;
        let end = d.iter().position(|&c| c == 0).unwrap_or(d.len());
        Ok(String::from_utf8_lossy(&d[..end]).into_owned())
    }

    // -----------------------------------------------------------------
    // Our objects, which the back end calls.

    #[repr(C)]
    struct LinkData {
        vtbl: *const usize,
        bytes: *const u8,
        len: i32,
    }

    unsafe extern "system" fn qi_none(
        this: *mut c_void,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hr {
        // SAFETY: an IID and an out pointer.
        unsafe {
            let g = &*iid;
            eprintln!(
                "ltcg-drive: {this:p} asked for {:08x}-{:04x}-{:04x}-{:02x?}",
                g.0, g.1, g.2, g.3
            );
            *out = std::ptr::null_mut();
        }
        E_NOINTERFACE
    }
    unsafe extern "system" fn add_ref(_: *mut c_void) -> u32 {
        2
    }
    unsafe extern "system" fn release(_: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn get_data_pointer(
        this: *mut LinkData,
        pb: *mut *const u8,
        cb: *mut i32,
    ) -> Hr {
        // SAFETY: one of ours; out pointers.
        unsafe {
            *pb = (*this).bytes;
            *cb = (*this).len;
        }
        S_OK
    }

    fn link_data_vtbl() -> *const usize {
        use std::sync::OnceLock;
        static V: OnceLock<[usize; 4]> = OnceLock::new();
        V.get_or_init(|| {
            [
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release as *const () as usize,
                get_data_pointer as *const () as usize,
            ]
        })
        .as_ptr()
    }

    /// A link data object holding `bytes` (leaked: the back end may keep it).
    fn link_data(bytes: Vec<u8>) -> *mut c_void {
        let b = bytes.leak();
        Box::into_raw(Box::new(LinkData {
            vtbl: link_data_vtbl(),
            bytes: if b.is_empty() {
                std::ptr::null()
            } else {
                b.as_ptr()
            },
            len: i32::try_from(b.len()).unwrap_or(i32::MAX),
        })) as *mut c_void
    }

    #[repr(C)]
    struct Plain {
        vtbl: *const usize,
    }

    // The memory coordinator (`IMemCoordinator`): Register, Unregister,
    // NotifyOOM.
    unsafe extern "system" fn mem_register(_: *mut c_void, handler: *mut c_void) -> Hr {
        eprintln!("ltcg-drive: memory handler registered");
        // SAFETY: the back end's handler: AddRef it, as link.exe does.
        unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(handler, 1)(handler) };
        S_OK
    }
    unsafe extern "system" fn mem_unregister(_: *mut c_void, handler: *mut c_void) -> Hr {
        // SAFETY: the back end's handler: Release it, as link.exe does.
        unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(handler, 2)(handler) };
        S_OK
    }
    unsafe extern "system" fn mem_notify_oom(_: *mut c_void) -> Hr {
        S_OK
    }

    // The link notifications (`ILinkNotify2`): OnError, OnStatus,
    // AddExternalFilenames.
    unsafe extern "system" fn notify_error(_: *mut c_void, err: *mut c_void) -> Hr {
        // ILinkError2: ErrorType (3), Objtk1, Objtk2, Sz (6), DwInfo.
        let mut sz: *const u16 = std::ptr::null();
        // SAFETY: the back end's error object, its slot 6 Sz(WCHAR**).
        let hr = unsafe {
            slot::<unsafe extern "system" fn(*mut c_void, *mut *const u16) -> Hr>(err, 6)(
                err,
                &raw mut sz,
            )
        };
        let text = if hr >= 0 && !sz.is_null() {
            // SAFETY: a NUL-terminated string of the back end's.
            unsafe {
                let mut n = 0;
                while *sz.add(n) != 0 {
                    n += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(sz, n))
            }
        } else {
            format!("(no text: {hr:#x})")
        };
        eprintln!("ltcg-drive: back end: {text}");
        S_OK
    }
    unsafe extern "system" fn notify_status(_: *mut c_void, done: u32, total: u32) -> Hr {
        if done == total {
            eprintln!("ltcg-drive: code generated ({total} units)");
        }
        S_OK
    }
    unsafe extern "system" fn notify_filenames(_: *mut c_void, _: *mut c_void) -> Hr {
        S_OK
    }

    fn leak_vtbl(slots: Vec<usize>) -> *const usize {
        slots.leak().as_ptr()
    }

    static OURS: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

    // The linker's code generation object (CLinkCG): only QueryInterface,
    // for the coordinator and the notifications.
    unsafe extern "system" fn cg_qi(_: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hr {
        let &(coordinator, notify) = OURS.get().expect("ours");
        // SAFETY: an IID.
        let g = unsafe { &*iid };
        eprintln!(
            "ltcg-drive: back end asks for {:08x}-{:04x}-{:04x}",
            g.0, g.1, g.2
        );
        // SAFETY: an IID and an out pointer.
        unsafe {
            *out = if *iid == IID_MEM_COORDINATOR {
                coordinator as *mut c_void
            } else if *iid == IID_LINK_NOTIFY2 {
                notify as *mut c_void
            } else {
                let g = &*iid;
                eprintln!(
                    "ltcg-drive: asked for {:08x}-{:04x}-{:04x}-{:02x?}: not ours",
                    g.0, g.1, g.2, g.3
                );
                std::ptr::null_mut()
            };
            if (*out).is_null() {
                E_NOINTERFACE
            } else {
                S_OK
            }
        }
    }

    fn ours() -> *mut c_void {
        let coordinator = Box::into_raw(Box::new(Plain {
            vtbl: leak_vtbl(vec![
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release as *const () as usize,
                mem_register as *const () as usize,
                mem_unregister as *const () as usize,
                mem_notify_oom as *const () as usize,
            ]),
        })) as *mut c_void;
        let notify = Box::into_raw(Box::new(Plain {
            vtbl: leak_vtbl(vec![
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release as *const () as usize,
                notify_error as *const () as usize,
                notify_status as *const () as usize,
                notify_filenames as *const () as usize,
            ]),
        })) as *mut c_void;
        let _ = OURS.set((coordinator as usize, notify as usize));

        Box::into_raw(Box::new(Plain {
            vtbl: leak_vtbl(vec![
                cg_qi as *const () as usize,
                add_ref as *const () as usize,
                release as *const () as usize,
            ]),
        })) as *mut c_void
    }

    // -----------------------------------------------------------------
    // The link.

    struct Options {
        c2: String,
        out: String,
        roots: Vec<String>,
        switches: Vec<String>,
        inputs: Vec<String>,
    }

    fn parse(args: &[String]) -> Result<Options, String> {
        let mut roots = Vec::new();
        let mut switches = Vec::new();
        let mut positional = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-root" => roots.push(it.next().ok_or("-root needs a name")?.clone()),
                "-arg" => switches.push(it.next().ok_or("-arg needs a switch")?.clone()),
                _ => positional.push(a.clone()),
            }
        }
        if positional.len() < 3 {
            return Err("usage: ltcg-drive <c2.dll> <out.obj> [-root <name>]... [-arg <switch>]... <in.obj>...".into());
        }
        if roots.is_empty() {
            roots.push("main".into());
        }
        if switches.is_empty() {
            switches.push("-GA".into());
        }
        let mut p = positional.into_iter();
        Ok(Options {
            c2: p.next().unwrap_or_default(),
            out: p.next().unwrap_or_default(),
            roots,
            switches,
            inputs: p.collect(),
        })
    }

    pub fn run(args: &[String]) -> Result<(), String> {
        let o = parse(args)?;
        let path = wide(&o.c2);
        // SAFETY: a NUL-terminated path; its dependencies found beside it
        // (LOAD_WITH_ALTERED_SEARCH_PATH).
        let module = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), 8) };
        if module.is_null() {
            // SAFETY: no arguments.
            let e = unsafe { GetLastError() };
            return Err(format!("{}: can't load (error {e})", o.c2));
        }
        // SAFETY: a module and a NUL-terminated name.
        let get = unsafe { GetProcAddress(module, c"DllGetObjHandler".as_ptr().cast()) };
        if get.is_null() {
            return Err("no DllGetObjHandler".into());
        }
        // SAFETY: HRESULT DllGetObjHandler(REFCLSID, REFIID, void**).
        let get: unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> Hr =
            unsafe { std::mem::transmute::<*mut c_void, _>(get) };
        let mut h6 = std::ptr::null_mut();
        // SAFETY: as link.exe calls it.
        let hr = unsafe { get(&CLSID_OBJ_HANDLER, &IID_OBJ_HANDLER6, &raw mut h6) };
        if hr < 0 || h6.is_null() {
            return Err(format!("DllGetObjHandler: {hr:#x}"));
        }
        let cg = ours();
        // IObjHandler6::Init (slot 4): (IUnknown* linker, int, int argc,
        // WCHAR** argv): link.exe's switches for the back end.
        let init_switch = wide("-msilDebug");
        let mut init_argv: Vec<*const u16> = vec![init_switch.as_ptr()];
        // SAFETY: as link.exe calls it (0, 1, ["-msilDebug"]).
        let hr = unsafe {
            slot::<
                unsafe extern "system" fn(
                    *mut c_void,
                    *mut c_void,
                    i32,
                    i32,
                    *mut *const u16,
                ) -> Hr,
            >(h6, 4)(h6, cg, 0, 1, init_argv.as_mut_ptr())
        };
        if hr < 0 {
            return Err(format!("Init: {hr:#x}"));
        }
        eprintln!("ltcg-drive: initialized");
        // IObjHandler9::SetPdbDll (slot 14).
        let h9 = qi(h6, &IID_OBJ_HANDLER9)?;
        let pdb = wide("mspdbcore.dll");
        let mut out: *mut u16 = std::ptr::null_mut();
        // SAFETY: SetPdbDll(const WCHAR*, WCHAR**).
        let hr =
            unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut u16) -> Hr>(
                    h9, 14,
                )(h9, pdb.as_ptr(), &raw mut out)
            };
        if hr < 0 {
            return Err(format!("SetPdbDll: {hr:#x}"));
        }

        eprintln!("ltcg-drive: PDB DLL set");
        // Each object, opened (IObjFileForFile2, slot 11), and its publics.
        let mut files = Vec::new();
        // The inputs' directives (IObjFile2::PbDirectives, slot 8): the
        // generated object's own .drectve doesn't hold them.
        let mut directives: Vec<u8> = Vec::new();
        let mut roots: Vec<u64> = Vec::new();
        for (i, input) in o.inputs.iter().enumerate() {
            let size = std::fs::metadata(input)
                .map_err(|e| format!("{input}: {e}"))?
                .len();
            let w = wide(input);
            // The object's token: link.exe's is its object's address;
            // ours, a block of its own (opaque to the back end, as far as
            // the logs show).
            let token = Box::leak(vec![0u8; 4096].into_boxed_slice()).as_ptr() as u64;
            let mut file = std::ptr::null_mut();
            let mut flags = 0u32;
            let machine: u16 = if i == 0 { 0 } else { 0x8664 };
            // SAFETY: IObjFileForFile2(WCHAR* path, WORD machine, DWORD
            // offset, DWORD size, ULONGLONG token, kind, IObjFile2**, DWORD*).
            let hr = unsafe {
                slot::<
                    unsafe extern "system" fn(
                        *mut c_void,
                        *const u16,
                        u16,
                        u32,
                        u32,
                        u64,
                        u32,
                        *mut *mut c_void,
                        *mut u32,
                    ) -> Hr,
                >(h6, 11)(
                    h6,
                    w.as_ptr(),
                    machine,
                    0,
                    u32::try_from(size).map_err(|_| format!("{input}: too big"))?,
                    token,
                    0,
                    &raw mut file,
                    &raw mut flags,
                )
            };
            if hr < 0 || file.is_null() {
                return Err(format!("{input}: IObjFileForFile2: {hr:#x}"));
            }
            eprintln!("ltcg-drive: {input}: opened (flags {flags:#x})");
            // Its publics (IObjFile2::EnumPublics, slot 5; IEnumPublics::Next).
            let mut e = std::ptr::null_mut();
            // SAFETY: EnumPublics(IEnumPublics**, ULONGLONG token).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void, u64) -> Hr>(file, 5)(
                    file, &raw mut e, token,
                )
            };
            if hr < 0 || e.is_null() {
                return Err(format!("{input}: EnumPublics: {hr:#x}"));
            }
            let mut listing = String::new();
            loop {
                let mut sym = std::ptr::null_mut();
                let mut got = 0u32;
                // SAFETY: Next(ULONG, IPublicSym**, ULONG*), slot 3.
                let hr = unsafe {
                    slot::<
                        unsafe extern "system" fn(
                            *mut c_void,
                            u32,
                            *mut *mut c_void,
                            *mut u32,
                        ) -> Hr,
                    >(e, 3)(e, 1, &raw mut sym, &raw mut got)
                };
                if hr < 0 || got == 0 || sym.is_null() {
                    break;
                }
                // IPublicSym: FComdat 3, ISym 4, TkFile 5, SzName 6, SymType 7.
                let kind = get_u32(sym, 7, "SymType")?;
                let mut handle = 0u64;
                // SAFETY: ISym(ULONGLONG*).
                let hr = unsafe {
                    slot::<unsafe extern "system" fn(*mut c_void, *mut u64) -> Hr>(sym, 4)(
                        sym,
                        &raw mut handle,
                    )
                };
                if hr < 0 {
                    return Err(format!("ISym: {hr:#x}"));
                }
                let n = name(get_obj(sym, 6, "SzName")?)?;
                let _ = write!(listing, " {n}({kind})");
                if o.roots.contains(&n) {
                    roots.push(handle);
                }
                // SAFETY: Release, slot 2.
                unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(sym, 2)(sym) };
            }
            eprintln!("ltcg-drive: {input}:{listing}");
            let d = data(get_obj(file, 8, "PbDirectives")?)?;
            directives.push(b' ');
            directives.extend(d.iter().take_while(|&&c| c != 0));
            files.push(file);
        }
        if roots.is_empty() {
            return Err(format!("none of the roots {:?} is an IL symbol", o.roots));
        }

        // Code generation: IObjHandler10::CodeGen6 (slot 15).
        let h10 = qi(h6, &IID_OBJ_HANDLER10)?;
        let switches: Vec<Vec<u16>> = o.switches.iter().map(|s| wide(s)).collect();
        let mut argv: Vec<*const u16> = switches.iter().map(std::vec::Vec::as_ptr).collect();
        let roots_data = link_data(roots.iter().flat_map(|r| r.to_le_bytes()).collect());
        let none1 = link_data(Vec::new());
        let none2 = link_data(Vec::new());
        let (mut out1, mut out2, mut out3) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let mut flag = 0i32;
        // SAFETY: CodeGen6(int, WCHAR**, ILinkDataRO* x3, ILinkDataRO**
        // x2, int*, ILinkDataRO**), as link.exe calls it.
        let hr = unsafe {
            slot::<
                unsafe extern "system" fn(
                    *mut c_void,
                    i32,
                    *mut *const u16,
                    *mut c_void,
                    *mut c_void,
                    *mut c_void,
                    *mut *mut c_void,
                    *mut *mut c_void,
                    *mut i32,
                    *mut *mut c_void,
                ) -> Hr,
            >(h10, 15)(
                h10,
                i32::try_from(argv.len()).unwrap_or(0),
                argv.as_mut_ptr(),
                roots_data,
                none1,
                none2,
                &raw mut out1,
                &raw mut out2,
                &raw mut flag,
                &raw mut out3,
            )
        };
        if hr < 0 {
            return Err(format!("CodeGen6: {hr:#x}"));
        }

        // The code: the first object's ICOFFObj.
        let coff = qi(files[0], &IID_COFF_OBJ)?;
        let obj = write_coff(coff, &directives)?;
        std::fs::write(&o.out, &obj).map_err(|e| format!("{}: {e}", o.out))?;
        eprintln!("ltcg-drive: wrote {} ({} bytes)", o.out, obj.len());
        Ok(())
    }

    // -----------------------------------------------------------------
    // The generated object, as a COFF file.

    struct Section {
        name: String,
        characteristics: u32,
        bytes: Vec<u8>,
        size: u32,
        relocs: Vec<u8>,
        nrelocs: u32,
    }

    fn write_coff(coff: *mut c_void, directives: &[u8]) -> Result<Vec<u8>, String> {
        // ICOFFObj: CContrib 4, CSymbols 7, GetContribForISec 10,
        // GetSymbolTableRO 13.
        let ncontrib = get_u32(coff, 4, "CContrib")?;
        let mut sections = Vec::new();
        for isec in 1..=ncontrib {
            let mut c = std::ptr::null_mut();
            // SAFETY: GetContribForISec(DWORD, IObjectContrib**).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hr>(
                    coff, 10,
                )(coff, isec, &raw mut c)
            };
            if hr < 0 || c.is_null() {
                return Err(format!("GetContribForISec({isec}): {hr:#x}"));
            }
            // IObjectContrib: CbContents 3, Characteristics 4, CRelocs 6,
            // PbContents 8, RgRelocs 12, SzSectionName 14.
            let size = get_u32(c, 3, "CbContents")?;
            let characteristics = get_u32(c, 4, "Characteristics")?;
            let nrelocs = get_u32(c, 6, "CRelocs")?;
            let name = name(get_obj(c, 14, "SzSectionName")?)?;
            let bss = characteristics & 0x80 != 0;
            let mut bytes = vec![0u8; if bss { 0 } else { size as usize }];
            if !bss && size > 0 {
                let mut got = 0u32;
                // SAFETY: PbContents(BYTE*, DWORD, DWORD*): our buffer.
                let hr = unsafe {
                    slot::<unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> Hr>(
                        c, 8,
                    )(c, bytes.as_mut_ptr(), size, &raw mut got)
                };
                if hr < 0 {
                    return Err(format!("{name}: PbContents: {hr:#x}"));
                }
            }
            let relocs = if nrelocs > 0 {
                let r = data(get_obj(c, 12, "RgRelocs")?)?;
                if r.len() != nrelocs as usize * 10 {
                    return Err(format!(
                        "{name}: {} bytes of {nrelocs} relocations",
                        r.len()
                    ));
                }
                r
            } else {
                Vec::new()
            };
            let (bytes, size) = if name == ".drectve" {
                let mut b = bytes;
                b.extend_from_slice(directives);
                let n = u32::try_from(b.len()).unwrap_or(0);
                (b, n)
            } else {
                (bytes, size)
            };
            sections.push(Section {
                name,
                characteristics,
                bytes,
                size,
                relocs,
                nrelocs,
            });
        }

        // Symbols: ICOFFSymTabRO: CSymbols 3, GetCOFFSymRO 4, ISYMNext 5.
        let tab = get_obj(coff, 13, "GetSymbolTableRO")?;
        let nsyms = get_u32(tab, 3, "CSymbols")?;
        let mut strings: Vec<u8> = vec![0; 4];
        let mut symtab: Vec<u8> = Vec::new();
        let mut isym = 0u32;
        while isym < nsyms {
            let mut s = std::ptr::null_mut();
            // SAFETY: GetCOFFSymRO(DWORD, ICOFFSymRO**).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hr>(tab, 4)(
                    tab, isym, &raw mut s,
                )
            };
            if hr < 0 || s.is_null() {
                return Err(format!("GetCOFFSymRO({isym}): {hr:#x}"));
            }
            if symtab.len() / 18 != isym as usize {
                return Err(format!("symbol {isym} at {}", symtab.len() / 18));
            }
            // ICOFFSymRO: CAuxSym 3, ISection 4, StorageClass 6, SzName 8,
            // Type 10, Value 11, EnumAuxSym 12.
            let mut naux = 0u8;
            let mut class = 0u8;
            let mut section = 0i32;
            let mut ty = 0u16;
            // SAFETY: getters of a BYTE*, BYTE*, int*, WORD*.
            unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *mut u8) -> Hr>(s, 3)(
                    s,
                    &raw mut naux,
                );
                slot::<unsafe extern "system" fn(*mut c_void, *mut u8) -> Hr>(s, 6)(
                    s,
                    &raw mut class,
                );
                slot::<unsafe extern "system" fn(*mut c_void, *mut i32) -> Hr>(s, 4)(
                    s,
                    &raw mut section,
                );
                slot::<unsafe extern "system" fn(*mut c_void, *mut u16) -> Hr>(s, 10)(
                    s,
                    &raw mut ty,
                );
            }
            let value = get_u32(s, 11, "Value")?;
            let n = name(get_obj(s, 8, "SzName")?)?;
            // The name: inline (8 bytes), else an offset in the strings.
            let mut rec = [0u8; 18];
            if n.len() <= 8 {
                rec[..n.len()].copy_from_slice(n.as_bytes());
            } else {
                let off = u32::try_from(strings.len()).unwrap_or(0);
                strings.extend_from_slice(n.as_bytes());
                strings.push(0);
                rec[4..8].copy_from_slice(&off.to_le_bytes());
            }
            rec[8..12].copy_from_slice(&value.to_le_bytes());
            rec[12..14].copy_from_slice(&(section as i16).to_le_bytes());
            rec[14..16].copy_from_slice(&ty.to_le_bytes());
            rec[16] = class;
            rec[17] = naux;
            symtab.extend_from_slice(&rec);
            // Its auxiliary records (IEnumAuxSym::Next; ICOFFAuxSym::ImgAuxSym, slot 4).
            if naux > 0 {
                let e = get_obj(s, 12, "EnumAuxSym")?;
                for _ in 0..naux {
                    let mut a = std::ptr::null_mut();
                    let mut got = 0u32;
                    // SAFETY: Next(ULONG, ICOFFAuxSym**, ULONG*).
                    let hr = unsafe {
                        slot::<
                            unsafe extern "system" fn(
                                *mut c_void,
                                u32,
                                *mut *mut c_void,
                                *mut u32,
                            ) -> Hr,
                        >(e, 3)(e, 1, &raw mut a, &raw mut got)
                    };
                    if hr < 0 || got == 0 || a.is_null() {
                        return Err(format!("{n}: aux: {hr:#x}"));
                    }
                    let mut aux = [0u8; 64];
                    // SAFETY: ImgAuxSym(BYTE*): an 18-byte record, into 64.
                    let hr = unsafe {
                        slot::<unsafe extern "system" fn(*mut c_void, *mut u8) -> Hr>(a, 4)(
                            a,
                            aux.as_mut_ptr(),
                        )
                    };
                    if hr < 0 {
                        return Err(format!("{n}: ImgAuxSym: {hr:#x}"));
                    }
                    // (The back end's ImgAuxSym leaves it empty: a section
                    // definition's record is built from its fields, as
                    // link.exe reads them. ICOFFAuxSym: Length 5, CRelocs 6,
                    // CLineNo 7, CheckSum 8, ISection 9, SelType 10.)
                    if class == 3 && section > 0 && n.starts_with('.') {
                        let length = get_u32(a, 5, "Length")?;
                        let relocs = get_u32(a, 6, "CRelocs")?;
                        let lines = get_u32(a, 7, "CLineNo")?;
                        let checksum = get_u32(a, 8, "CheckSum")?;
                        let mut number = 0i32;
                        let mut selection = 0u8;
                        // SAFETY: getters of an int* and a BYTE*.
                        unsafe {
                            slot::<unsafe extern "system" fn(*mut c_void, *mut i32) -> Hr>(a, 9)(
                                a,
                                &raw mut number,
                            );
                            slot::<unsafe extern "system" fn(*mut c_void, *mut u8) -> Hr>(a, 10)(
                                a,
                                &raw mut selection,
                            );
                        }
                        aux = [0u8; 64];
                        aux[0..4].copy_from_slice(&length.to_le_bytes());
                        aux[4..6].copy_from_slice(&(relocs as u16).to_le_bytes());
                        aux[6..8].copy_from_slice(&(lines as u16).to_le_bytes());
                        aux[8..12].copy_from_slice(&checksum.to_le_bytes());
                        aux[12..14].copy_from_slice(&(number as u16).to_le_bytes());
                        aux[14] = selection;
                        aux[16..18].copy_from_slice(&((number >> 16) as u16).to_le_bytes());
                    }
                    // A weak external's: its default (TagIndex, 11) and
                    // its search characteristics (where Misc.TotalSize is:
                    // MiscTotalSize, 14), as link.exe reads them.
                    if class == 105 {
                        let tag = get_u32(a, 11, "TagIndex")?;
                        let search = get_u32(a, 14, "MiscTotalSize")?;
                        aux = [0u8; 64];
                        aux[0..4].copy_from_slice(&tag.to_le_bytes());
                        aux[4..8].copy_from_slice(&search.to_le_bytes());
                    }
                    symtab.extend_from_slice(&aux[..18]);
                }
            }
            let mut next = 0u32;
            // SAFETY: ISYMNextCOFFSymRO(ICOFFSymRO*, DWORD*).
            let hr =
                unsafe {
                    slot::<unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u32) -> Hr>(
                        tab, 5,
                    )(tab, s, &raw mut next)
                };
            isym = if hr >= 0 && next > isym {
                next
            } else {
                isym + 1 + u32::from(naux)
            };
        }
        let total = u32::try_from(strings.len()).unwrap_or(0);
        strings[..4].copy_from_slice(&total.to_le_bytes());

        // The file: header, section headers, each section's bytes and
        // relocations, symbols, strings.
        let nsec = sections.len();
        let mut at = 20 + 40 * nsec;
        let mut headers = Vec::new();
        let mut body = Vec::new();
        let mut long_names = Vec::new();
        for s in &sections {
            let mut h = [0u8; 40];
            if s.name.len() <= 8 {
                h[..s.name.len()].copy_from_slice(s.name.as_bytes());
            } else {
                long_names.push(s.name.clone());
                let off = strings.len();
                strings.extend_from_slice(s.name.as_bytes());
                strings.push(0);
                let tag = format!("/{off}");
                h[..tag.len()].copy_from_slice(tag.as_bytes());
            }
            h[16..20].copy_from_slice(&s.size.to_le_bytes());
            if !s.bytes.is_empty() {
                h[20..24].copy_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
                body.extend_from_slice(&s.bytes);
                at += s.bytes.len();
            }
            if s.nrelocs > 0 {
                h[24..28].copy_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
                h[32..34]
                    .copy_from_slice(&u16::try_from(s.nrelocs).unwrap_or(u16::MAX).to_le_bytes());
                body.extend_from_slice(&s.relocs);
                at += s.relocs.len();
            }
            h[36..40].copy_from_slice(&s.characteristics.to_le_bytes());
            headers.extend_from_slice(&h);
        }
        if !long_names.is_empty() {
            let total = u32::try_from(strings.len()).unwrap_or(0);
            strings[..4].copy_from_slice(&total.to_le_bytes());
        }
        let mut file = Vec::new();
        file.extend_from_slice(&0x8664u16.to_le_bytes());
        file.extend_from_slice(&u16::try_from(nsec).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&u32::try_from(symtab.len() / 18).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&0u16.to_le_bytes());
        file.extend_from_slice(&0u16.to_le_bytes());
        file.extend_from_slice(&headers);
        file.extend_from_slice(&body);
        file.extend_from_slice(&symtab);
        file.extend_from_slice(&strings);
        Ok(file)
    }
}
