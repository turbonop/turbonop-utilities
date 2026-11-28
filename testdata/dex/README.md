# DEX fixtures

`tools/dex/build.cmd` builds the `Sample-*` files with javac (JDK 21,
`--release 11`, `-g -parameters`) and the Android SDK's d8 (build-tools
37), and dumps each with its dexdump (`dexdump -a -d -f -h`, run from
here, so that it names the file bare):

- `Sample-035.dex` (`tools/dex/Sample.java`, `--min-api 21`): classes,
  interfaces with default and static methods (desugared here), enums,
  annotations with every element kind and system annotations, static
  values of every type, every arithmetic and conversion, constants of
  every width, branches, packed, sparse, string and enum switches,
  arrays of every element type and their fill-array-data, try/catch/
  finally, monitors, lambdas, inner, local, anonymous classes, generic
  locals in scopes that end and restart, strings with escapes, control
  characters, NUL, surrogates.
- `Sample-037.dex` (`--min-api 24`), `Sample-038.dex` (`--min-api 26`:
  invoke-polymorphic), `Sample-039.dex` (`--min-api 28`, with
  `tools/dex/MakeIndy.java`'s hand-written class file and
  `Bootstraps.java`: call sites with every kind of static argument,
  method handles of every kind, const-method-handle, const-method-type),
  `Sample-041.dex` (the same in d8's container format, `--min-api 36`).
- `Handmade-039.dex`, `Handmade-040.dex`, `Handmade-container.dex`:
  written by `crates/formats/dex/tests/handmade` (every opcode and
  format, payload, debug opcode, value kind, hidden-API flag; a 041
  container of two files), dumped by dexdump;
  `TURBONOP_DEX_BLESS=1 cargo test -p turbonop-dex --test fixtures_handmade`
  writes them and their dumps again.
- `*.dex.txt`: dexdump's output (CRLF, as dexdump.exe writes it; strings
  as their raw MUTF-8 bytes), which `crates/formats/dex/tests` reproduce
  byte for byte.
