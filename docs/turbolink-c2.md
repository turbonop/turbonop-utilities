# turbolink and LTCG: the link.exe ↔ c2.dll interface

Link-time code generation (`cl /GL`, `link /LTCG`) is `link.exe` handing
the compiler's back end, `c2.dll`, the intermediate language in `/GL`
objects and taking back machine code. For turbolink to link `/GL`
objects it has to have the same conversation with `c2.dll`, which is
undocumented. This is that conversation as observed for toolset
14.52.37025 (x64), and how hard supporting it looks.

**Result.** The interface has been driven without `link.exe`.
`ltcg-drive` (below) loads `c2.dll`, hands it `/GL` objects, has it
generate code, and writes the code as an ordinary COFF object. Linked by
`link.exe` *without* `/LTCG`, five test programs behave exactly as
`link.exe`'s own LTCG builds of them do:
- one translation unit;
- two translation units;
- `/GL` code with a native object;
- a C++ DLL with exports, a static initializer, RTTI, and an exception
  thrown across it.

(`ltcg-drive` doesn't drive a library of `/GL` objects or incremental
LTCG. turbolink does libraries: below.)

**turbolink links `/GL` objects itself.** `turbolink /LTCG` drives
`c2.dll` (see "turbolink's LTCG" below). These link and run as
`link.exe /LTCG`'s builds of them do, with the same output and exit
code:
- one C object;
- two C objects, one's function inlined into the other's;
- C++ with an exception, virtual calls and a static constructor;
- `/GL` code with a native object;
- a library of `/GL` objects:
  - its members call one another and a native member of theirs;
  - one member is reached only from a native library's member;
  - one is never loaded;
  - an import is called directly;
  - also with a native object first;
- C++ from a library of `/GL` objects (virtual calls, exceptions), with
  and without `/DEBUG`;
- functions and data nothing reaches, with `/OPT:REF` and `/OPT:NOREF`;
- a `/GL` DLL whose exports are `dllexport`s, loaded by a native
  program.

Each image is the same size as `link.exe`'s. The bytes are not yet the
same:
- 208 to 493 differ in the 9.5 KB C images (3,284 to 3,565 before the
  three lists and `Init`'s switches were link.exe's);
- 1,299 in the 12 KB C++ one (4,997 before);
- 2,580 in the two objects' `/DEBUG` build (6,175 before);
- 8,000 to 12,000 in the C++ library and `/DEBUG` ones.

The private repository's `ltcg_programs_run_as_link_exes` and
`ltcg_libraries_run_as_link_exes` check them. Through the logging probe,
turbolink's `Init` switches, the objects it opens (their order, offsets
and sizes) and `CodeGen6`'s three lists are link.exe's in eleven links:
the C library twice, C++, the unreached ones four ways (`/OPT:REF`,
`/DEBUG /OPT:REF`, `/OPT:NOREF`, a DLL), the C++ library with and
without `/DEBUG`, two objects, and the DLL. The one difference is below
("Not yet done").

## turbolink's LTCG

`crates/link/linker/src/c2.rs` holds all of the COM calls (the crate's
only `unsafe` code). `ltcg.rs` holds the link logic, as a `Host` that
symbol resolution (`resolve::resolve_with`, through its `Ltcg` trait)
drives. There is one resolve, as link.exe has one pass:

1. **Find the back end, when a `/GL` object first loads.** It uses
   `TURBOLINK_C2` if that is set. Otherwise it takes `c2.dll` from the
   first `PATH` directory that also has `link.exe` or `cl.exe` (a
   developer prompt's `bin\Hostx64\x64`). A link with no `/GL` object
   never loads it.
2. **Open each `/GL` object as it loads**, a command-line input or a
   library's member that the search loads. The back end is loaded with
   link.exe's `Init` switches (`-optref` with `/OPT:REF`, `-msilDebug`
   with `/DEBUG`), then `SetPdbDll` with the full path of the
   `mspdbcore.dll` beside it (a bare name fails with `E_FAIL`). Each
   object is opened through `IObjFileForFile2`, a member in its library
   at its data's offset. Its publics (`EnumPublics`: name, `SymType`,
   COMDAT selection, handle; a definition's references, `RgISymRefs`,
   and size; a weak alias's default) and its directives
   (`PbDirectives`) are read.
3. **A stand-in in its place.** Each `/GL` object is resolved as a
   generated COFF *stand-in*. Each definition becomes a section of its
   own: COMDAT with the object's selection if it is a COMDAT, plain
   otherwise. References are undefined symbols, and the directives go
   in `.drectve`. Statics and weak aliases are left out. The stand-ins'
   references load library members as any object's do (a `/GL` member
   too).
4. **Generate code, once the search is over.** `CodeGen6` gets `-GA`
   for an EXE or `-GD` for a DLL and three lists, as link.exe's
   ("The three lists", below). The generated `ICOFFObj` comes back as a
   COFF object in memory.
5. **Link the result.** The stand-ins leave the link (what they defined
   is undefined again; what only they referred to is no longer wanted).
   The generated object joins it as its first file, named
   `* CIL library *(* CIL module *)` as link.exe names it. The libraries
   are searched again for what it refers to, and the link proceeds as a
   native one. Members loaded for the IL's references stay, as with
   link.exe.

### What link.exe does with a library of `/GL` objects

All [obs], from `/VERBOSE` and the probe, for the test programs above:

- **One pass.** A `/GL` member is found and loaded like any member
  (`Found lib_a`, `Loaded gl.lib(la.obj)`), for IL references and native
  ones alike, in the search's order. Pass 1's search loads all of them,
  then `Generating code`, `Finished generating code`, then one more
  `Searching libraries` round (for the generated code's references)
  before pass 2.
- **The generated object is first.** Pass 2's module list starts with
  `* CIL library *(* CIL module *)`, before every native object, even
  with a native object first on the command line. The map agrees: the
  generated functions are laid out before a native object's that comes
  first on the command line.
- **Members are opened as they load:** the command line's objects, then
  each member in load order. Each member is opened as
  `IObjFileForFile2("gl.lib", 0x8664, <its data's offset>, <its size>,
  ...)`, after a `RequestPchSymbol("gl.lib", offset, size, ...)` that
  returns `E_NOTIMPL`. turbolink skips that call. In link.exe's calls,
  the size and kind arguments' upper 32 bits are garbage (DWORDs in
  64-bit slots).

### The three lists

`CodeGen6`'s three input buffers are arrays of handles. What link.exe
puts in them, as observed [obs] and as turbolink now makes them (the
same lists in all eleven links compared):

1. **The roots:** the IL definitions that a native object names (an
   undefined symbol, or a copy of a COMDAT of its own, even one that
   won: link.exe roots the IL's vtables and RTTI when a native library
   member also has them), the entry point, the exports (`dllexport`s
   among them) and the `/INCLUDE`s. It goes by name: the IL's copy that
   won, else its first. A DLL with nothing exported and no entry point
   has none, and link.exe goes on.
2. **The imports called directly:** the IL's references (kind 1) that
   resolved to an import's thunk, a function imported without
   `__declspec(dllimport)` (`GetCurrentProcessId`, and vcruntime's
   `_CxxThrowException`, `__CxxFrameHandler4`, `_purecall`...). The back
   end then calls them through the import address table:
   `__imp_GetCurrentProcessId` is what the generated code refers to.
   `__imp_` references aren't listed.
3. **The dead,** with `/OPT:REF` only (empty with `/OPT:NOREF`, and
   with `/DEBUG` unless `/OPT:REF` is given): the IL's COMDATs and its
   data (kind 4) that nothing reaches from the roots and the other
   definitions (non-COMDAT functions and data, kind 2, and statics,
   kind 8). Reaching goes through what each definition refers to
   (`RgISymRefs`). A weak alias (kind 6) leads to its default, and a
   reference to the copy that won. Examples:
   - `la_also` and `lib_b_unused` (unreferenced functions in loaded
     members);
   - `dead_leaf`, called only by the dead `dead_root`: so it is
     transitive;
   - an inline `helper` called only by `dead_root`;
   - the unused `_OptionsStorage` of `__local_stdio_scanf_options` (kind
     4, though not a COMDAT);
   - not `g_dead_data`, a plain global (kind 2, not a COMDAT).

   The back end leaves them out. Without the list it keeps them: before
   this list was passed, turbolink's own `/OPT:REF` dropped them from
   the image.

Facts this rests on [obs]:
- **`IPublicSym2`** is `{14c0a371-7254-4eb5-b86e-c0d4db844720}`. link.exe
  asks each definition for it, then calls `CRefs` (slot 9) and
  `RgISymRefs` (slot 10). `RgISymRefs(ULONGLONG**)` fills the *caller's*
  buffer through the pointer it's given. With a null pointer it returns
  `S_OK` and writes nothing.
- **Handles are unique within an object only.** A reference's handle
  can equal another object's reference's (`lib_a` in `main.obj` and
  `nat_in_lib` in `lb.obj` shared one). A definition's references are
  its own object's publics, except that a COMDAT's copy can refer to
  another object's (`std` inline functions in the C++ library test: the
  back end seems to keep one copy's). Handle 0 is an empty reference
  (throw information's unused fields).
- **`SymType` 6** is a weak alias: `??_EShape@@UEAAPEAXI@Z` (a vector
  deleting destructor), whose default (`SzNameDefaultSym`) is
  presumably the `??_G` scalar one [inf]. The vtable refers to `??_E`.
- **`IPublicSym3::Size`** is the definition's size: 24 for a
  three-slot vtable, whose native copy's section is 32 bytes (the RTTI
  locator's pointer before it).

`/GL` objects without `/LTCG` are linked with LTCG anyway, with warning
`LNK4197`, as `link.exe` does (it restarts the link). `/LTCG:STATUS` and
`NOSTATUS` are accepted. `/LTCG:INCREMENTAL`, PGO and `/LTCGOUT` are
still "not yet". If `c2.dll` cannot be found or fails, the link stops
with `LNK1257` and the back end's messages.

**Not yet done:**
- **The dead list, in one case.** In the C++ library test, `main` refers
  to a handle (its last reference, after two empty ones, in a
  `try`/`catch`) that no object's publics list. turbolink then passes
  no dead list, which is safe: the back end keeps what link.exe would
  have it drop. link.exe lists three there.
- **Code-generation switches** beyond `-GA`/`-GD` (the earlier DLL
  probe's `-ThrowingNew`, and the rest of the matrix);
- incremental LTCG and PGO;
- `/cgthreads`;
- byte-identical images;
- the overlap of native work with `CodeGen6`;
- reading the `ICOFFObj` directly, instead of writing it and reading it
  back;
- one copy of the COM wrapper: `ltcg-drive` still has its own.

## Sources

Every fact below is marked by where it comes from:

- **[sym]** Microsoft's public symbols for `c2.dll` and `link.exe`:
  decorated names, and vtables read slot by slot with `hexcavator
  vtables`.
- **[obs]** Seen in `turbonop-ltcg-probe`'s logs of real `link /LTCG`
  runs.
- **[drive]** Confirmed by `ltcg-drive` working.
- **[inf]** Inferred: consistent with everything seen, not proven.

Nothing here comes from Microsoft's source code or internal documents.

## The tools

- **`hexcavator vtables <image>`** (`crates/analysis/program/src/vtables.rs`)
  lists every MSVC vftable with each slot's function. It works from
  public symbols alone: slots come from base relocations, and an
  unsized table's length is inferred (an upper bound). Folded functions
  (`/OPT:ICF`) are reported as "one of n".
- **`turbonop-ltcg-probe`** (`crates/link/ltcg-probe`) is a stand-in
  `c2.dll` that logs the whole conversation:
  - It puts a proxy in front of every interface pointer passed in
    either direction. All proxies share one table of generic
    forwarding thunks.
  - It names each call from the `hexcavator vtables -json` output of
    both binaries.
  - It shows the arguments: strings, GUIDs, link data's bytes, and
    symbol handles by name.

  It's installed in a *copy* of the toolset's `bin` directory, with the
  real back end renamed `c2real.dll`.
- **`ltcg-drive`** (same crate) is LTCG without `link.exe`: the proof of
  concept.
- **`tools/ltcg-probe`** has the test programs and `run.cmd`, which
  builds them and links each one through the probe. Its outputs and
  logs are this machine's and stay out of the repository.

## Getting in

`c2.dll` exports `DllGetObjHandler`, `InvokeCompilerPass(W)` (the
compiler driver's entry point), `AbortCompilerPass`,
`CloseTypeServerPDB` and `UtcBld*`. Of these, `link.exe` looks up only
`DllGetObjHandler` [sym].

```
HRESULT DllGetObjHandler(REFCLSID, REFIID, void**)          [sym]
  CLSID {0cb3fe38-d9a5-4dab-ac9b-d6b6222653c2}               [obs][drive]
  IID   {0737f2d8-976c-4a67-a950-456cc5a65db4} = IObjHandler6 [obs][drive]
```

The interfaces are COM-style, generated by MIDL from an `objint.idl`
(`__MIDL___MIDL_itf_objint_*` enumerations) [sym]. Methods return
`HRESULT`s, and `E_NOTIMPL` is common: the folded stubs are `E_NOTIMPL`
bodies [obs].

### Interface IDs

| IID | Interface | |
|---|---|---|
| `0737f2d8-976c-4a67-a950-456cc5a65db4` | IObjHandler6 | [obs][drive] |
| `447e3a28-b77b-4d2e-bb4c-6fdb1622c57f` | IObjHandler9 | [obs][drive] |
| `512179b7-ae57-40a2-ac1e-5986586a6e4d` | IObjHandler10 | [obs][drive] |
| `a9470bb0-911f-11d1-a712-0060083e8c78` | ICOFFObj | [obs][drive] |
| `f23aafa9-3655-4b33-a84d-1eebfce232d3` | IPublicSym5 | [obs] |
| `370ec183-8a62-4df7-8216-4cddb590f040` | IPublicSym3 | [obs][drive] |
| `14c0a371-7254-4eb5-b86e-c0d4db844720` | IPublicSym2 | [obs][drive] |
| `47bfade0-1ca7-4b72-83db-018c5a316b54` | IEnumExternalFilenames2 | [obs] |
| `02e37a58-b410-44f5-88e5-83fed6d99711` | IMemCoordinator (linker's) | [obs][drive] |
| `c0c21495-7642-4e6b-8181-94ca7789b241` | ILinkNotify2 (linker's) | [obs][drive] |

## The interfaces

Slot numbers include IUnknown's three: `QueryInterface`, `AddRef` and
`Release`. All come from [sym] (vtable reads) unless marked. "WCHAR" is
the binaries' `unsigned short`.

### IObjHandler6/9/10: the back end's (c2)

| slot | method | |
|---|---|---|
| 3 | `CodeGen(int argc, WCHAR** argv, ILinkDataRO*, ILinkDataRO**)` | |
| 4 | `Init(IUnknown* linker, int, int argc, WCHAR** argv)` | [obs][drive] |
| 5 | `IObjFileForFile(WCHAR* path, WORD, DWORD, DWORD, ULONGLONG, enum, IObjFile2**)` | |
| 6 | `OnCtrlC()` | |
| 7 | `LoadPchObject(WCHAR*, DWORD, DWORD)` | |
| 8 | `CodeGen2(int, WCHAR**, ILinkDataRO*, ILinkDataRO*, ILinkDataRO**)` | |
| 9 | `Init(IUnknown*, int, int, int, WCHAR**)`: `E_NOTIMPL` | [obs] |
| 10 | `RequestPchSymbol(WCHAR* path, DWORD offset, DWORD size, enum, ILinkDataRO**)`: `E_NOTIMPL` | [obs] |
| 11 | `IObjFileForFile2(…as 5…, DWORD* flags)` | [obs][drive] |
| 12 | `CodeGen4(…)` (IObjHandler9 onward) | |
| 13 | `CodeGen5(…)` | |
| 14 | `SetPdbDll(const WCHAR* dll, WCHAR**)` | [obs][drive] |
| 15 | `CodeGen6(int argc, WCHAR** argv, ILinkDataRO* roots, ILinkDataRO* native, ILinkDataRO*, ILinkDataRO**, ILinkDataRO**, int*, ILinkDataRO**)` (IObjHandler10) | [obs][drive] |

**Slots 0–11** are IObjHandler6's. IObjHandler9's table continues with
`CodeGen4`, `CodeGen5` and `SetPdbDll`, and IObjHandler10's adds
`CodeGen6`. These are the same three objects (`CC2::XObjHandler6`, `9`
and `10`) [sym][obs].

**The last slots of each table** hold `CodeGen3` and a `CodeGenCore`,
in a different order and with different signatures in each class.
They're probably the class's own virtual methods, not interface
members [inf]. That doesn't matter for calling the interface.

**Slots 9 and 10 of IObjHandler10** are a single folded stub.
IObjHandler6's table, which isn't folded there, names them [sym].

### IObjFile2: a `/GL` object, as the back end opened it

| slot | method | |
|---|---|---|
| 3 | `Characteristics(DWORD*)` or `FNative(int*)` (folded) | |
| 4 | `CloseCache(int)` | |
| 5 | `EnumPublics(IEnumPublics**, ULONGLONG token)` | [obs][drive] |
| 6 | `FNative`/`Characteristics` | |
| 7 | `Machine(WORD*)`: 0x8664 | [obs] |
| 8 | `PbDirectives(ILinkDataRO**)`: the object's linker directives | [obs][drive] |
| 9 | `EnumExternalFilenames(IEnumExternalFilenames**)`: its compile PDB | [obs] |
| 10 | `CContrib` in c2's table, `CloseCoffCon(int)` in link's | unresolved |

### IPublicSym, IPublicSym3, IPublicSym5: one public of a `/GL` object

| slot | method | |
|---|---|---|
| 3 | `FComdat(int* is, BYTE* selection)` | [obs] |
| 4 | `ISym(ULONGLONG*)`: the symbol's **handle**, the token for it in CodeGen's lists | [obs][drive] |
| 5 | `TkFile(ULONGLONG*)` | |
| 6 | `SzName(ILinkDataRO**)` | [obs][drive] |
| 7 | `SymType(enum*)` | [obs][drive] |
| 8 | `SzNameDefaultSym(ILinkDataRO**)`: a weak external's default | [obs] |
| 9, 10 | `CRefs(DWORD*)`, `RgISymRefs(ULONGLONG**)` (IPublicSym2 onward): the caller's buffer, filled | [obs][drive] |
| 11 | `Size(DWORD*)` (IPublicSym3 onward) | [obs] |
| 12 | `SzCoffSectionName(ILinkDataRO**)` (IPublicSym4 onward) | |
| 13 | `CxxModuleInfo(int*, int*, int*, ILinkDataRO**)` (IPublicSym5): C++ modules | [obs] |

`SymType` values [obs]:

| value | meaning | examples |
|---|---|---|
| 1 | a reference | `__imp_*`, `__security_check_cookie` |
| 2 | an external definition | `main`; COMDATs such as `printf` (selection 2, "any") |
| 4 | data | a static local of an inline function (`_OptionsStorage`): its exact meaning is [inf]; the dead list takes it as a COMDAT |
| 6 | a weak alias | `??_EShape@@UEAAPEAXI@Z`, a vector deleting destructor; `SzNameDefaultSym` names its default |
| 8 | a static | `square` |

IEnumPublics and the other enumerators follow COM's order: `Next(n,
T**, ULONG* fetched)`, `Skip`, `Reset`, `Clone`. That's proven where
the slots aren't folded [sym][obs].

### ICOFFObj: the generated code, as a COFF object

You get it by QueryInterface on the *first* `/GL` object's IObjFile2,
after code generation. There is one for the whole program, however many
`/GL` objects there were [obs][drive].

| slot | method | |
|---|---|---|
| 4 | `CContrib(DWORD*)`: its sections | [obs][drive] |
| 7 | `CSymbols(DWORD*)` | [obs] |
| 9 | `EnumContrib(IEnumContrib**)` | [obs] |
| 10 | `GetContribForISec(DWORD isec, IObjectContrib**)`: by 1-based section number | [drive] |
| 11 | `FImportObj(int*)` | [obs] |
| 13 | `GetSymbolTableRO(ICOFFSymTabRO**)` | [obs][drive] |
| 14 | `GetSymbolTableRW(ICOFFSymTabRW**)`: the linker sets symbols' values and sections through this | [obs] |
| 15 | `ObjtkFromSzSymbol(ILinkDataRO*, ULONGLONG*)` | |
| 17 | `SzRefForIsym(ULONGLONG, ILinkDataRO**)` | |
| 3, 5, 6, 8, 12, 16, 18 | `BeginOptCaching`, `Close`, `COFFTimeStamp`, `EndOptCaching`, `GetStringTable`, `TimeStamp`, `IncrInit`: all one `E_NOTIMPL` stub in c2 | [sym][obs] |

link.exe's implementation's table gives these slots' order [sym].
Because `GetStringTable` isn't implemented, names come one at a time.

### IObjectContrib: one section of the generated object

| slot | method |
|---|---|
| 3 | `CbContents` |
| 4 | `Characteristics` |
| 5 | `CLineNo` |
| 6 | `CRelocs` |
| 7 | `ISection` |
| 8 | `PbContents(BYTE*, DWORD, DWORD*)` |
| 9, 10 | `PbContentsRO(ILinkDataRO**)`, `PbContentsRW(ILinkDataRW**)` |
| 11 | `RgLineNo(ILinkDataRO**)` |
| 12 | `RgRelocs(ILinkDataRO**)`: `IMAGE_RELOCATION`s, 10 bytes each [drive] |
| 13 | `RgRelocsEx(BYTE*, DWORD)` |
| 14 | `SzSectionName(ILinkDataRO**)` |
| 15 | `TkFile` |
| 16 | `VirtualAddress` |

All [sym]; the `[drive]` mark is the driver's confirmation.

### ICOFFSymTabRO/RW, ICOFFSymRO/RW, ICOFFAuxSym: its symbol table

**ICOFFSymTabRO** [sym][drive]:

| slot | method |
|---|---|
| 3 | `CSymbols` |
| 4 | `GetCOFFSymRO(DWORD isym, ICOFFSymRO**)` |
| 5 | `ISYMNextCOFFSymRO(ICOFFSymRO*, DWORD*)` |
| 6 | `EmitCOFFSymbolTable(BYTE*, DWORD)` |
| 7 | `EnumCOFFSymRO` |

Symbol indexes are raw COFF numbering: they count auxiliary records,
and relocations use them as they are [obs][drive].

**ICOFFSymRO** [sym][drive]:

| slot | method |
|---|---|
| 3 | `CAuxSym(BYTE*)` |
| 4 | `ISection(int*)` |
| 5 | `ISym` |
| 6 | `StorageClass(BYTE*)` |
| 7 | `SzFile` |
| 8 | `SzName(ILinkDataRO**)` |
| 9 | `TkFile` |
| 10 | `Type(WORD*)` |
| 11 | `Value(DWORD*)` |
| 12 | `EnumAuxSym(IEnumAuxSym**)` |

The read-write version (ICOFFSymRW) adds `SetISection`, `SetType` and
`SetValue`.

**ICOFFAuxSym** [sym]:

| slot | method |
|---|---|
| 4 | `ImgAuxSym(BYTE*)`: returns success but leaves the buffer empty [drive] |
| 5 | `Length` |
| 6 | `CRelocs` |
| 7 | `CLineNo` |
| 8 | `CheckSum` |
| 9 | `ISection(int*)`: a COMDAT's associated section |
| 10 | `SelType(BYTE*)` |
| 11 | `TagIndex` |
| 14 | `MiscTotalSize`: a weak external's search characteristics |
| 19 | `AuxType` |
| 13, 15, 16, 17 | one folded body for `AryDim`, `FcnPLineNo`, `FcnPNextFcn` and `MiscSize`, in both binaries: identical bodies, so which is which doesn't matter |

Because `ImgAuxSym` leaves its buffer empty, a section definition's
record (`Length`, `CRelocs`, `CLineNo`, `CheckSum`, `ISection`,
`SelType`) and a weak external's (`TagIndex`, `MiscTotalSize`) are
built from their fields, as link.exe reads them [obs][drive].

### What the linker implements for the back end

- **CLinkCG**, passed to `Init`, has only IUnknown's methods. The back
  end asks it for two interfaces [obs][drive]:
  - **IMemCoordinator** (slot 3 `Register(IMemHandler*)`, 4
    `Unregister`, 5 `NotifyOOM`). The back end registers two handlers:
    its `CMemHandler` and an `LtcgMapHandler`.
  - **ILinkNotify2** (slot 3 `OnError(ILinkError2*)`, 4
    `OnStatus(DWORD done, DWORD total)` for progress, 5
    `AddExternalFilenames(IEnumExternalFilenames2*)`).
- **ILinkError2** (the back end's) has `ErrorType`, `Objtk1`, `Objtk2`,
  `Sz(WCHAR**)` (the message) and `DwInfo`. An internal compiler error
  arrives through it [drive].
- **ILinkDataRO** (both sides') is a buffer: slot 3
  `GetDataPointer(BYTE**, int*)` [sym][obs][drive].
- link.exe also has `IObjFile2`/`ICOFFObj` implementations for native
  objects (`CObjFileT<IMAGE_FILE>`, `CImportObject`) and a
  `CLinkNativeSymbol`. **No test passed any of them to the back end**
  [obs]. What needs them (PGO? other options?) is unknown.

## A link, call by call

What `link.exe /LTCG` does [obs], and `ltcg-drive` repeats [drive]:

1. **`DllGetObjHandler`** returns IObjHandler6.
2. **`Init`.** It tries slot 9, the five-argument `Init`, which returns
   `E_NOTIMPL`, then calls slot 4, `Init(CLinkCG, 0, argc, argv)`. The
   argument vector is the linker's switches for the back end:
   `["-msilDebug"]` with `/DEBUG`, `["-optref"]` with `/OPT:REF` (the
   default without `/DEBUG`), `["-optref" "-msilDebug"]` with both, and
   `[]` with `/OPT:NOREF` [obs]; with `/LTCG:INCREMENTAL`,
   `["-msilDebug" "-incremental" "-ipdb<file>.ipdb" "-iobj<file>.iobj"
   "-tsiobj<timestamp>"]`. During `Init` the back end gets
   IMemCoordinator from CLinkCG and registers its two handlers.
3. **`SetPdbDll("mspdbcore.dll")`**, through IObjHandler9.
4. **Each `/GL` object:**
   - It's opened with `IObjFileForFile2(path, machine, offset, size,
     token, kind, &obj, &flags)`:
     - `machine` is 0 for the first object and 0x8664 after it.
     - `offset` and `size` are the object's place in its file: a
       library's member is opened at its offset inside the `.lib`,
       after a `RequestPchSymbol` that returns `E_NOTIMPL`.
     - `token` is the linker's identifier for the object: link.exe's is
       a pointer, and the back end echoes it back. A zeroed block of our
       own works as well [drive].
     - `kind` was 0 in every test.
     - `flags` came back as 2.
   - The back end reads the object file itself.
   - The linker reads, through IObjFile2: the external file names (the
     compile PDB), `Machine`, `PbDirectives`, and every public, each
     with its `SymType`, handle, COMDAT status, name, C++ module
     information and size. From these it resolves symbols together with
     the native objects and libraries.
5. **`CodeGen6`**, through IObjHandler10:
   - **argc and argv** are the back end's code-generation switches:
     `["-GA"]` for an EXE, `["-GD" "-ThrowingNew"]` for the test DLL.
     *How link.exe's options map to these is not yet surveyed.*
   - **The first buffer**, `roots`, is an array of symbol handles: the
     IL definitions referenced from outside the IL [obs]. Examples:
     `main` (the CRT's startup code calls it), `gl_plus_one` (called by
     the native object), and the DLL's exports and the EH and RTTI data
     that native code uses.
   - **The second buffer** is an array of handles of the IL's
     references that resolved to imports' thunks (functions imported
     without `dllimport`): `__CxxFrameHandler4`, `_CxxThrowException`,
     `_purecall`, `GetCurrentProcessId` [obs]. The back end then calls
     them through `__imp_`. `ltcg-drive` passes none, and its output
     still works [drive]. See "The three lists".
   - **The third buffer**, with `/OPT:REF`, is the IL's COMDATs and
     data that nothing reaches (empty in the first tests, which all
     had `/DEBUG`). See "The three lists".
   - **The outputs** are two buffers of 12-byte records {object token,
     DWORD}, an `int` (0) and an empty buffer [obs]. Their meaning is
     unknown. `ltcg-drive` ignores them, and the result links and runs.
   - **Code generation runs on the back end's worker threads.**
     `OnStatus` progress calls arrive from several threads [obs], so the
     linker's callbacks must be thread-safe.
6. **The generated code** is the first object's `ICOFFObj` [obs][drive]:
   - Sections: `.text$mn`, `.rdata`, `.pdata`/`.xdata`, `.debug$S`/`$T`,
     `.chks64`, and `.CRT$XCU`, `.data`, `.bss` and EH sections for the
     DLL.
   - Relocations and the symbol table.
   - The linker treats this as one more native object. Its `.drectve`
     is blank, so the directives come from the inputs' `PbDirectives`.
   - link.exe writes symbol values and sections back through the
     read-write symbol table.
7. **Release** everything; the back end unregisters its memory handler.

## What's unknown

- **The switch mapping.** Which `-G*` and other back-end switches link
  options produce: `/OPT:*`, `/GUARD:*`, `/CETCOMPAT`, `/Brepro`,
  `/DEBUG` kinds, DLL against EXE, and what `-ThrowingNew` follows. The
  remedy is the probe over a matrix of link options: a few hours.
- **The outputs of `CodeGen6`.** The {token, DWORD} records, the
  `int`, the fourth buffer.
- **What the dead list can't name:** a handle a definition refers to
  that no object's publics list ("Not yet done").
- **`IObjFile2` slot 10.** It's `CContrib` in c2 but `CloseCoffCon` in
  link.
- **`SymType` 4's meaning,** and the values not seen yet.
- **What hands native objects to the back end.** Neither
  `CLinkNativeSymbol` nor link.exe's `ICOFFObj`s were ever passed over.
- **PGO** (`/GENPROFILE`, `/USEPROFILE`, through `pgodb140.dll`): out of
  turbolink's first version. **ARM64EC/ARM64X** (`IObjFileHybrid`,
  `IArm64XObj`): not observed.
- **Where the back end looks for its message DLL.** It found `clui.dll`
  only when the host program sat in the toolset's directory (`1033\` is
  beside it) [obs]: possibly the host executable's directory, not its
  own. Run from elsewhere, `Init` failed with `C1510`. turbolink would
  have to run from there, or we'd need to find another way to load it.
- **Our object's debug information.** For the two-object program,
  `link.exe` warns `LNK4255` about the written object ("multiple objects
  of the same name"; it is linked as if without debug information). Its
  `.debug$S` presumably needs the treatment link.exe gives it.

## How hard is LTCG for turbolink?

**Feasible: proven for the main path.** The interface is understood
well enough to drive: an EXE from one or more `/GL` objects, `/GL` with
native objects, and a C++ DLL with EH. It's small:

- About fifteen interfaces matter.
- Their method order comes from public symbols automatically, so a new
  toolset's layout is checked by rerunning `hexcavator vtables`, and
  any change in it is visible.
- The IIDs the conversation uses are known.

What turbolink has to do:

1. Open each `/GL` object (or library member) with the back end, and
   add its publics to the symbol table next to the native objects'.
2. Resolve symbols, then hand `CodeGen6` its roots and switches.
3. Read the generated `ICOFFObj` and link it as a native input. It can
   be read directly through the interface, with no COFF file round
   trip.

**Speed.** Code generation is the back end's, so it's the floor. While
`CodeGen6` runs (it blocks one of our threads and uses its own), the
native objects' work can proceed: resolution, layout of the native
sections, and their PDB type merging. That overlap is what turbolink
can win over `link.exe` in an LTCG link.

**Risks:**

- **Toolset churn.** Mitigated by the automatic vtable check. The IIDs
  and the call sequence would need the probe rerun on each new toolset.
- **The switch mapping,** above.
- **The message DLL's location,** above.
- **Fidelity in the corners:** errors from the back end, incremental
  LTCG's files, `/GUARD:CF` and CET with LTCG, and very large programs
  (memory, and many objects).

**Estimate, in agent time.** Assumes turbolink's native linking exists:

- **Basic LTCG** (open, resolve, generate, consume, for EXEs and DLLs,
  libraries included): about a day. `ltcg-drive` is most of the
  conversation already. *Done, libraries and the three lists included.*
- **The switch matrix and the unknown outputs** with the probe: a few
  hours.
- **Incremental LTCG, error paths, and the option corners:** a few
  days.
- **The real bottleneck** is a corpus of real-world `/GL` programs
  (built here, kept private) to compare against `link.exe`. Writing
  the code is not.
