# The IL in MSVC's `/GL` objects

What `cl /GL` writes for `link /LTCG` to hand to its back end (c2.dll), as
far as it is understood. Read by `turbonop_pe::ltcg`
(crates/formats/pe/src/ltcg) and dumped by `hexcavator ltcg`.

The format is undocumented. Everything here was worked out from objects
MSVC 14.52 (19.52.37029) writes: small sources varied one thing at a time,
and a corpus of some 440 objects compiled from this repository's test
sources (tools/) for x64, x86 and ARM64, with `/O1`, `/O2`, `/Od`, `/Zi`,
C++14 to C++20 and C11. The corpus stayed on the build machine; the
fixtures in testdata/ltcg-il (tools/ltcg-il) and testdata/coff are what is
committed.

Confidence is stated as the evidence allows:

- **certain**: a field's reading is forced by the bytes (a walk that lands
  exactly on the next record, on every object in the corpus, and on
  sources varied to change only that field), or matches a tool's output
  exactly.
- **likely**: consistent with everything seen, but only seen in a few
  shapes, or its meaning is a guess that fits.
- **not known**: the bytes are skipped by a rule that holds on the corpus,
  but what they mean is not.

Two whole-corpus checks back the walks: every `.cil$gl`, `.cil$sy` and
`.cil$in` in the corpus walks record by record to exactly its end (443 of
443 objects), and lib.exe's symbol index for each object (`lib /OUT`, then
`dumpbin /linkermember:2`) is reproduced exactly by the rule in
"Public definitions" (30,494 public definitions, none different).

## Encodings (certain)

- `u8`, `u16`, `u32`: little-endian.
- **id**: a `u16`; if its top bit is set, another `u16` follows and the id is
  `(lo & 0x7FFF) | hi << 15`. Symbols, scopes and sections are named by ids.
- **c16** / **c32**: a byte below 0x80 is the value; 0x80 is followed by
  the value as `u16` / `u32`. (Bytes 0x81-0xFF never start one.)
- **leb**: unsigned LEB128 (only a type descriptor's type uses it).
- **sz**: NUL-terminated string. **lstr**: c16 length, then the bytes.

Which counts are c16 and which c32 was settled with sources of 150
parameters and 200 references: parameter counts and attribute counts are
c16, reference counts c32 (certain). The virtual-class count, scope counts
and directive count are `u8` as far as seen (likely: never above 127).

## The sections

| Section | Holds |
|---|---|
| `.drectve` | `-compiler:"<c2.dll>"` |
| `.cil$fg` | the back end's command line: `u32` count, then NUL-separated strings |
| `.cil$gl` | the global symbol table: records |
| `.cil$ex` | each function's IL (bytecode, not decoded) |
| `.cil$sy` | each function's scopes and locals |
| `.cil$in` | variables' initial values |
| `.cil$db` | CodeView type and id records |

## `.cil$gl`

A 30-byte header (contents not known), then records, each led by a tag
byte, one after another; the directives record ends the section.

| Tag | Record | Fields after the tag |
|---|---|---|
| 0x18 | predefined | c16, c16, c16 (meaning not known; always first) |
| 0x12 | source file | id, sz path, u8 checksum length, u8 kind (1 MD5, 2 SHA-1, 3 SHA-256), checksum |
| 0x09 | section | id, sz name, u8 flags (not known), sz class (`CODE`, `DATA`, `CONST`, or empty), u32 COFF characteristics (0 unless `#pragma section` gave them) |
| 0x1C | namespace | sz name, common fields, [attributes] |
| 0x0B | named type | u8 scope, id, sz name, u8 (0), c32 CodeView type index, common fields, [attributes] |
| 0x08 | constant | u8 scope, id, sz name, c32 type, value type (below), u8 0, u8 size, u8 length, that many value bytes |
| 0x1D | attribute | id, lstr namespace, lstr name, u8 argument count, each: u8 kind (0 a string), lstr text |
| 0x1E | (C++20) | five bytes, zero in every object seen |
| 0x01 | variable (scalar) | see below |
| 0x02 | variable (aggregate) | as 0x01, with an alignment byte after its type |
| 0x0E | function with a body | see below |
| 0x04 | function declared | as 0x0E, up to its id index |
| 0x10 | vector deleting destructor (declared) | as 0x04, then the id of its scalar deleting destructor |
| 0x0A | directives | u8 0, u8 0, u8 count, each: u8 kind (0x04 `/DEFAULTLIB`, 0x0A a linker option), sz text; ends the section |

Tags, framing, and the field order are certain for every row (the walk).
The meanings in parentheses are as stated; "scope" is 0 in `.cil$gl` and a
scope's number in `.cil$sy` (certain: the same records appear in both).

### A value's type (certain framing, likely meaning)

A byte **MT**: 0x80 always set; 0x40 means one more byte follows (seen
with `__declspec(align)`); the low four bits encode the alignment: 2 for
1 byte, 4, 6, 8, 0xA, 0xC, 0xE for 2 to 64 bytes (certain: varied
`align(8)` to `align(64)` and `char`/`int`/pointers). Then **TC**, the type's
class: 1 integer, 2 unsigned/`bool`, 3 pointer, 4 function pointer, 5
floating point, 6 aggregate, 7 `void`, 0x0D (unnamed temporaries) (likely:
by the types seen).

### Variables (0x01, 0x02)

`u8 scope, id, u8 flags, sz name, MT [byte], TC, u8 0, u8 W, u8 S, c32
size, common fields, c32 type, [u8 alignment if 0x02], references,
[attributes]`.

- flags: 0x26 compiler-made (RTTI, vftables), 0x25 string literals, 0x24
  internal linkage, else 0 (likely).
- W: 0x02 writable, 0x04/0x08 read-only (certain for `const`).
- S: 1 defined, 2 declared, 3 common, 4 static, 7 dllexport, 8 dllimport
  (certain: one source each).
- size: as the IL counts it: RTTI's image-relative fields count at pointer
  size (certain: RTTI records' sizes against their initializers).
- references: if the common fields' extra has 0x0080 (a dynamic
  initializer's pointer, `x$initializer$`), exactly two (id, u8 how) with
  no count; else if the attributes have 0x80 (initialized), a c32 count
  and that many (id, u8 how); else none (certain).

### Functions (0x0E, 0x04, 0x10)

`id, u8 flags, sz name, MT [byte], TC, u8 S, u8 return size, common
fields, c32 type index (LF_PROCEDURE/LF_MFUNCTION), u32 id index
(LF_FUNC_ID/LF_MFUNC_ID)`, and for a body:

`c32 IL offset (.cil$ex), c32 locals offset (.cil$sy), c16 weight, u32 1,
u32 body flags, c16 parameter count (this counted), c32 reference count,
references (id, u8 how: 1 direct, 2 address taken, 0 virtual call),
[attributes]`.

- S: 3 static, 4 declared, 5 defined, 9 dllexport, 10 dllimport (certain).
- weight: grows with the body, by about one per operation (`a + 1` → `a +
  1 + a * 3` took it from 7 to 11): the inliner's measure, likely. This is
  the "byte that grows with a body"; it is a c16 (certain: 605 seen).
- body flags bit 0x20: the definition is emitted whatever refers to it (all
  711 non-inline public functions have it, inline ones don't, but for the
  three explicit instantiations, which do): certain as a predictor of
  lib.exe's index; its name is a guess. Other bits not known.

### The common fields

Every variable, function, named type and namespace has, after its type:

`u8 attributes, u8 F, [u16 extra if F & 0x80], [id section if F & 0x02],
u32 1, u8[4] kind, kind's fields`.

- attributes (certain where tested): for a function 0x10 callee pops,
  0x20 inline, 0x40 varargs, 0x80 noreturn; for a variable 0x80 has an
  initializer, 0x40 selectany, 0x20 COMDAT-like (string literals, RTTI,
  vftables, inline variables, local statics of inline functions), 0x10
  vftables (meaning not known).
- F: 0x80 and 0x02 as above (certain); 0x01 and 0x20 track a variable
  being referred to / having its address taken (likely); others not known.
- section: a section record's id (certain: `__declspec(allocate)`).
- kind[0] for a function: 0x01 virtual → `u8, u8 n, n ids` (the class and
  the bases it overrides from, CodeView type indexes it seems); 0x02 →
  two bytes (special member: 0x01 constructor, 0x81 copy constructor,
  0x02 assignment, 0x03 destructor, 0x08 comparison, 0x09/0x0A `operator
  new`/`delete`, likely); 0x08 pure; 0x80 nothing more. Framing certain.
- kind[0] for a variable: 0x02 (vftables, type descriptors) → two bytes,
  and if the first has 0x01, a leb CodeView type index: the type a
  `??_R0` describes (certain: 0x74 for `int`, the class's forward
  reference for classes).
- kind[1] 0x80 on compiler-made functions (likely); kind[2] 0x01: an
  attribute list ends the record: c16 count, ids of 0x1D records
  (certain); kind[3] 0x02 in objects built with `/std:c++17` or later, 0 with the
  default (likely: seen to follow the option, not isolated further).

## Public definitions: what lib.exe lists (certain over the corpus)

lib.exe's index of a `/GL` object lists the public definitions (and
commons) that a root reaches through references that are direct or take
an address (a virtual call needs the vftable, not the function), a vector
deleting destructor reaching its scalar deleting destructor. Roots:

- functions whose body flags have 0x20;
- `static` functions with a body that aren't inline;
- variables (public or static) not COMDAT-like, or selectany; commons.

`Globals::lib_listed` implements it; `hexcavator ltcg -publics` shows it.
It matched lib.exe exactly on 443 objects (30,494 definitions, three
architectures). Inline functions nothing emitted refers to are what it
leaves out.

## `.cil$ex` (framing likely, contents not decoded)

A 0x58-byte header (0x5B, 0x58, zeros in all 443 objects), then one unit
of IL per function, in source order. Every unit opens with `4F 1F 80
<u32> 4F 20 80 FE 00` (the u32 differs with options: `/Od`, x86); units
are found by that opening. Evidence that these are the units: their count
equals `.cil$sy`'s stream count in every object (45,025 units), every
function record's IL offset is a unit's start, and the k-th unit pairs
with the k-th `.cil$sy` stream exactly as the records say. So a function's
IL is its unit: start to the next unit (certain where the counts agree;
the finder is a pattern, so "likely" in general).

Functions nothing refers to, inline or `static` (and in-class member
functions), keep their IL unit but get no record (certain: sources with
one such function each; and 28,000 of the corpus's units have no record).

The bytecode itself: operations led by an opcode byte (`4F` takes a
sub-opcode; `54 00` seems to end a function), not decoded.

## `.cil$sy` (certain framing)

A stream per IL unit, back to back, each ending with 0x06. Records:

| Tag | Record | Fields |
|---|---|---|
| 0x03 | scope | u8 level, id, u8 kind (0x1F function, 0x1E block, 0 label, others for try/`__try`/coroutines), sz name (a label's), u8, u8 |
| 0x0D | scope start | u8 its number (1 parameters, 2 the body, ...) |
| 0x01, 0x02 | parameter / local | as a `.cil$gl` variable but with no references: class byte (W's place) 3 for parameters, 1 for locals |
| 0x1A | temporary | u8 scope, id, MT [byte], TC, c32 size, common fields, c32 type, [attributes] |
| 0x07 | local static | u8 scope, id, sz decorated name, MT [byte], TC, u8, c32 size, u8 u8, c32 type, id of the global, u8 |
| 0x0B, 0x08 | local type, constant | as in `.cil$gl` |

## `.cil$in` (certain framing)

Initializers back to back, then 0x0F at the section's end.

- `00 id c32` (always 0) starts one, `07` ends it.
- `01 kind c32 size value`: kind 1 signed, 2 unsigned, 3 pointer, 4 (not
  known), 5 floating point (then `size` raw bytes). Otherwise a size-1
  value is its one byte; else a byte other than 0x80 is the value
  (sign-extended for kind 1), and 0x80 is followed by `size` bytes.
- `02 id c32 addend c32 size`: an address; `12 ...`: the same, image-relative
  (RTTI's 32-bit offsets).
- `03 c16 n bytes`: bytes as they are; `08 c32 n`: zeros.

Ids are variables', constants', local statics', and some with no record
(variables nothing refers to). Items' sizes add up to the variable's IL
size in 98% of non-RTTI cases; RTTI counts its image-relative fields at 4
here and at pointer size in the record.

## Not known

- `.cil$gl`'s 30-byte header and the 0x18 records' values.
- The bytecode in `.cil$ex`.
- `.cil$db` past the id lists (`Types` stops there).
- Most flag bits named "not known" above.
