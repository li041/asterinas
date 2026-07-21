# fio benchmark results for PR #3419

The baseline was measured from PR #3419 at commit
`61b6e07e4664383ae27463738f979dcc54936c58`. Each workload was run once on
2026-07-21. The JSON files under `raw/baseline/run-1` are the unmodified output
produced by the benchmark harness.

| Workload | Mode | Linux (MB/s) | Asterinas (MB/s) | Asterinas / Linux |
|---|---|---:|---:|---:|
| read | cached | 32534.4 | 13529.1 | 41.58% |
| read | direct | 10449.1 | 5081.4 | 48.63% |
| write | cached | 3901.75 | 594.543 | 15.24% |
| write | direct | 2156.92 | 3378.51 | 156.64% |

## PR #3605 direct-write comparison

PR #3605 commit `6e2022cf6a5b5cf4874ee0dae0b02a8c1f2930c9` was applied on top of the
baseline commit. The Asterinas half of the write workload was then rerun with
the same fio, QEMU, and virtiofsd configuration.

| Variant | Asterinas direct write (MB/s) | Change |
|---|---:|---:|
| PR #3419 baseline | 3378.51 | - |
| PR #3419 + PR #3605 | 6018 | +78.13% |

The optimized run was stopped after the Asterinas half because PR #3605 does
not modify the Linux guest. `comparison.json` records the measured result and
the exact source commits used to construct the test tree.

These are single-run local measurements, not a three-run average. They are
intended to keep the tracked baseline current and to quantify the effect of PR
#3605. Repeated CI measurements are still needed before setting regression
thresholds.
