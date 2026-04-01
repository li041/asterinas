---
name: pr-const-conversion-review
description: Review a branch diff as a PR against another branch. Use when checking whether newly added constants, structs and enums are redundant and should reuse existing repository constants, and whether explicit type conversions or helper conversion functions should be replaced by implementing `From`/`TryFrom`.
---

# PR Constant and Conversion Redundancy Review

This skill reviews one branch against another branch in PR form and focuses on two classes of design debt:
- redundant constant definitions that should reuse existing constants;
- explicit conversion code that should be modeled with `From` or `TryFrom`.
- redundant data-type definitions (`struct`/`enum`) that should reuse existing repository types.

Use this skill when the task is to evaluate a diff, not to implement feature logic.

## Inputs

Required:
- base branch (for example: `main`)
- compare branch (for example: `feature-x`)

## Review Goal

Produce actionable findings that reduce duplication and improve type-conversion ergonomics without changing behavior.

## Workflow

1. Compute the PR-style diff from base to compare branch.
2. Enumerate changed Rust files and prioritize lines that add:
- `const` or `static` items;
- cast operators (`as`);
- conversion helpers (for example `to_*`, `from_*`, `into_*` wrappers).
3. Expand candidate extraction with syntax patterns in changed hunks:
- numeric constants, for example `const X: Ty = 4096;`, `const X: Ty = 0x1000;`, `const X: Ty = 0o644;`, `const X: Ty = 1 << 12;`;
- enum/int mapping by `match`, for example `fn foo(x: u32) -> Enum { match x { ... } }`;
- conversion functions that are not named `to_/from_/into_`, for example `inode_type_from_dirent_type`.
4. Extract newly added `struct` and `enum` definitions and check whether an existing repository type with equivalent fields/variants semantics already exists.
5. For each added constant, search the repository for equivalent meaning/value/name.
6. For each explicit conversion, determine whether `From`/`TryFrom` can represent the mapping safely.
7. For each added `struct`/`enum`, determine whether reuse, type aliasing, or extension of existing type is preferable.
8. Classify findings by confidence and risk.
9. Report precise, minimal fixes and any required validation tests.

## Decision Logic

### A. Constant redundancy checks

Mark as **redundant constant** when all are true:
- semantic meaning matches an existing constant;
- type and unit are compatible, or can be made compatible with small local changes;
- reusing existing constant does not weaken module boundaries.

Do **not** flag when either is true:
- duplication is intentional to preserve API/stability boundary;
- value is same but semantic domain differs (for example timeout vs retry limit).

Preferred fixes:
1. Replace new constant usage with existing constant path.
2. Re-export the canonical constant if call sites need local naming.
3. Remove dead duplicate definitions after replacement.

Constant candidate priority:
- `const NAME: TYPE = <number-literal>;`
- `const NAME: TYPE = <bit expression>;` (for example shifts or OR-ed flags)
- magic-number clusters in one module (for example type tags or mode bits)

### B. Conversion-style checks

Mark as **prefer `From`** when all are true:
- conversion is infallible and total;
- repeated explicit conversion appears in multiple call sites;
- implementing `From<Src> for Dst` improves clarity without violating orphan rules.

Mark as **prefer `TryFrom`** when either is true:
- conversion may fail due to range/format/domain checks;
- current code already branches on conversion failure.

Do **not** flag when either is true:
- conversion is intentionally local to avoid public API expansion;
- trait implementation would be misleading for domain semantics.

Preferred fixes:
1. Introduce `impl From<Src> for Dst` (or `TryFrom`) near the type definition.
2. Replace ad-hoc helper calls or `as` casts with `.into()` or `Dst::from(src)`.
3. Keep behavior identical; add checked conversion for narrowing casts.

Migration rule for accepted conversion findings:
1. Do not keep legacy wrapper conversion helpers once trait conversion is introduced.
2. Replace call sites to use trait-based conversion directly (`.into()`, `Dst::from(src)`, or `TryFrom`).
3. Remove the redundant helper function in the same change unless backward compatibility explicitly requires it.
4. If compatibility requires temporary coexistence, mark helper as transitional and add a clear removal TODO.

### C. Struct/Enum redundancy checks

Mark as **redundant type definition** when all are true:
- newly added `struct`/`enum` has equivalent domain meaning to an existing repository type;
- field/variant set is the same or trivially mappable without semantic loss;
- the new type does not provide substantial behavior (no complex methods or only simple accessors/conversions).

Do **not** flag when either is true:
- new type encodes a distinct domain boundary, lifecycle, ownership, or safety invariant;
- methods/trait impls on the new type provide non-trivial behavior not present in candidate existing type.

Complex-method exclusion (usually do not treat as redundancy):
- non-trivial state transitions;
- protocol-specific validation logic;
- synchronization/locking semantics;
- resource ownership/drop invariants.

Preferred fixes:
1. Reuse existing type directly.
2. Use `type` alias or newtype wrapper only if a boundary marker is needed.
3. If conversion is required, implement `From`/`TryFrom` instead of duplicating fields/variants.

Type candidate priority:
- added `struct` with only data fields and minimal/no methods;
- added `enum` with variant set mirroring existing enum;
- duplicate request/response payload structs with near-identical layout.

Additional conversion candidates that must be checked:
- `match`-based mapping functions, especially int-to-enum and enum-to-int conversion tables.
- small wrapper functions that hide casts without `to_/from_/into_` in their names.
- repeated map/closure conversions, for example `map(|x| ...)` plus casts.

When to prefer a trait over a mapping function:
- prefer `TryFrom<u32> for Enum` for int-to-enum mapping with unknown/default branch;
- prefer `From<Enum> for u32` for total enum-to-int mapping;
- keep standalone function when conversion is intentionally local and not a type-level relation.

## Quality Gates

A finding is valid only if it includes:
- location: file path and line reference in changed code;
- evidence: matching existing constant or conversion pattern location;
- safety note: why behavior is preserved (or how failure is handled for `TryFrom`);
- minimal patch direction: smallest safe change.

## Reporting Format

For each finding, use:
- severity: `high` | `medium` | `low`
- category: `const-redundancy` | `conversion-trait` | `type-redundancy`
- location: changed file and line
- finding: concise problem statement
- evidence: existing canonical constant or repeated conversion sites
- recommendation: exact refactor direction (`reuse`, `From`, or `TryFrom`)
- regression risk: what to retest

If no findings are confirmed, output:
- `No actionable redundancy found in constants or conversion style for this diff.`
- residual uncertainty areas (if any)

## Practical Command Pattern

Use non-destructive commands:

```bash
git diff --name-only <base>...<compare> -- '*.rs'
git diff <base>...<compare> -- '*.rs'
rg -n "\\b(const|static)\\b|\\bas\\b|\\b(to_|from_|into_)" kernel ostd osdk
rg -n "^\\+.*const\\s+[A-Z0-9_]+\\s*:\\s*[^=]+\\s*=\\s*(0x[0-9A-Fa-f_]+|0o[0-7_]+|0b[01_]+|[0-9][0-9_]*)" <diff-file-or-pipe>
rg -n "^\\+.*match\\s+" <diff-file-or-pipe>
rg -n "^\\+.*\\b(struct|enum)\\b\\s+" <diff-file-or-pipe>
```

Then validate each candidate with targeted repository search before reporting.

## Coverage Notes

Why misses can happen:
- Keyword-only scan can miss conversion functions not containing `to_`, `from_`, or `into_` in the name.
- Keyword-only scan can miss numeric constants if the diff filter is too conversion-oriented.
- `match` tables represent conversion semantics but are not cast syntax, so they are easy to skip.
- simple `struct`/`enum` duplication can be missed if scans only look at constants and casts.

Fallback rule:
- If initial keyword scan returns sparse candidates, always run a second pass on changed files for:
	- added `const` assignments with numeric or bit-expression RHS;
	- added `match` expressions that return another domain type;
	- newly added small conversion helpers.
	- added simple `struct`/`enum` definitions for possible reuse.

## Example Prompts

- Review branch `virtio-fs-bugs` against `main` and find redundant constants and `From`/`TryFrom` opportunities.
- In this PR diff, only check `kernel/src/fs/**` for duplicated constants and explicit casts that should become trait conversions.
- Perform conservative mode review: only report high-confidence redundancy/conversion findings.
- In this PR diff, also inspect `match`-based mapping functions (for example int-to-enum helpers) and decide whether `From`/`TryFrom` is a better fit.
- Detect newly added `const NAME = <number>` or bit-flag constants and verify whether existing repository constants can be reused.
- Detect newly added simple `struct`/`enum` definitions and determine whether an existing repository type should be reused.
