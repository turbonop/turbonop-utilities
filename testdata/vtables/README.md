# vtables test program

`tools/vtables/build.cmd` builds `tools/vtables/main.cpp` with MSVC for
x64 and ARM64, with RTTI (`/GR`): `vtables-<arch>.exe`, its full PDB
(`vtables-<arch>.pdb`), and its stripped PDB (`vtables-<arch>-stripped.pdb`:
public symbols only, as Microsoft's symbol server serves its own).

What it holds, for `hexcavator vtables` (`apps/hexcavator/tests/it/vtables.rs`):

- `Square` (one base) and `Badge` (two: a second table, `{for `Named'}`).
- Three lambdas held by `std::function`, each in a
  `std::_Func_impl_no_alloc` of its own, with its own table. Their tables
  and RTTI are internal: the stripped PDB has no symbol for them, so only
  their Complete Object Locators find and name them.
