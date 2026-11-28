The `/GL` IL, more of it read: TODO.md's "LTCG objects" items 1 and 2. The format is written up in docs/ltcg-il.md, with the evidence for each field and how sure it is (certain / likely / not known).

**`.cil$gl`: every record is walked, one after another.** Source files, sections, namespaces, named types, constants, attributes, variables (scalars and aggregates), and functions (with a body, declared, and vector deleting destructors), up to the directives. The fields they share (attributes, flags, the section, the kind, and the attribute list) are read too. The byte that grows with a body is a c16 weight, about one per operation. If a record isn't understood, the old head search takes over for the rest (`Globals::walked` says when that happened).

**Public definitions as lib.exe lists them.** lib.exe lists the public definitions that a root reaches through direct or address-taken references; virtual calls don't count. The roots are non-inline or explicitly instantiated functions (a body flag), static non-inline functions, non-COMDAT or selectany variables, and commons. `Globals::lib_listed` implements this, and `hexcavator ltcg -publics` uses it.

**`.cil$sy`:** each function's scopes (labels included) and its parameters, locals, temporaries, and local statics, one stream per IL unit.

**`.cil$in`:** initializers: integers, floats, addresses, image-relative addresses, bytes, zeros.

**`.cil$ex`:** split into one unit of IL per function, found by the bytes every unit opens with. The unit count is checked against `.cil$sy`'s stream count. Functions nothing refers to (inline or static) keep their unit but get no record. The bytecode itself is not decoded.

**Evidence.** About 440 objects were built from the repository's tool sources for x64, x86, and ARM64, with several option sets. The corpus stayed on the build machine. Over all of them:
- every `.cil$gl`, `.cil$sy`, and `.cil$in` walks to exactly its end;
- the unit count equals the stream count, and each function record's IL offset and locals offset pair up;
- lib.exe's index is reproduced exactly: 30,494 public definitions, none different.

**`hexcavator ltcg`:**
- new options: `-records`, `-locals`, `-init`;
- `-refs` now also covers what variables' initializers refer to;
- the headers say whether every record was walked, and how many IL units there are;
- a function's size is now its IL unit.

**Tests:**
- unit tests for the readers;
- new fixtures in testdata/ltcg-il (built by tools/ltcg-il/build.cmd), each with lib.exe's index: x64 and ARM64 C++, and x64 and x86 C;
- tests that each object walks whole, that `-publics` equals lib.exe's index, and that each record kind, the locals, and the initializers come out right;
- damaged-input tests (truncation and byte changes), checking the readers never panic.

**Left (now in TODO.md):**
- `hexcavator size` weighing `/GL` objects and libraries;
- turbolib adopting `lib_listed`;
- decoding the bytecode;
- `.cil$gl`'s header;
- `.cil$db` past its id lists.
