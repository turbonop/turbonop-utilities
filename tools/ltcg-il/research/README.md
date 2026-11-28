# LTCG IL research scripts

The experiments behind docs/ltcg-il.md (Python 3, Windows, MSVC). They
work in a scratch directory of their own (put them there): `msvc.py`
runs a tool with an environment captured from vcvarsall (`env64.txt`,
`env86.txt`, `enva64.txt`: the output of `set` after vcvarsall; vcvarsall
takes minutes on a busy machine, so it is captured once).

- `il.py`: an LTCG object's sections, and hex dumps.
- `exp.py`: compiles small sources given on the command line with `/GL`
  and dumps their records (varying one thing at a time).
- `walk.py`, `sy.py`, `ini.py`: strict walkers of `.cil$gl`, `.cil$sy`,
  `.cil$in` (what the Rust reader was ported from).
- `mkcorpus.py`: compiles tools/**/*.c* for x64, x64 `/Od`, x86, ARM64
  into `corpus/gen` (idle priority). The corpus is not committed.
- `libidx.py`: lib.exe's index of a library of each corpus object.
- `check.py`: the sections' cross-checks (units = streams, pairing,
  initializers' owners). `reach.py` (with `STATIC_ROOTS=1`),
  `pubrule.py`: the lib.exe index rule against the indexes.
- `rcheck.rs`: the same checks against the Rust reader (a scratch cargo
  project depending on turbonop-pe by path).
- `size-wip.patch`: a draft of `hexcavator size` for `/GL` objects
  (turbonop-size's objects.rs, hexcavator's is_object_input); compiled
  once, never tested. See TODO.md.
- `pr-draft.md`: a draft pull request description.
