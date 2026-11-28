# .NET metadata fixtures

`tools/clr/build.cmd` builds these with the .NET SDK's C# compiler against
the .NET 8 reference assemblies (deterministically, from R:, so the paths
they record are `R:\...`):

- `Sample.dll` (`Sample.cs`): a library with a little of everything the
  metadata can hold: namespaces, nested and generic types and methods
  (with constraints), interfaces (with static abstract and default
  members), explicit overrides, properties, indexers, events, enums of
  several underlying types, explicit layouts and field offsets, fields
  with initial data (FieldRVA), constants and default parameter values,
  custom attributes with every kind of argument (enums, types, boxed
  values, arrays, named fields and properties), declarative security,
  P/Invoke (ImplMap, ModuleRef), marshalling descriptors, fixed buffers,
  function pointers, `in`/`out`/`ref` and `ref` returns, volatile fields
  (modreq), multi-dimensional and jagged arrays, `__arglist` (vararg
  call sites, a sentinel), `TypedReference`, pinned locals, iterators,
  async methods, lambdas, `stackalloc`, `switch`, try/catch/filter/
  finally, records, a type forwarder, user strings with escapes. It's
  public-signed with `tools/clr/sample.pub` (a public key only: the
  strong-name signature's space is there, zeroed), has
  `Sample.Extra.netmodule` added (File and ExportedType rows), two
  embedded resources (`Sample.Strings.resources`, which
  `tools/clr/ResGen.cs` writes with one value of each type
  ResourceWriter stores natively, and `notes.txt`), and a linked one
  (`linked.txt`).
- `Sample.pdb`: its portable PDB.
- `Sample.Extra.netmodule` (`Extra.cs`): a module without a manifest.
- `Hello.exe` (`Hello.cs`): a 32-bit program (PE32, `32BITREQUIRED`, an
  entry point).
- `oracle/ClrDump.dll` (`tools/clr/ClrDump.cs`): the oracle, which
  dumps a file's metadata as System.Reflection.Metadata reads it
  (method bodies decoded with System.Reflection.Emit's opcodes,
  `.resources` entries with ResourceReader). It targets .NET 8 and rolls
  forward to any newer runtime; the tests run it on the machine's
  framework assemblies when there's a `dotnet`.
- `*.dump.txt`: the oracle's dump of each fixture, which
  `crates/formats/clr/tests/oracle.rs` reproduces from turbonop-clr line
  for line; `opcodes.txt`, Reflection.Emit's CIL opcode table.
