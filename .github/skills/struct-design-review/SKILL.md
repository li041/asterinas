---
name: struct-design-review
description: Review Rust kernel structs and their methods for redundancy, abstraction correctness, ownership/lifecycle semantics, concurrency risks, and Linux semantic alignment. Use when evaluating data-structure design quality, minimality, and practical simplification opportunities.
---

# Struct Design Review Skill

This skill performs strict, kernel-oriented design review for a target struct and its associated methods.

Scope: Asterinas repository only. Do not generalize recommendations for non-Asterinas codebases.

Default mode: strong critical review. Prefer surfacing plausible design risks unless disproven by code evidence.

When you need Linux kernel evidence for comparison or behavioral confirmation, prefer the local `~/linux` checkout if it exists. Do not use online source searches for Linux code unless the local checkout is unavailable or the user explicitly asks for an external source. When citing evidence, prefer file paths from the local checkout so the user can jump directly to the relevant lines.

Use it when you need evidence-based answers to:
- Is this struct minimal, or does it carry redundant/shadow state?
- Does the abstraction match one clear responsibility?
- Do behavior and stored state stay consistent over lifecycle and concurrency?
- Is the design aligned with Linux kernel semantics?

## Inputs Required

Provide at least:
- Target struct name and file path.
- Method set to review (`impl` blocks, trait impls, helper functions that mutate/read fields).
- Linux counterpart candidates are required for finalization (for example `struct file`, `inode`, `dentry`, `vm_area_struct`).

When possible, include Linux implementation references:
- file path under Linux source tree
- concrete field names
- relevant function names that mutate/check those fields

If methods are spread across files, gather all relevant call paths before concluding.

## Review Goal

Prioritize correctness and minimality under kernel constraints.
Default stance: if a field cannot justify existence with semantics or measured performance need, recommend removal.
Critical stance: if justification is missing or weak, mark as issue first and downgrade only when strong evidence exists.

## Workflow

1. Build a field-method matrix.
2. Identify redundancy and shadow state.
3. Validate abstraction boundaries.
4. Check behavior-state consistency and invariants.
5. Verify lifecycle and ownership semantics.
6. Audit concurrency and lock design.
7. Compare semantics with Linux counterpart structures.
8. Propose minimal, practical refactoring.

## Detailed Checklist

### 1. Field-Method Matrix (must do first)

For every field, classify usage:
- read in methods
- written in methods
- write-only
- read-only
- never touched

Then classify each method:
- state-constructor (establishes invariant)
- state-transition (mutates invariant)
- pure query (read-only)
- side-effect bridge (I/O or external dependency)

Do not continue to high-level conclusions before this matrix is complete.

### 2. Redundancy and Minimality

Flag fields when:
- value is derivable from other fields.
- cached state has no clear hot-path/perf justification.
- same logical state exists in multiple layers (for example VFS and FS-private object both store it).
- multiple fields encode one concept (shadow state).

Ownership inefficiency checks:
- unnecessary `Arc`/`Rc`.
- avoidable `.clone()` especially in hot path or loops.
- owned field where borrowed relation is sufficient.

Decision rule:
- Keep cached/duplicated field only when consistency strategy is explicit and cheaper than recomputation.

### 3. Abstraction Correctness

Ask:
- Does the struct represent one stable concept?
- Are policy and mechanism mixed?
- Is it becoming a god struct (too many unrelated reasons to change)?

Split suggestions when you see:
- lifecycle state + operational config + transient cache all in one struct.
- fields updated by disjoint method groups with weak coupling.

### 4. Behavior-State Consistency

Check:
- methods that never read key fields they should depend on.
- fields never participating in any invariant.
- write-only/read-only anomalies that suggest dead state.
- transitions that can violate invariants under error paths.

Require explicit invariant statements, for example:
- if `opened == true`, backing object must be valid.
- refcount/map/list membership must stay synchronized.

If invariant is implicit, treat as risk.

### 5. Lifecycle and Ownership Semantics

Validate:
- Who creates, owns, mutates, and destroys the object.
- Whether ownership model matches call graph reality.
- Whether interior mutability is necessary and scoped.

Flag:
- nested `Arc` patterns without strong reason.
- lock/mutex wrapped around mostly immutable data.
- unclear single-writer vs multi-writer model.

Prefer simpler ownership first:
- `&T` / `&mut T` / plain ownership over shared ownership where possible.

### 6. Concurrency and Locking Semantics

Check lock granularity and lock scope:
- one lock protecting unrelated fields (false contention).
- lock held across blocking/I/O/wait operations.
- potential deadlock via lock order inversion.
- TOCTOU from check-then-use across unlock/relock windows.

For each risk, provide at least one concrete interleaving scenario.

### 7. Linux Semantic Comparison (Implementation-Level)

Note: This section is mandatory and is the primary external correctness anchor for this skill.

Pick the closest Linux equivalent first, then compare:
- field composition
- responsibility boundary
- ownership/lifetime model
- synchronization strategy

Implementation-level requirement (mandatory):
- Name Linux source files/functions/fields used as comparison evidence.
- Do not stop at concept-level analogy.
- For each major Asterinas field group, state Linux implementation mapping or explicit absence.
- If Linux behavior differs, explain whether Asterinas divergence is intentional, architecture-required, or likely accidental complexity.

Execution notes:
- If local Linux checkout exists at `~/linux`, use it first for evidence collection.
- Treat "no concrete Linux file/function/field evidence" as incomplete review.
- For each major finding, include at least one Asterinas-side location and one Linux-side location.
- If no Linux equivalent can be found, explicitly state search scope and why the mapping is unresolved.

Classify findings:
- present here but absent in Linux: potential over-design or layer violation.
- present in Linux but missing here: potential semantic gap.

Do not copy Linux mechanically.
Explain whether differences are intentional, required by architecture, or accidental complexity.

Annotation for divergence judgment:
- intentional: design doc, comments, or call-path behavior clearly supports divergence.
- required: Asterinas architecture constraints (for example framekernel boundary) force divergence.
- accidental complexity: no strong constraint or measurable benefit justifies extra state/abstraction.

### 8. Over-Engineering and Simplification

Flag patterns:
- abstraction layers without measurable benefit.
- enum/trait indirection where closed/simple struct suffices.
- premature generalization for hypothetical future features.

Prefer refactors that reduce state surface area:
- remove field
- merge duplicate state
- split struct by responsibility
- move field to the correct layer
- simplify ownership and locking

## Severity Model

Use:
- `critical`: can cause correctness/security/concurrency failure.
- `high`: likely semantic bugs or persistent inconsistency.
- `medium`: design debt with realistic maintenance/perf cost.
- `low`: readability/maintainability improvements.

Strong critical review defaults:
- Prefer `high` over `medium` when invariant ownership or lock semantics are unclear.
- Treat undocumented shadow state as at least `high` unless proven harmless.
- Treat redundant ownership layers (`Arc` + interior mutability + duplicate cache) as at least `medium`, escalate when on hot path.

## Output Format (mandatory)

### Summary
- Overall assessment: `good` / `over-designed` / `under-designed` / `mixed`.

### Issues Found
- For each issue provide:
  - field/method
  - problem type
  - evidence (where and how observed)
  - impact
  - minimal fix
  - severity

### Linux Comparison
- Closest equivalent structure.
- Linux implementation evidence:
  - source file(s)
  - relevant field(s)
  - relevant method/path(s)
- Key differences and implications.
- Divergence judgment: intentional / required / accidental complexity.

### Suggested Refactoring
- Concrete changes only, for example:
  - remove field `x`
  - derive `y` from `z`
  - replace `Arc<T>` with `&T` or ownership move
  - split into `A` + `B`
  - move field to VFS/FS-private layer

## Quality Gates Before Finalizing

A review is incomplete unless all are true:
- Every field is accounted for in the field-method matrix.
- At least one explicit invariant (or missing invariant risk) is documented.
- Linux counterpart is identified and compared at implementation level with concrete evidence.
- Refactoring suggestions are concrete and minimal.
- No generic advice without field/method-level evidence.

## Mode Banner (put at top of review output)

Start each review with:
- Mode: `Asterinas-only | Linux implementation-level comparison | Strong critical`

## Example Prompts

- "Review struct `FooInodeState` in `kernel/.../foo.rs` with this skill and compare to Linux `inode` semantics."
- "Analyze whether fields in `BarFileHandle` are redundant; include Arc/clone ownership critique and lock risks."
- "Perform strict behavior-state consistency review for `BazDentry` and suggest minimal refactor plan."
