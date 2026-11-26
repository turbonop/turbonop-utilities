# rtti-strip: plan

rtti-strip (working name) makes our own C++ binaries smaller by replacing
the names in their RTTI type descriptors with short synthetic ones. The
RTTI data stays: `dynamic_cast`, `typeid`, `type_info::operator==`,
`type_index`, and exception handling keep working. What goes is the
decorated type name (`.?AV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@`
and so on) in each descriptor, which in a large template-heavy program
adds up to hundreds of kilobytes.

This is for binaries we build and control, not a general-purpose tool.
Compatibility only has to hold for the program it's run on, under the
contract in [The contract](#the-contract). That's what makes it
shippable where a general compiler or linker change was not.

It runs after the link, so it works on LTCG builds, which matter most. It
comes in two modes:

- **Pad** (phase 1): write each short name in place and zero the rest of
  the descriptor. The image keeps its size; the compressed download
  shrinks.
- **Compact** (phase 3): raise the image, remove the freed bytes, and
  lower it again, fixing every reference and the PDB. This is the first
  user of a general post-link binary rewriter, which is on the roadmap
  for other transformations anyway.

This is a plan, not a specification. Each phase becomes TODO.md items,
and items leave TODO.md as they ship.

## Status (2026-11-26)

| Phase | State |
|---|---|
| 0 Measurements | Not yet. Two facts are already settled: [the runtime's comparisons](#how-the-runtime-compares-types) and [the FIXUP stream](#the-fixup-stream). |
| 1 Pad mode (MSVC x64, then arm64 and x86) | Not yet. |
| 2 Compact in place, if the layout allows it | Not yet. |
| 3 The rewriter, and compact mode | Not yet. |
| 4 ELF and Mach-O | Not yet; [notes](#elf-and-mach-o). |

## How the runtime compares types

Read in the vcruntime sources Visual Studio ships (MSVC 14.52.37025,
`VC\Tools\MSVC\14.52.37025\crt\src\vcruntime`):

| Where | What it compares |
|---|---|
| `std_type_info.cpp`, `__std_type_info_compare` (`operator==`, `!=`, `before`) | Pointers, then `strcmp(lhs + 1, rhs + 1)`: from the **second** byte. |
| `std_type_info.cpp`, `__std_type_info_hash` (`hash_code`) | FNV-1a over the name from the second byte. |
| `std_type_info.cpp`, `__std_type_info_name` (`name()`) | Undecorates the name; returns `nullptr` if that fails. |
| `frame.cpp`, `TypeMatchHelper` (catch) | Descriptor pointers, then `strcmp` of the whole name. |
| `rtti.cpp`, `TypeidsEqual` (`dynamic_cast`, multiple and virtual inheritance) | Pointers, then `strcmp` of the whole name. |
| `rtti.cpp`, `FindSITargetTypeInstance` (`dynamic_cast`, single inheritance) | A loop by pointer; if it finds nothing, a second loop by `strcmp` alone ("in case pCOLocator comes from a different image"). |
| `ehhelpers.cpp`, `_is_exception_typeof` | `strcmp` alone. |

The linker keeps one `??_R0<name>@8` COMDAT per name, so within an image
each type has one descriptor and **equal types always match by pointer**.
Unequal types always reach `strcmp`. So:

- **Within an image, names only have to be distinct.** They don't have to
  be anything in particular. A collision makes two types equal: a wrong
  `catch`, or a `dynamic_cast` that succeeds where it should fail (type
  confusion). Collisions must be impossible, not unlikely: the tool
  checks.
- **Every name needs one fixed lead byte**, then a unique tail, because
  `type_info` comparison skips the first byte. Names `"A"` and `"B"` would
  be equal under `operator==`.
- **Across images**, equal types match only by name. That's
  [the contract](#the-contract)'s business.
- `type_index` keeps working: equality is unchanged; `hash_code` and
  `before` give different values, so the iteration order of
  `std::map<type_index, …>` and `std::unordered_map<type_index, …>`
  changes from an unprocessed build.
- `name()` returns `nullptr` for a synthetic name unless the name is valid
  decorated syntax. Breaking `name()` and `raw_name()` is accepted.

## The contract

A program may be processed only if no comparison crosses from a processed
module to an unprocessed one, except the ones rtti-strip preserves names
for. Concretely, none of `typeid`, `dynamic_cast`, or a `type_info`
comparison may have one side's type information in a module that wasn't
processed with this one, unless that type's name is preserved.

Code that does this without saying so:

- **In the STL headers (14.52.37025), compiled into our module but acting
  on objects whose vtables are in `msvcp140.dll`:**
  - `<regex>`, `regex_traits::transform_primary` compares
    `typeid(*_Coll)` with `typeid(collate<_Elem>)` and
    `typeid(collate_byname<_Elem>)`. The locale's collate facets are
    created inside `msvcp140.dll` (certain for `wchar_t`, `wlocale.cpp`;
    likely the same for `char`). Renamed, the comparison fails and
    `transform_primary` returns an empty string: equivalence classes
    (`[[=a=]]`) stop matching. Silent.
  - `<ostream>`, `std::print`/`println` to an `ostream&`, does
    `dynamic_cast<filebuf*>(rdbuf())` to find a Unicode console. `cout`'s
    `filebuf` is `msvcp140.dll`'s (`cout.cpp`: `static filebuf
    fout(stdout)`). Renamed, the cast fails and output to the console
    takes the non-Unicode path. Silent.
- **Header-only, and fine within a module, but crossing if the object
  came from another of our DLLs:** `std::any_cast`, `shared_ptr`'s
  `get_deleter`, `std::function::target` and `target_type`,
  `std::rethrow_if_nested`, `dynamic_pointer_cast`, the `<future>`
  deleter cast, the `<ostream>` `syncbuf` casts. Casts that are meant to
  fail stay correct (no false match is possible).
- `msvcp140.dll`'s own sources (`crt\src\stl`) contain no `typeid` or
  `dynamic_cast`. So the crossings above all come from header code in our
  modules acting on the DLL's objects.

### What rtti-strip preserves

Two rules, both automatic:

1. **Exception handling.** Every descriptor reachable from a
   `CatchableType` (throw side), a `HandlerType` (catch side), or an
   exception specification's type list. This covers throwing to and
   catching from any module, including ones the tool never reads.
2. **Types the runtime DLLs also have.** Read the RTTI of every
   unprocessed C++ runtime DLL the program imports (`msvcp140*.dll`,
   `vcruntime140*.dll`, `concrt140.dll`, …) and preserve every name of
   ours that also appears there. This covers the two `msvcp140.dll`
   cases above and any future ones without a list.

Rule 2 reads the DLLs at processing time. Their RTTI names are fixed by
the ABI (frozen since 2015), so a newer redistributable on the user's
machine is unlikely to add a type that matters; app-local deployment of
the runtime removes the question. Linking the runtime statically removes
it too (it is then processed with everything else).

The tool reports which names it preserved, and why.

## Synthetic names

A name is a lead byte (`.`, as today), a tail of bytes from 1 to 255 (no
NUL; the runtime only uses `strcmp`), and a NUL. Base 255 carries about
7.99 bits a byte.

On x64 and arm64 a TypeDescriptor is 16 bytes of header (`pVFTable`,
`spare`) and the name; descriptors are probably 8-aligned (phase 0
checks). The slot sizes that matter:

| Slot | Name | Tail bits |
|---|---|---|
| 24 bytes | lead + 6 + NUL | ~47.9 |
| 32 bytes | lead + 14 + NUL | ~111 |

A 64-bit hash needs 9 bytes, which lands in the 32-byte slot; there all
14 bytes are free, so a 64-bit hash buys nothing. On x86 the header is 8
bytes; its alignment is phase 0's to check.

Two naming schemes:

- **Dense.** Numbers in the order the tool finds descriptors (sorted by
  original name, for determinism). Always 24 bytes; no collisions. Names
  differ across separately processed modules.
- **Hashed.** A hash of the original name, 6 bytes (24-byte slots) or 14
  (32-byte slots). The same type gets the same name in every module
  without shared state, which makes comparisons across our own DLLs work.
  With 10,000 types in a 48-bit space, the chance of any collision is
  about 2×10⁻⁷; the tool checks the whole set it processes and refuses on
  a collision (`--hash-bytes 14` then). The hash is a fixed, documented
  function, so a later run on another DLL agrees.

Either way, no synthetic name may equal a preserved name (the tool
checks; the tail range can also exclude the bytes real decorated names
start with, `?` and the type codes, so they can't meet).

Descriptors that are already short (`.H` for `int`) are left alone when
the synthetic name wouldn't be shorter.

A sidecar map (synthetic name to original) is written next to the PDB,
for crash triage and for our own tools (the Dump Analyzer, Hexcavator)
to show real names.

## Finding the descriptors

Every descriptor starts with `pVFTable = &type_info::vftable`, an absolute
pointer with a base relocation. So: walk `.reloc` for fixups whose target
is `type_info`'s vftable, and check that `spare` is 0 and a `.` name
follows. Cross-check with:

- the complete object locators reached from vftables (RVAs on x64 and
  arm64), their class hierarchies and base class descriptors;
- the EH tables reached from `.pdata` and `.xdata` (FH3 `FuncInfo`, FH4's
  compressed tables) and the `ThrowInfo`s;
- the PDB's publics: `??_R0…@8` (descriptors), `_TI…`, `_CTA…`, `_CT…`
  (throw side). On x64 a `ThrowInfo` is reached only by a RIP-relative
  `lea` with no base relocation, so the PDB (or the FIXUP stream) is how
  the throw side is found exactly.

Anything that doesn't check out is left alone and reported.

## The FIXUP stream

Linking with `/DEBUGTYPE:CV,FIXUP` (with `/DEBUG`) adds a FIXUP stream to
the PDB: the DBI optional debug header's third index. `/DEBUG:FULL` alone
does not; checked with link 14.52.37029 on a two-function test. (There is
no `/DEBUGTYPE:FULL`; `/DEBUG:FULL` is the separate full-PDB-versus-fastlink
choice.)

The records are 12 bytes, `u16 type, u16 flags, u32 rva, u32 target_rva`,
with the COFF relocation type. The test's four were `DIR64` (a data
pointer), two `REL32` (the RIP-relative loads; their RVAs are the
instructions' displacements), and one `ADDR32NB` (the debug directory).
So it does record RIP-relative references, which base relocations don't.
Whether it records every kind the rewriter needs (`.pdata`, `.xdata`, EH
tables, CFG tables, arm64's `ADRP`/`ADD` pairs and branch ranges, under
LTCG) is phase 0's to check. `crates/symbols/pdb` knows the stream's
index (`dbi.rs`) but doesn't parse it yet.

## Phase 0: measurements

- **A corpus with RTTI.** turbolib's LLVM corpus builds with
  `LLVM_ENABLE_RTTI` off. Use our own program's release builds (LTCG), and
  one public template-heavy program with RTTI.
- **The prize.** Total name bytes in `.data$r`; the image's size against
  the padded image's compressed size (zip, zstd, the installer's
  compressor).
- **Layout.** Descriptor alignment on x64, arm64, and x86; where `.data$r`
  falls within `.data` under LTCG, and what follows it.
- **FIXUP coverage.** Every reference kind above, on an LTCG build, on x64
  and arm64.
- **FH4's encoding**, enough to find each `HandlerType`'s descriptor.

## Phase 1: pad mode

- Read the PE (`crates/formats/pe`), the PDB, and the imported runtime
  DLLs; find the descriptors; compute the preserved set; assign names;
  write them in place with the rest of each slot zeroed; recompute the
  checksum. In parallel throughout (descriptor discovery and hashing per
  section shard).
- Output is deterministic: same input, same bytes.
- **The checker.** A separate pass that reads the output and proves:
  every descriptor's name is distinct; every preserved name is unchanged;
  nothing outside the name bytes changed; every name parses as
  lead + tail + NUL.
- **The test program.** A suite built in every configuration (x64, arm64,
  x86; `/MD` and `/MT`; LTCG and not; FH3 and FH4) and run before and
  after: catches by value, reference, and pointer, across a DLL boundary
  both ways and from `msvcp140.dll` (`bad_alloc`, `length_error`);
  `dynamic_cast` with single, multiple, and virtual inheritance, cross
  casts, and casts that must fail; `type_index` in maps; `any_cast`;
  `function::target`; `get_deleter`; `rethrow_if_nested`;
  `regex_traits::transform_primary`; `std::print` to `cout`. Every result
  must match the unprocessed build's (apart from `name()` and ordering).

## Phase 2: compact in place

If phase 0 finds `.data$r` at the end of `.data`'s initialized bytes
(only uninitialized data after it), compaction doesn't need the general
rewriter: move descriptors down, keep the section's virtual size (so
nothing after them moves), and cut its raw size. The file shrinks, the
image in memory doesn't. Only references to descriptors move: complete
object locators, base class descriptors, catchable and handler types,
`typeid(T)`'s `lea`s (from the FIXUP stream), absolute pointers (base
relocations), and the PDB's publics and section contributions. If
`.data$r` isn't last, this phase is skipped.

## Phase 3: the rewriter, and compact mode

The general tool: raise the image into sections, blocks, and references,
transform, lower. For compact mode the transform is "shrink these
descriptors". The parts:

- **Raising**, from the PE and the FIXUP stream, checked against base
  relocations, `.pdata`, the load config (CFG and EH-continuation tables),
  exports, imports, TLS, resources, and the debug directory. A reference
  that can't be accounted for fails the run.
- **Lowering**: lay out again, rewrite every reference, regenerate
  `.reloc`, `.pdata`, the CFG tables, and the directories.
- **The PDB.** First with OMAP (the `OmapToSrc`/`OmapFromSrc` streams,
  which map new addresses to old and are what DIA and dbghelp use for
  binaries rewritten after the link), so symbols, line tables, and frame
  data stay untouched. Later, optionally, rewriting them in place. Our own
  `crates/symbols/pdb` and symbolizer must apply OMAP too.

## ELF and Mach-O

Different enough to plan when they come up. In the Itanium ABI a
`type_info` holds a pointer to a separate name string (`_ZTS…`), so
shorter names don't move descriptors. libstdc++ compares names with
`strcmp` unless the name starts with `*` (then by pointer); libc++ has
several modes, one of which (Apple arm64) tags the pointer. Exported
`_ZTS`/`_ZTI` symbols put the mangled names in `.dynstr` anyway, so the
program also needs hidden visibility for the strings to be worth
removing.

## Open questions

- Whether the hashed scheme should default to 6 bytes or 14.
- Whether any of our own code calls `name()` or `raw_name()`. The tool
  can report imports of `__std_type_info_name`; `raw_name()` is inline
  and can't be seen that way.
