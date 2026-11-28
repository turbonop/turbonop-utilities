//! The compiler's back end (`c2.dll`), driven for link-time code generation
//! through the interface `link.exe` uses (docs/turbolink-c2.md): open each
//! `/GL` object, read its publics and directives, generate code for the
//! whole program, and read the generated object back as COFF.
//!
//! This is the one place turbolink needs unsafe code: the back end is a
//! DLL with COM-style interfaces, so every call goes through its objects'
//! vtables (by the slots `hexcavator vtables` reads from its public
//! symbols), and the objects it calls back on are ours, laid out as COM
//! objects. Nothing else in the linker calls it except through the safe
//! functions here.
//!
//! Windows x64 only: elsewhere `Backend::load` reports that LTCG isn't
//! available.

/// One public of a `/GL` object, as the back end reports it.
#[derive(Clone, Debug)]
pub struct Public {
    /// Its name.
    pub name: Vec<u8>,
    /// Its kind (`SymType`): 1 a reference, 2 an external definition, 4
    /// data, 6 a weak alias (`??_E`, a vector deleting destructor, for
    /// its `??_G`), 8 a static.
    pub kind: u32,
    /// The back end's handle for it (what code generation's roots list).
    pub handle: u64,
    /// Whether it's a COMDAT, and its selection.
    pub comdat: Option<u8>,
    /// What a definition refers to (`RgISymRefs`): handles of the object's
    /// publics, definitions and references (none for a reference).
    pub refs: Vec<u64>,
    /// Its size in bytes (`IPublicSym3::Size`; 0 for a reference).
    pub size: u32,
    /// A weak alias's default (`SzNameDefaultSym`: kind 6).
    pub default: Option<Vec<u8>>,
}

/// A `/GL` object the back end opened.
#[derive(Clone, Debug)]
pub struct IlObject {
    /// Its publics.
    pub publics: Vec<Public>,
    /// Its linker directives (`.drectve`'s text).
    pub directives: Vec<u8>,
}

/// The three lists of handles code generation (`CodeGen6`) takes, as
/// link.exe makes them (docs/turbolink-c2.md).
#[derive(Clone, Copy, Debug, Default)]
pub struct Lists<'a> {
    /// The IL definitions something outside the IL refers to.
    pub roots: &'a [u64],
    /// The IL's references that are imports called directly (a thunk's
    /// name, not `__imp_`): the back end calls them through the import
    /// address table instead.
    pub imports: &'a [u64],
    /// With `/OPT:REF`, the IL's COMDAT definitions nothing reaches: the
    /// back end leaves them out.
    pub dead: &'a [u64],
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub use imp::Backend;

#[cfg(not(all(windows, target_arch = "x86_64")))]
/// The back end (not available on this platform).
pub struct Backend;

#[cfg(not(all(windows, target_arch = "x86_64")))]
impl Backend {
    /// Not available here.
    pub fn load(_: &std::path::Path, _: &[&str]) -> Result<Backend, String> {
        Err("link-time code generation needs Windows x64 (the compiler's back end)".into())
    }
    /// Not available here.
    pub fn open(&mut self, _: &std::path::Path, _: u32, _: u32) -> Result<IlObject, String> {
        Err("unavailable".into())
    }
    /// Not available here.
    pub fn generate(&mut self, _: &[&str], _: &Lists<'_>) -> Result<Vec<u8>, String> {
        Err("unavailable".into())
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code, clippy::cast_possible_truncation)]
// (x64 only: usize and u64 are one size, so the casts don't truncate.)
mod imp {

    use std::ffi::c_void;
    use std::path::Path;

    use super::{IlObject, Lists, Public};

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
    const IID_PUBLIC_SYM2: Guid = Guid(
        0x14c0_a371,
        0x7254,
        0x4eb5,
        [0xb8, 0x6e, 0xc0, 0xd4, 0xdb, 0x84, 0x47, 0x20],
    );
    const IID_PUBLIC_SYM3: Guid = Guid(
        0x370e_c183,
        0x8a62,
        0x4df7,
        [0x82, 0x16, 0x4c, 0xdd, 0xb5, 0x90, 0xf0, 0x40],
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
    type FnRelease = unsafe extern "system" fn(*mut c_void) -> u32;

    fn qi(obj: *mut c_void, iid: &Guid) -> Result<*mut c_void, String> {
        let mut out = std::ptr::null_mut();
        // SAFETY: slot 0 of every COM object.
        let hr = unsafe { slot::<FnQi>(obj, 0)(obj, iid, &raw mut out) };
        if hr < 0 || out.is_null() {
            return Err(format!("QueryInterface: {hr:#x}"));
        }
        Ok(out)
    }

    fn release(obj: *mut c_void) {
        // SAFETY: slot 2 of every COM object.
        unsafe { slot::<FnRelease>(obj, 2)(obj) };
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

    /// A link data object's bytes, up to its NUL.
    fn bytes_to_nul(obj: *mut c_void) -> Result<Vec<u8>, String> {
        let mut d = data(obj)?;
        if let Some(end) = d.iter().position(|&c| c == 0) {
            d.truncate(end);
        }
        Ok(d)
    }

    /// What the public `sym` (a definition) refers to: `IPublicSym2`'s
    /// `CRefs` (slot 9) and `RgISymRefs` (slot 10), as link.exe reads them.
    fn references(sym: *mut c_void) -> Result<Vec<u64>, String> {
        let sym2 = qi(sym, &IID_PUBLIC_SYM2)?;
        let n = get_u32(sym2, 9, "CRefs");
        let refs = n.and_then(|n| {
            if n == 0 {
                return Ok(Vec::new());
            }
            // (The caller's buffer, through a pointer to it, as link.exe
            // passes its own.)
            let mut buffer = vec![0u64; n as usize];
            let mut list: *mut u64 = buffer.as_mut_ptr();
            // SAFETY: RgISymRefs(ULONGLONG**): the back end writes `CRefs`
            // handles to the buffer `*list` points at (`n` of them).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *mut *mut u64) -> Hr>(sym2, 10)(
                    sym2,
                    &raw mut list,
                )
            };
            if hr < 0 {
                return Err(format!("RgISymRefs: {hr:#x}"));
            }
            Ok(buffer)
        });
        release(sym2);
        refs
    }

    /// The public `sym`'s size: `IPublicSym3::Size` (slot 11), as
    /// link.exe reads it.
    fn public_size(sym: *mut c_void) -> Result<u32, String> {
        let sym3 = qi(sym, &IID_PUBLIC_SYM3)?;
        let size = get_u32(sym3, 11, "Size");
        release(sym3);
        size
    }

    // -----------------------------------------------------------------
    // Our objects, which the back end calls.

    #[repr(C)]
    struct LinkData {
        vtbl: *const usize,
        bytes: *const u8,
        len: i32,
    }

    unsafe extern "system" fn qi_none(_: *mut c_void, _: *const Guid, out: *mut *mut c_void) -> Hr {
        // SAFETY: an out pointer.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
    unsafe extern "system" fn add_ref(_: *mut c_void) -> u32 {
        2
    }
    unsafe extern "system" fn release_ours(_: *mut c_void) -> u32 {
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

    fn leak_vtbl(slots: Vec<usize>) -> *const usize {
        slots.leak().as_ptr()
    }

    /// A link data object holding `bytes` (leaked: the back end may keep it).
    fn link_data(bytes: Vec<u8>) -> *mut c_void {
        use std::sync::OnceLock;
        static V: OnceLock<usize> = OnceLock::new();
        let vtbl = *V.get_or_init(|| {
            leak_vtbl(vec![
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release_ours as *const () as usize,
                get_data_pointer as *const () as usize,
            ]) as usize
        }) as *const usize;
        let b = bytes.leak();
        Box::into_raw(Box::new(LinkData {
            vtbl,
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

    // IMemCoordinator: Register, Unregister, NotifyOOM.
    unsafe extern "system" fn mem_register(_: *mut c_void, handler: *mut c_void) -> Hr {
        // SAFETY: the back end's handler: AddRef it, as link.exe does.
        unsafe { slot::<FnRelease>(handler, 1)(handler) };
        S_OK
    }
    unsafe extern "system" fn mem_unregister(_: *mut c_void, handler: *mut c_void) -> Hr {
        // SAFETY: the back end's handler: Release it, as link.exe does.
        unsafe { slot::<FnRelease>(handler, 2)(handler) };
        S_OK
    }
    unsafe extern "system" fn mem_notify_oom(_: *mut c_void) -> Hr {
        S_OK
    }

    /// The back end's error messages, collected (from any of its threads).
    static MESSAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    // ILinkNotify2: OnError, OnStatus, AddExternalFilenames.
    unsafe extern "system" fn notify_error(_: *mut c_void, err: *mut c_void) -> Hr {
        // ILinkError2's slot 6, Sz(WCHAR**): the message.
        let mut sz: *const u16 = std::ptr::null();
        // SAFETY: the back end's error object.
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
        if let Ok(mut m) = MESSAGES.lock() {
            m.push(text);
        }
        S_OK
    }
    unsafe extern "system" fn notify_status(_: *mut c_void, _: u32, _: u32) -> Hr {
        S_OK
    }
    unsafe extern "system" fn notify_filenames(_: *mut c_void, _: *mut c_void) -> Hr {
        S_OK
    }

    static OURS: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

    // The linker's code generation object (link.exe's CLinkCG): only
    // QueryInterface, for the memory coordinator and the notifications.
    unsafe extern "system" fn cg_qi(_: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hr {
        let &(coordinator, notify) = OURS.get().expect("set before Init");
        // SAFETY: an IID and an out pointer.
        unsafe {
            *out = if *iid == IID_MEM_COORDINATOR {
                coordinator as *mut c_void
            } else if *iid == IID_LINK_NOTIFY2 {
                notify as *mut c_void
            } else {
                std::ptr::null_mut()
            };
            if (*out).is_null() {
                E_NOINTERFACE
            } else {
                S_OK
            }
        }
    }

    fn linker_object() -> *mut c_void {
        let coordinator = Box::into_raw(Box::new(Plain {
            vtbl: leak_vtbl(vec![
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release_ours as *const () as usize,
                mem_register as *const () as usize,
                mem_unregister as *const () as usize,
                mem_notify_oom as *const () as usize,
            ]),
        })) as *mut c_void;
        let notify = Box::into_raw(Box::new(Plain {
            vtbl: leak_vtbl(vec![
                qi_none as *const () as usize,
                add_ref as *const () as usize,
                release_ours as *const () as usize,
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
                release_ours as *const () as usize,
            ]),
        })) as *mut c_void
    }

    /// The back end, loaded and initialized for one link.
    pub struct Backend {
        h6: *mut c_void,
        /// The opened objects (the first one's ICOFFObj is the generated code).
        files: Vec<*mut c_void>,
    }

    // (The back end's objects are used from the thread that made them; the
    // pointers only travel with the Backend.)
    unsafe impl Send for Backend {}

    fn messages() -> String {
        MESSAGES.lock().map(|m| m.join("; ")).unwrap_or_default()
    }

    impl Backend {
        /// Loads `c2` and initializes it as link.exe does (`Init` with
        /// `switches`, then `SetPdbDll` with the `mspdbcore.dll` beside it).
        pub fn load(c2: &Path, switches: &[&str]) -> Result<Backend, String> {
            let path = wide(&c2.display().to_string());
            // SAFETY: a NUL-terminated path; its dependencies found beside
            // it (LOAD_WITH_ALTERED_SEARCH_PATH).
            let module = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), 8) };
            if module.is_null() {
                // SAFETY: no arguments.
                let e = unsafe { GetLastError() };
                return Err(format!("{}: can't load (error {e})", c2.display()));
            }
            // SAFETY: a module and a NUL-terminated name.
            let get = unsafe { GetProcAddress(module, c"DllGetObjHandler".as_ptr().cast()) };
            if get.is_null() {
                return Err(format!("{}: no DllGetObjHandler", c2.display()));
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
            let cg = linker_object();
            let init_switches: Vec<Vec<u16>> = switches.iter().map(|s| wide(s)).collect();
            let mut init_argv: Vec<*const u16> = init_switches.iter().map(Vec::as_ptr).collect();
            let init_argc = i32::try_from(init_argv.len()).unwrap_or(0);
            // SAFETY: IObjHandler6::Init (slot 4): (IUnknown*, int, int argc,
            // WCHAR** argv), as link.exe calls it.
            let hr = unsafe {
                slot::<
                    unsafe extern "system" fn(
                        *mut c_void,
                        *mut c_void,
                        i32,
                        i32,
                        *mut *const u16,
                    ) -> Hr,
                >(h6, 4)(h6, cg, 0, init_argc, init_argv.as_mut_ptr())
            };
            if hr < 0 {
                return Err(format!("the back end's Init: {hr:#x} {}", messages()));
            }
            let h9 = qi(h6, &IID_OBJ_HANDLER9)?;
            // (By its full path, beside c2.dll: link.exe names it plainly,
            // found beside itself, in the toolset's directory; turbolink
            // isn't there.)
            let pdb_dll = c2.with_file_name("mspdbcore.dll");
            let pdb = wide(&pdb_dll.display().to_string());
            let mut out: *mut u16 = std::ptr::null_mut();
            // SAFETY: IObjHandler9::SetPdbDll (slot 14): (const WCHAR*, WCHAR**).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut u16) -> Hr>(
                    h9, 14,
                )(h9, pdb.as_ptr(), &raw mut out)
            };
            if hr < 0 {
                return Err(format!("SetPdbDll: {hr:#x}"));
            }
            Ok(Backend {
                h6,
                files: Vec::new(),
            })
        }

        /// Opens the `/GL` object at `offset` in `path` (`size` bytes: a
        /// library's member is opened in the library): its publics and
        /// directives.
        pub fn open(&mut self, path: &Path, offset: u32, size: u32) -> Result<IlObject, String> {
            let w = wide(&path.display().to_string());
            // The object's token: link.exe's is its object's address; ours,
            // a block of its own (opaque to the back end).
            let token = Box::leak(vec![0u8; 4096].into_boxed_slice()).as_ptr() as u64;
            let mut file = std::ptr::null_mut();
            let mut flags = 0u32;
            let machine: u16 = if self.files.is_empty() { 0 } else { 0x8664 };
            // SAFETY: IObjFileForFile2 (slot 11): (WCHAR* path, WORD
            // machine, DWORD offset, DWORD size, ULONGLONG token, kind,
            // IObjFile2**, DWORD* flags), as link.exe calls it.
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
                >(self.h6, 11)(
                    self.h6,
                    w.as_ptr(),
                    machine,
                    offset,
                    size,
                    token,
                    0,
                    &raw mut file,
                    &raw mut flags,
                )
            };
            if hr < 0 || file.is_null() {
                return Err(format!(
                    "{}: the back end can't open it: {hr:#x} {}",
                    path.display(),
                    messages()
                ));
            }
            // Its publics: IObjFile2::EnumPublics (slot 5), IEnumPublics::Next.
            let mut e = std::ptr::null_mut();
            // SAFETY: EnumPublics(IEnumPublics**, ULONGLONG token).
            let hr = unsafe {
                slot::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void, u64) -> Hr>(file, 5)(
                    file, &raw mut e, token,
                )
            };
            if hr < 0 || e.is_null() {
                return Err(format!("{}: EnumPublics: {hr:#x}", path.display()));
            }
            let mut publics = Vec::new();
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
                // IPublicSym: FComdat 3, ISym 4, SzName 6, SymType 7.
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
                let mut is = 0i32;
                let mut selection = 0u8;
                // SAFETY: FComdat(int*, BYTE*).
                let hr =
                    unsafe {
                        slot::<unsafe extern "system" fn(*mut c_void, *mut i32, *mut u8) -> Hr>(
                            sym, 3,
                        )(sym, &raw mut is, &raw mut selection)
                    };
                let comdat = (hr >= 0 && is != 0).then_some(selection);
                let name = bytes_to_nul(get_obj(sym, 6, "SzName")?)?;
                let (refs, size) = if kind == 1 {
                    (Vec::new(), 0)
                } else {
                    (references(sym)?, public_size(sym)?)
                };
                let default = if kind == 6 {
                    Some(bytes_to_nul(get_obj(sym, 8, "SzNameDefaultSym")?)?)
                } else {
                    None
                };
                publics.push(Public {
                    name,
                    kind,
                    handle,
                    comdat,
                    refs,
                    size,
                    default,
                });
                release(sym);
            }
            let directives = bytes_to_nul(get_obj(file, 8, "PbDirectives")?)?;
            self.files.push(file);
            Ok(IlObject {
                publics,
                directives,
            })
        }

        /// Generates code (`CodeGen6`) with the back end's `switches` and
        /// `lists`, and returns the generated object as a COFF file.
        pub fn generate(
            &mut self,
            switches: &[&str],
            lists: &Lists<'_>,
        ) -> Result<Vec<u8>, String> {
            let h10 = qi(self.h6, &IID_OBJ_HANDLER10)?;
            let switches: Vec<Vec<u16>> = switches.iter().map(|s| wide(s)).collect();
            let mut argv: Vec<*const u16> = switches.iter().map(Vec::as_ptr).collect();
            let handles =
                |list: &[u64]| link_data(list.iter().flat_map(|r| r.to_le_bytes()).collect());
            let roots_data = handles(lists.roots);
            let imports_data = handles(lists.imports);
            let dead_data = handles(lists.dead);
            let (mut out1, mut out2, mut out3) = (
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let mut flag = 0i32;
            // SAFETY: IObjHandler10::CodeGen6 (slot 15): (int, WCHAR**,
            // ILinkDataRO* x3, ILinkDataRO** x2, int*, ILinkDataRO**), as
            // link.exe calls it.
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
                    imports_data,
                    dead_data,
                    &raw mut out1,
                    &raw mut out2,
                    &raw mut flag,
                    &raw mut out3,
                )
            };
            if hr < 0 {
                return Err(format!("code generation failed: {hr:#x} {}", messages()));
            }
            let first = *self.files.first().ok_or("no /GL objects")?;
            let coff = qi(first, &IID_COFF_OBJ)?;
            write_coff(coff)
        }
    }

    // -----------------------------------------------------------------
    // The generated object, as a COFF file.

    struct Section {
        name: Vec<u8>,
        characteristics: u32,
        bytes: Vec<u8>,
        size: u32,
        relocs: Vec<u8>,
        nrelocs: u32,
    }

    #[allow(clippy::too_many_lines)]
    fn write_coff(coff: *mut c_void) -> Result<Vec<u8>, String> {
        // ICOFFObj: CContrib 4, GetContribForISec 10, GetSymbolTableRO 13.
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
            let name = bytes_to_nul(get_obj(c, 14, "SzSectionName")?)?;
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
                    return Err(format!("PbContents: {hr:#x}"));
                }
            }
            let relocs = if nrelocs > 0 {
                let r = data(get_obj(c, 12, "RgRelocs")?)?;
                if r.len() != nrelocs as usize * 10 {
                    return Err(format!("{} bytes of {nrelocs} relocations", r.len()));
                }
                r
            } else {
                Vec::new()
            };
            // (Its `.drectve` is blank: the objects' directives were the
            // link's already, from their stand-ins.)
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
            let n = bytes_to_nul(get_obj(s, 8, "SzName")?)?;
            let mut rec = [0u8; 18];
            if n.len() <= 8 {
                rec[..n.len()].copy_from_slice(&n);
            } else {
                let off = u32::try_from(strings.len()).unwrap_or(0);
                strings.extend_from_slice(&n);
                strings.push(0);
                rec[4..8].copy_from_slice(&off.to_le_bytes());
            }
            rec[8..12].copy_from_slice(&value.to_le_bytes());
            rec[12..14].copy_from_slice(&(section as i16).to_le_bytes());
            rec[14..16].copy_from_slice(&ty.to_le_bytes());
            rec[16] = class;
            rec[17] = naux;
            symtab.extend_from_slice(&rec);
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
                        return Err(format!("auxiliary record: {hr:#x}"));
                    }
                    let mut aux = [0u8; 18];
                    // (ImgAuxSym leaves its buffer empty: a section
                    // definition's and a weak external's records are built
                    // from their fields, as link.exe reads them. ICOFFAuxSym:
                    // Length 5, CRelocs 6, CLineNo 7, CheckSum 8, ISection 9,
                    // SelType 10, TagIndex 11, MiscTotalSize 14.)
                    if class == 3 && section > 0 && n.starts_with(b".") {
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
                        aux[0..4].copy_from_slice(&length.to_le_bytes());
                        aux[4..6].copy_from_slice(&(relocs as u16).to_le_bytes());
                        aux[6..8].copy_from_slice(&(lines as u16).to_le_bytes());
                        aux[8..12].copy_from_slice(&checksum.to_le_bytes());
                        aux[12..14].copy_from_slice(&(number as u16).to_le_bytes());
                        aux[14] = selection;
                        aux[16..18].copy_from_slice(&((number >> 16) as u16).to_le_bytes());
                    }
                    if class == 105 {
                        let tag = get_u32(a, 11, "TagIndex")?;
                        let search = get_u32(a, 14, "MiscTotalSize")?;
                        aux[0..4].copy_from_slice(&tag.to_le_bytes());
                        aux[4..8].copy_from_slice(&search.to_le_bytes());
                    }
                    symtab.extend_from_slice(&aux);
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

        // The file: header, section headers, contents and relocations,
        // symbols, strings.
        let nsec = sections.len();
        let mut at = 20 + 40 * nsec;
        let mut headers = Vec::new();
        let mut body = Vec::new();
        for s in &sections {
            let mut h = [0u8; 40];
            if s.name.len() <= 8 {
                h[..s.name.len()].copy_from_slice(&s.name);
            } else {
                let off = strings.len();
                strings.extend_from_slice(&s.name);
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
        let total = u32::try_from(strings.len()).unwrap_or(0);
        strings[..4].copy_from_slice(&total.to_le_bytes());
        let mut file = Vec::new();
        file.extend_from_slice(&0x8664u16.to_le_bytes());
        file.extend_from_slice(&u16::try_from(nsec).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&u32::try_from(at).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&u32::try_from(symtab.len() / 18).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&headers);
        file.extend_from_slice(&body);
        file.extend_from_slice(&symtab);
        file.extend_from_slice(&strings);
        Ok(file)
    }
}
