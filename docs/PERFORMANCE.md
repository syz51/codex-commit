# Routine commit benchmarks

Local macOS measurements on 2026-10-31. These are individual observations, not a statistically controlled speed guarantee.

Both workflows used GPT-6 Luna at low reasoning, normal service tier, `--no-config --sandbox danger-full-access -a never`, and push disabled. The baseline wrapper was built from `32b144ebc069d8be95886a5c10adc982334bf7c7`; the updated wrapper used the local efficiency changes. No installed executable or saved configuration was changed.

Each workload used disposable local Git fixtures with identical paired tracked and untracked file contents, configured test identities, and no remotes. Workloads were a small README addition with an intentional two-space Markdown break, a CLI help change with its assertion, and the exact 13-file change set from `81a22dba` to `32b144eb` (2,342 insertions, 208 deletions). The fixture code has 129 tests; the updated wrapper itself separately passed 134 workspace tests.

Each fixture's normal ignored target directory was prewarmed with `cargo build --workspace --all-targets --locked` outside the timer. Package verification was not explicitly prewarmed. Every valid run executed `make check`, `make test`, and `cargo publish -p codex-commit --dry-run --locked --allow-dirty`. Numeric exit statuses or complete successful tool diagnostics confirmed all three. Batched-phase and whitespace-follow-up runs retained numeric status 0 for every check.

Elapsed time includes inspection, checks, and committing. Model request counts and input usage came from each run's session events. Input counts are cumulative across requests; cached input is included in total input and excluded from uncached input. Uncached counts alone do not establish billing or equivalent context across runs.

| Workflow revision | Workload | Seconds | Model requests | Total input tokens | Uncached input tokens |
| --- | --- | ---: | ---: | ---: | ---: |
| Baseline (low) | README + Markdown hard break | 24.18 | 7 | 139,155 | 23,187 |
| Baseline (low) | CLI help + assertion | 26.59 | 11 | 222,270 | 47,166 |
| Baseline (low) | 13-file supervision/scratch update | 39.57 | 8 | 200,195 | 30,595 |
| Initial fixes | 13-file supervision/scratch update | 28.41 | 10 | 324,579 | 40,035 |
| Initial fixes | README + Markdown hard break | 25.19 | 7 | 173,463 | 20,631 |
| Initial fixes | CLI help + assertion | 25.41 | 9 | 190,735 | 17,935 |
| Batched phases | README + Markdown hard break | 26.29 | 5 | 124,241 | 42,065 |
| Batched phases | CLI help + assertion | 20.54 | 4 | 82,886 | 23,494 |
| Batched phases | 13-file supervision/scratch update | 25.43 | 6 | 192,619 | 29,035 |
| Whitespace follow-up | README + Markdown hard break | 22.32 | 4 | 83,770 | 11,066 |

Initial fixes exposed additional requests from separate checks, one-second polling, repeated patch inspection, and a normal no-match pattern scan. The next revision explicitly batched checks and commit verification, used zero-context diffs, waited up to 30 seconds for checks, and distinguished pattern no-match status from errors. A final wording-only change prohibited optional Git whitespace gates and used index conflict detection. Only the affected README workload was repeated after that final change; source and large workload measurements belong to the preceding batched-phase revision.

The final Markdown follow-up used four requests: combined guidance/patch inspection, one check batch, stage/commit/SHA/status verification, and the final response. No Git whitespace gate ran, and the intended hard break was preserved.

The skills catalog varied despite unchanged configuration: some README runs received 63 entries instead of 14, increasing initial context. Cache reuse, model choices, execution order, filesystem paths, and network timing also varied. The observations do not isolate the effect of any single setting, and no uniform reduction in uncached tokens is claimed.

The performance matrix used twelve paid fixture trials: ten valid observations above and two invalid trials excluded from the table. The invalid trials accidentally reused the baseline executable through a shared Cargo output directory. Subsequent builds used separate output directories and verified distinct executable hashes, help defaults, and the delivered prompt before inference. Valid performance runs preserved source contents, left clean fixture commits, changed no real repository HEAD or configuration, pushed nothing, and removed their private managed task directories.

Relevant executable SHA-256 values:

- Baseline: `ee5320b1a68f9f0ad733bb340f350aac4ebe1633d207294e1ce5138d91954122`.
- Initial fixes: `7c94e5c1c2b6eaccded1f97b41496c72fe01be11d78753a8d23fccc0bdd0010d`.
- Batched phases: `fa5cd16ce19b6ce8fe488fbb6b69e0684ec6a4e12de8231433ccf9830ffefe10`.
- Whitespace follow-up: `ed4a30f692e57e8307cbdafd4f2050f4e55ebc576a56f88314c5e00411bbf582`.

These results support batching routine commit work without removing required checks or hooks. Full patch preloading, automatic context-profile reductions, and post-run history enforcement remain separate design options; they are not needed to reproduce the four-request small-change flow and are not enabled by default.

## Post-review safety regressions

The implementation subsequently tightened the batched commit's staged-path/conflict gates, scanning of entire summarized untracked files, and inspection of conflict-marker matches. It also added untracked regular-file sizes for planning and a guard against an isolated test filter accidentally running zero tests. These changes postdate the performance table; the table is not a measurement of this later revision.

Four additional targeted live runs exercised two byte-identical fixtures across two prompt revisions:

| Case | First targeted run | Follow-up |
| --- | --- | --- |
| README change with an excluded file already staged | Complete staged-path mismatch stopped the commit; original HEAD and excluded staged blob were preserved. | Safely stopped again with the excluded staging preserved. Checks still ran first, so no early-scope-check efficiency improvement is claimed. |
| Large untracked document with intentional conflict-marker examples near EOF | The entire 860-line, 64,328-byte file was scanned, benign examples inspected, and exact contents committed. A mistaken cached-quiet comparison caused a recoverable false failure and two extra calls. | Whole-file scanning and all commit gates succeeded without that false failure or recovery. Exact contents were committed. |

All three repository checks returned numeric status 0 in all four targeted runs. The scope cases intentionally left the fixtures uncommitted; the positive cases left clean fixture commits. Every run preserved the real source repository, pushed nothing, and removed its private managed directory. The follow-up positive run used a NUL-aware parser for exact staged paths and checked conflicts and content before allowing the commit.

The follow-up prompt explicitly distinguishes a nonempty staged diff from a failed content comparison: `git diff --cached --quiet` normally returns 1 for changes awaiting commit, while `git diff --quiet -- <paths>` without `--cached` compares the worktree with the index for fully staged content. Intentionally partial staging requires a comparison that respects that scope. As the scope fixture shows, prompt-directed ordering remains model-dependent.

The final targeted-regression executable SHA-256 was `87a6fe2bdcc121f6b39cd692c9fcec4f9084a60c36cb312a231c675b31324c96`. These four runs are regression evidence, not additional paired performance samples or a guarantee of universal model compliance.

A subsequent closing review identified that rename detection hides a rename's source in the default staged path listing. The prompt now explicitly uses `git diff --cached --name-only -z --no-renames` for scope verification. Isolated real-Git tests cover plain filesystem moves and `git mv`, confirm both source and destination appear, and commit the move with contents preserved and a clean index. This wording fix required no additional paid model run; it postdates all live observations above.

A final pre-publication safeguard explicitly checks for unmerged index entries before any staging. A real-Git conflict fixture confirms that the overview reports `UU`, that the index exposes conflict stages before `git add`, and that staging alone clears those stages while leaving marker content untouched. Routine commits therefore stop on those entries before staging; this test is regression evidence, not another paid performance sample.

After v0.6.4, a real session exposed a failed shell scope check followed by a commit: NUL-delimited paths were passed through command substitution, and the assertion's exit 1 was misclassified as Git's expected comparison status. The native `finalize` helper now owns byte-preserving scope verification and the subsequent Git commit. The measurements above predate this change; no performance or model-compliance improvement is claimed from additional live trials.
