# Architecture review checklist

Questions to ask of a crate, or a family of crates that do the same job
for different inputs (instruction sets, file formats, debug formats).
Grown from the review of `crates/isa`; to become a skill.

## Uniformity

- Do the crates of a family have the same interface: the same entry
  points, the same argument order and types, the same error model?
- Can a general tool be written over the family's shared traits alone,
  with no per-member code? How much per-member code do the tools above
  have today (count the `match arch` and `match format` blocks)?
- Do the crates follow the same naming, file layout, test layout, and
  documentation structure?
- How many crates does adding one more member touch? It should be one.

## Names

First principles, not categorical imperatives: bend one where that makes
a better interface.

- Do the names follow Rust's conventions and best practices (the API
  guidelines: `SCREAMING_SNAKE_CASE` constants, `UpperCamelCase` types
  with acronyms as words, `snake_case` functions, no `get_`)?
- Are names full words (`Register`, not `Reg`), but for very common
  things and terms of art (ours included, where the full name is
  cumbersome)?
- Are there as few terms as we can get away with: the same operation the
  same verb everywhere (a `BitSet` and a `RegisterSet` both `contains`,
  `insert`, `remove`), the same concept the same word (a memory operand's
  `displacement` in every instruction set)?
- Does each term mean one thing (not "object" for both an object file
  and an object model; not "space" for both an index's kind and a
  memory)?
- Does the general name mean the general thing, and a more specific name
  the more specific thing (the traits' `operands`, the common form; an
  instruction set's richer form, `detailed_operands`)?
- While rearchitecting: is anything named less clearly than it could
  be? Rename it.
- Generated names may be ugly; is their noise trimmed only where nothing
  is lost (Arm's trailing `_`; `Opcode::name` keeps Arm's spelling)?

## Layering

- Which way do dependencies point? Does a lower layer depend on a higher
  one (a decoder on a container format)?
- Does knowledge live in the right layer? Facts about a member (which
  instructions are calls, which bits are a branch's offset) belong in the
  member's crate, not in a tool above it.
- Is configuration (modes, features, flags from a header) built in
  exactly one place?
- Is context the lower layer needs (names, tables) passed in through a
  trait the higher layer implements?

## Types

- Is the API as typed as it can be: registers, memory operands, accesses,
  addresses, indices as types, not as integers or strings?
- Are sets of one domain's values kept from mixing with another's?
- What is public and why? Public fields are a commitment. Are public
  enums `#[non_exhaustive]` where more variants will come, and exhaustive
  where callers must match all?
- Can every value the API returns also be built by hand and checked (for
  writers, encoders, assemblers)? Does it round-trip?

## Errors and robustness

- One error model across the family: `Option` or `Result`, chosen
  consistently, and do the errors say why (truncated, undefined,
  unsupported)?
- No panics on any input, and fuzzed.
- Are certain and inferred results kept apart, and does the API say
  which is which?

## Performance

- Any string comparison, allocation, or dynamic dispatch per element in a
  hot loop? Dispatch should be hoisted to once per buffer.
- Is static knowledge precomputed (in tables) rather than recomputed at
  run time?
- Are sizes of per-element values asserted and measured?
- Does the work use every core (or saturate I/O)?
- Did the change make the build slower? Generated code is checked in.

## Correctness evidence

- Does each member have an oracle (LLVM, objdump, the vendor's tools), a
  golden test, and exhaustive coverage where the space allows?
- Is completeness (of effects, of fields) tested, not just agreement on
  what's present?

## Provenance

- Can generated code be regenerated, and does it say from what and which
  version?
- Are licenses and notices of sources present and consistent?

## Documentation

- Does each crate say what it covers, what it deliberately doesn't, and
  which oracle it matches?
