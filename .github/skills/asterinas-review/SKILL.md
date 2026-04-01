---
name: asterinas-review
description: Review Rust kernel code in the asterinas repository. Use when analyzing PRs, diffs, or Rust code for unnecessary Arc usage, redundant clone calls, lock correctness issues (deadlocks, TOCTOU, long critical sections), and readability issues such as deep nesting or missed early returns.
---

# Asterinas Code Review Skill

This skill performs focused, high-signal code review for the Asterinas kernel project.

Use it together with:
- [AGENTS.md](/root/asterinas/AGENTS.md) for repository layout and kernel constraints.
- [Rust coding guidelines](/root/asterinas/book/src/to-contribute/coding-guidelines/rust-guidelines/README.md) for style, concurrency, and performance expectations.
- The current diff or target files under review.

When you need Linux kernel evidence for comparison or behavioral confirmation, prefer the local `~/linux` checkout if it exists. Do not use online source searches for Linux code unless the local checkout is unavailable or the user explicitly asks for an external source. When citing evidence, prefer file paths from the local checkout so the user can jump directly to the relevant lines.

## Review Goal

Find defects and regressions first. Favor correctness, then clarity, then ownership efficiency. Report only issues that are concrete and actionable.

## Review Workflow

1. Identify the changed code paths and the surrounding invariants.
2. Check whether any `Arc` is truly needed for shared ownership or cross-thread access.
3. Check each `.clone()` for avoidable ownership churn or hot-path cost.
4. Review lock usage for deadlocks, TOCTOU, lock order violations, and overlong critical sections.
5. Review structure and readability against the repository guidelines, especially nesting depth and block expressions.
6. Summarize findings with severity, explanation, and the exact code location if available.

## Review Checklist

### 1. `Arc` usage

Check whether `Arc` is actually required.

Flag if any of the following apply:
- Used in a single-threaded context.
- No shared ownership is needed.
- Could be replaced with `&`, `Box`, or an owned value.
- Nested `Arc` appears, such as `Arc<Mutex<Arc<T>>>`.

Ask:
- Is there real cross-thread sharing?
- Is the lifetime already bounded?

Classify as:
- unnecessary Arc
- suspicious Arc (needs justification)
- justified Arc

### 2. `.clone()` usage

Detect semantic or performance issues with `.clone()`.

Flag:
- Clone immediately after creation.
- Clone just to satisfy the borrow checker when the code can be restructured.
- Clone of `Arc` in hot paths.
- Repeated clone inside loops.

Prefer:
- Borrowing with `&`.
- Moving ownership.
- Refactoring ownership boundaries.

If the clone is on an `Arc`, ask whether a borrowed reference or an owned move would preserve the same semantics.

### 3. Lock correctness

Check for deadlocks, TOCTOU, and lock scope issues.

Deadlocks:
- Multiple locks without a documented global order.
- Nested lock acquisition.
- Lock held across function calls.
- Lock held across I/O, waiting, or other blocking work.

TOCTOU:
- Check-then-use without keeping the state under the same lock.
- Validation done under one lock and action done under another.

Lock scope:
- Lock held too long.
- Work done inside the lock that can be moved out.
- Expensive cloning, allocation, or I/O inside the critical section.

Output must include:
- Problem description.
- Concrete interleaving scenario, if applicable.
- The specific lock order or invariant that is violated, if one exists.

### 4. Code style and readability

Use the repository's coding guidelines as the reference point, especially:
- block expressions to scope temporary state.
- minimize nesting depth to three levels or fewer.
- early returns and guard clauses.
- `let...else` for flattening control flow.
- `?` for error propagation.
- `continue` for loop filtering.

#### 4.1 Nesting depth

Flag if nesting is deeper than three levels.

Refactor using:
- early return.
- helper functions.
- `continue`.
- `let...else`.

The normal path should be the first visible path. Error and edge cases should be handled and dismissed early.

#### 4.2 Early return / guard clauses

Prefer:
```rust
if !cond {
    return Err(...);
}
```

over nested `if`/`else` paths for error handling.

#### 4.3 `let...else`

Prefer:
```rust
let Some(x) = option else {
    return Err(...);
};
```

when it flattens control flow.

#### 4.4 `?` operator

Replace manual error propagation with `?` when it reduces nesting and clarifies the main path.

#### 4.5 Block expressions

Use block expressions when temporary variables are only needed to produce one final value. This keeps temporary state local and avoids leaking one-off names into outer scope.

Flag patterns like:
- temporary bindings that exist only to compute a single final value.
- long-lived locals that can be reduced to a single scoped expression.

Prefer:
```rust
let socket_addr = {
    let bytes = read_bytes_from_user(addr, len as usize)?;
    parse_socket_addr(&bytes)?
};
connect(socket_addr)?;
```

over:
```rust
let bytes = read_bytes_from_user(addr, len as usize)?;
let socket_addr = parse_socket_addr(&bytes)?;
connect(socket_addr)?;
```

## Reporting Format

When you find an issue, report it with:
- severity: `critical`, `high`, `medium`, or `low`.
- location: file path and line if available.
- finding: what is wrong.
- impact: why it matters.
- fix: the smallest safe change.

If nothing actionable is found, say so explicitly and mention any residual risk areas you could not fully verify.

## Review Tone

Be precise, terse, and evidence-based. Do not speculate unless the code makes the risk plausible; if you do speculate, label it clearly as a concern that needs confirmation.