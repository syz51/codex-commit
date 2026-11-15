# State machines and sequential work

This document lists the implicit state machines in this repository, as audited in November 2026.
Each machine gives its states, its transitions, the code that enforces them, and its gaps.
Each gap has a verdict from a probe or a test: **confirmed**, **by design**, or **refuted**.
Line references are correct for the commit that added this document.
All probes and measures used the mock Codex example or a shell stub. No model ran.

## Summary

| # | Machine | Code | Gaps |
| --- | --- | --- | --- |
| 1 | Invocation and configuration | `src/cli.rs` | none |
| 2 | Repository preflight | `src/cli.rs` | G1, G2 confirmed |
| 3 | Inspection status collapse | `src/inspection.rs` | G3 by design |
| 4 | Startup signal phase | `src/process.rs` | none |
| 5 | Supervisor run and cancellation | `src/process.rs` | G4 confirmed |
| 6 | Task scratch directory | `src/scratch.rs` | G5 by design |
| 7 | Compact renderer turn state | `src/output.rs` | G6 by design |
| 8 | `finalize` verify-then-commit | `src/finalize.rs` | G7 by design |
| 9 | Generation pipeline | `src/generation.rs`, `src/generate_cli.rs` | G8, G9 confirmed |
| 10 | Generation event parser | `src/generation.rs` | none |
| 11 | Release plan and owed tag | `xtask/src/release.rs` | G10 by design |
| 12 | Daily release workflow | `.github/workflows/daily-release.yml` | G11 by design |
| 13 | Tag release workflow | `.github/workflows/tag-release.yml` | G12 by design |
| 14 | Release publication workflow | `.github/workflows/release.yml` | G13 by design |
| 15 | Lint tool pin check | `.github/workflows/lint-tool-updates.yml` | G14 refuted |

## 1. Invocation and configuration

- States: `Display` (help, version), `Completions`, `Task`. For `Task`: configuration is `none`, `default file` or `explicit file`.
- Transitions: `parse` selects the invocation (`src/cli.rs:253`). `default_config_path` accepts only absolute bases (`src/cli.rs:109`). `load_config` ignores a missing default file and fails on a missing explicit file (`src/cli.rs:122`). CLI options override the file, field by field. `on-request` approval needs `--interactive` (`src/cli.rs:286`).
- Enforcement: all configuration errors return before any Git or Codex process starts. Test: `config_failures_are_reported_before_git_or_codex` in `tests/cli.rs`.
- Gaps: none found.

## 2. Repository preflight

- States: `unchecked` → `repository resolved` and `branch resolved` → `inspected` → `dry-run printed` or `identity checked` → `launch`.
- Transitions: one `git rev-parse --is-bare-repository --show-toplevel` call resolves the working tree (`src/cli.rs:369`). It runs at the same time as `git symbolic-ref` (`src/cli.rs:480`). Both children are joined before an error returns. A repository error has precedence over a detached HEAD. A dirty tree then starts two concurrent `git var` identity checks (`src/cli.rs:429`).
- Enforcement: tests `preflight_and_inspection_overlap_independent_git_queries`, `preflight_reports_each_unsuitable_repository_with_its_own_message`, `invalid_args_and_unsuitable_repositories_do_not_launch_codex`, `identity_preflight_overlaps_roles_and_joins_before_signal_error`.
- Gaps:
  - **G1, confirmed: a preflight failure loses Git's reason.** `git()` discards stderr and maps each failure to one fixed message (`src/cli.rs:351`). Probe: a repository with a broken `.git/config`. Git says `fatal: bad config line 11 in file .git/config`. The wrapper says `run inside a Git repository or use -C DIR`. The same cause also makes `symbolic-ref` report `detached HEAD`. Inspection keeps the Git exit status but also discards stderr (`src/inspection.rs:188`): an unreadable index gives `Git inspection failed: git status ... (exit status: 128)` without Git's `Permission denied` text.
  - **G2, confirmed: preflight Git calls have no time limit, and a signal sent only to the wrapper leaves the Git child running.** Signal handlers are installed after preflight (`src/cli.rs:586`). Probe: a Git stub that sleeps 6 s in `status`. The wrapper still waited after 3 s. After `SIGTERM` to the wrapper, the wrapper exited with 143 and the Git child stayed alive until its own end. A terminal Ctrl-C reaches the full foreground group, so that path has no orphan.

## 3. Inspection status collapse

- States: `full status` (`--untracked-files=all`) → `collapsed status` (`--untracked-files=normal`) → `error`.
- Transitions: a full status above 8 MiB starts one retry with untracked directories collapsed. A collapsed status above the limit is an error (`src/inspection.rs:163`). Status and both numstat calls run at the same time; all three are joined, and errors keep the order status, staged, unstaged (`src/inspection.rs:52`). Each Git child is drained after the retained limit, so a real Git failure stays a failure (`src/inspection.rs:188`).
- Enforcement: unit tests in `src/inspection.rs` (`isolated_git_cases`), and `collector_git_failure_stops_before_codex_or_scratch_creation`.
- Gaps:
  - **G3, by design: the overview can be stale when the agent uses it.** `README.md` says "the snapshot can become stale" and the prompt notice tells the agent to verify staged content. `finalize` checks the index again. Test: `scope_mismatches_preserve_index_and_head_without_running_hooks` in `tests/finalize.rs`.

## 4. Startup signal phase

- States: `default dispositions` → `startup` (handlers installed, first cancellation stored) → `supervising`.
- Transitions: `prepare_signals` installs the handlers and stores the first cancellation with `compare_exchange` (`src/process.rs:62`, `:103`). `run` closes the scratch and returns the signal status when a cancellation arrived before spawn (`src/process.rs:135`). `finish_startup` hands the stored signal to the supervisor (`src/process.rs:40`, `:166`).
- Enforcement: the unit test at `src/process.rs:677` and the cancellation tests of machine 5.
- Gaps: none found. The time before this phase is gap G2.

## 5. Supervisor run and cancellation

- States: `running` → `cancelling` (first signal forwarded, 250 ms grace) → `forced` (`SIGKILL`) → `reaped` → `cleaned` → `exit`. Direct modes also have `stopped` (after `SIGTSTP`) → `running` (after `SIGCONT`).
- Transitions: the first cancellation wins with `compare_exchange(0, signal)` (`src/process.rs:222`). The grace period is `CANCELLATION_GRACE` (`src/process.rs:21`). A second cancellation forces `SIGKILL` at once (`src/process.rs:228`). The reaper thread escalates after the deadline, reaps Codex and closes the scratch (`src/process.rs:309`-`387`). Compact mode owns the process group; direct modes target only the Codex PID. The exit status keeps the first signal.
- Enforcement: `compact_cancellation_bounds_unresponsive_codex_shutdown` (under 2 s), `compact_cancels_stopped_agent_and_preserves_first_signal_on_repeat`, `compact_exits_when_detached_descendant_retains_pipes`, `compact_blocked_progress_cancellation_reaps_owned_processes`, `direct_targeted_cancellation_is_bounded_without_rewriting_terminal_modes`, and the real-shell job-control tests in `tests/cli.rs`.
- Gaps:
  - **G4, confirmed: a commit-mode run has no time limit.** Probe: the mock in `wait` mode. The wrapper was still alive after 6 s with an idle agent and ended only on `SIGTERM` (exit 143, scratch removed). `generate` has a 120 s default limit; commit modes have none. An unattended caller, such as a script or a scheduled job, can wait without end. This is an owner decision on the interface.

## 6. Task scratch directory

- States: `absent` → `created` (mode 0700, with `uv-cache`) → `configured` (child environment set) → `removed`, or `left behind`.
- Transitions: `TaskScratch::new` rejects a base inside the working tree (`src/scratch.rs:17`). `create_in` makes the directory (`src/scratch.rs:38`). `configure` sets the child environment (`src/scratch.rs:114`). `close` removes it after Codex is reaped (`src/scratch.rs:128`). A removal failure is reported and fails an otherwise good run.
- Enforcement: `compact_scratch_is_removed_on_agent_and_protocol_failures`, `compact_cleanup_failure_is_visible_without_overwriting_an_agent_failure`, `compact_spawn_failure_cleans_scratch_and_invalid_base_does_not_launch_codex`, `compact_rejects_temporary_roots_inside_worktree_including_symlink_aliases`.
- Gaps:
  - **G5, by design: `SIGKILL` of the wrapper loses the state.** Probe: after `SIGKILL`, the scratch directory stayed, the mock Codex child stayed alive, and the next run did not remove the old directory. `SECURITY.md` and `README.md` say that killing or crashing the wrapper can leave temporary files behind. No process can handle `SIGKILL`.

## 7. Compact renderer turn state

- States: `Renderer { pending, completed, failed }` (`src/output.rs`). `pending` holds the last message until later activity shows it was progress.
- Transitions: `turn.started` sets `completed = false` (`src/output.rs:53`). `turn.completed` sets `completed = true`. `turn.failed` sets `failed = true` (`src/output.rs:58`); no event clears it. `finish` reports success only when the child succeeded, the turn completed and no turn failed (`src/output.rs:146`). The final message goes to stdout only on success.
- Enforcement: unit tests in `src/output.rs`; `compact_rejects_invalid_and_incomplete_events`.
- Gaps:
  - **G6, by design: `failed` has no exit.** A failed turn followed by a completed turn still fails the run. The code comment says "turn.failed remains decisive" (`src/output.rs:70`). The new test `failed_turn_stays_failed_when_a_later_turn_completes` pins this.

## 8. `finalize` verify-then-commit

- States: `arguments valid` → `repository resolved` → `index verified` → `committed` → `reported`.
- Transitions: one `rev-parse` call resolves the root and rejects a bare repository (`src/finalize.rs:115`). Then `symbolic-ref`, `ls-files --unmerged` and `diff --cached --name-only` run at the same time (`src/finalize.rs:132`). Their results are checked in that order, and only the first failure is reported. The staged path set must equal the selected set. `git commit` runs once with hooks enabled (`src/finalize.rs:177`). Then `rev-parse --verify HEAD` and `status` run at the same time (`src/finalize.rs:192`). A failure after the commit says "commit succeeded, but result verification failed".
- Enforcement: `tests/finalize.rs`, including `independent_git_queries_overlap_and_report_only_the_first_failure`, `scope_mismatches_preserve_index_and_head_without_running_hooks`, `hooks_and_commit_failures_are_not_bypassed_or_retried`, `post_commit_reporting_failure_reports_that_head_already_advanced`.
- Gaps:
  - **G7, by design: the index can change between the check and the commit.** Probe: a `pre-commit` hook that runs `git add other`. `finalize -- tracked` exited 0 and the commit held `other` and `tracked`. `README.md` says the helper "does not isolate the repository from concurrent writers or hooks that modify staging".

## 9. Generation pipeline

- States: `validated` → `credentials isolated` → `features discovered` → `isolation audited` → `inferred` → `parsed` → `grounded`. Each state can end in `Timeout`, `Cancelled` or a typed error.
- Transitions: `generate_message` validates input and sets one deadline for all stages (`src/generation.rs:562`). It copies the access token into a temporary home without refresh credentials (`src/generation.rs:488`). It runs three Codex children in sequence: `features list`, the loopback audit, and inference. Each child runs in its own process group, and `run` kills the group on every exit path (`src/generation.rs:270`, `:318`). The CLI maps `SIGINT` and `SIGTERM` to the cancellation flag (`src/generate_cli.rs:84`).
- Enforcement: unit tests in `src/generation.rs` (`supervisor_closes_stdin_and_cancels_hung_processes`, `loopback_audit_rejects_exposed_tools`, `isolated_credentials_remove_refresh_capability`); `tests/generate_cli.rs`.
- Gaps:
  - **G8, confirmed: `SIGHUP` and `SIGQUIT` are not handled in `generate`.** Probe: a Codex stub that sleeps. `SIGINT` gave exit 1 with `generation cancelled`, the child gone and no files left. `SIGHUP` gave exit 129; the child stayed alive and the temporary home with the copied `auth.json` stayed on disk. The child has its own process group, so a closed terminal does not reach it.
  - **G9, confirmed: no test runs the three stages together.** No test in the repository calls `generate_message` past the credential copy with a stub Codex. The order discovery → audit → inference and the success path have no test. For the same reason this audit has no time measure for this path.

## 10. Generation event parser

- States: the JSONL event order that `parse_events` accepts (`src/generation.rs:768`): thread start, turn start, exactly one agent message, turn completion. Tool events or a schema mismatch fail closed.
- Enforcement: `rejects_malformed_event_order`, `schema_and_tool_events_fail_closed`, `parses_structured_message`.
- Gaps: none found.

## 11. Release plan and owed tag

- States: `Plan::Unchanged`, `Plan::Pending` (version on `main` has no tag), `Plan::Prepared` (`xtask/src/release.rs:42`). Owed tag: `none` or `release commit found`.
- Transitions: `prepare` needs a clean tree, compares the crate contents with the latest tag, and writes four files plus the notes (`xtask/src/release.rs:72`, `:108`-`115`). `release_tag` finds the oldest first-parent commit that carries the version (`xtask/src/release.rs:134`).
- Enforcement: the tests in `xtask/src/release.rs`, including `release_tag_rejects_untracked_versions_and_dirty_preparation`.
- Gaps:
  - **G10, by design: the four file writes are not one atomic step.** A crash between them leaves a dirty tree. Probe: `prepare-release` on a dirty tree stops with `prepare-release needs a clean working tree`, so a partial state cannot be released. CI uses a fresh checkout for each run.

## 12. Daily release workflow

- States: plan `unchanged`, `pending`, `prepared`; release PR `none`, `open`, `auto-merge armed`, `merged`, `closed`.
- Transitions: `prepare` → `propose` (prepared), `close-stale` (unchanged or pending), `recover-tag` (pending). `propose` pushes with `--force-with-lease` and arms auto-merge with `--match-head-commit` (`.github/workflows/daily-release.yml:122`, `:142`). One concurrency group, `daily-release`, serializes runs.
- Gaps:
  - **G11, by design: `propose` fails when `main` moved after `prepare`.** The step prints "main moved after the release was prepared; run this workflow again" (`.github/workflows/daily-release.yml:110`). The next daily run prepares from current `main` (`docs/RELEASING.md`). Probe: the last six scheduled runs all succeeded.

## 13. Tag release workflow

- States: CI on the release commit is `success`, `waiting` or `failure`; tag `absent`, `pushed`, `already present at the same commit`.
- Transitions: `waiting` gives a notice and `ready=false` (`.github/workflows/tag-release.yml:84`-`89`). `failure` fails the run. `success` pushes the annotated tag. Runs are grouped by commit and are not cancelled (`.github/workflows/tag-release.yml:22`).
- Gaps:
  - **G12, by design: `waiting` has no exit inside the run.** `docs/RELEASING.md` says: "While its CI runs, nothing happens; its completion tags it" and "any later run recovers a lost one". The daily `recover-tag` job is the second route. Probe: the last five runs, all started by CI completion, succeeded.

## 14. Release publication workflow

- States: GitHub release `none`, `draft`, `published`; crate `absent`, `published`; `latest` flag set or not.
- Transitions: `validate` → `build` → `draft` → `cargo` → `publish` in `.github/workflows/release.yml`. Each job reads the current state first, so a rerun continues from that state. The `publish` job queues in one group (`.github/workflows/release.yml:319`). Verification retries 30 times with 5 s between tries.
- Gaps:
  - **G13, by design: a failed run leaves a partial release.** `docs/RELEASING.md` gives the recovery: rerun the failed jobs or dispatch the workflow against the same tag. The xtask tests cover `verify-release-tag`.

## 15. Lint tool pin check

- States: each of three pinned tools is `current` or `stale`.
- Transitions: the weekly `lint-tool-updates.yml` run compares the pins with the latest releases, with three tries per request.
- Gaps:
  - **G14, refuted: a workflow job can wait without a time limit.** Probe: a parse of all six workflow files. Each of the 15 jobs sets `timeout-minutes` (5 to 20).

## Sequential work on hot paths

Method: release build, macOS, Git 2.56. Two fixture repositories: `small` (60 files) and `large` (20,000 files), each with 20 unstaged, 20 staged and 20 untracked files. Numbers are medians in milliseconds. "Before" is `main` at `286e40e`.

### Changed

| Site | Before | After | Runs |
| --- | --- | --- | --- |
| `--dry-run`, small: preflight and inspection | 33.7 | 14.9 | 200 |
| `--dry-run`, large | 68.7 | 45.5 | 200 |
| Full compact run with the mock Codex, small | 51.4 | 34.9 | 100 |
| `finalize`, small | 44.3 | 30.3 | 60 |
| `finalize`, large | 86.1 | 70.9 | 60 |

- Inspection: status and the two numstat calls ran in sequence; they now overlap. This step alone moved the small dry-run from 33.7 to 21.8 and the large one from 68.7 to 52.6.
- Preflight: three Git calls in sequence became one `rev-parse` call that overlaps `symbolic-ref`. This moved the small dry-run from 21.8 to 14.9 and the large one from 52.6 to 45.5.
- `finalize`: seven Git calls in sequence around the commit became three rounds. A plain `git commit` takes 13.8 (small) and 30.4 (large) in the same fixtures.
- The overlap tests fail on the old binary and pass on the new one. The dry-run output is byte-identical before and after.

### Examined, no work

| Site | Measure | Reason |
| --- | --- | --- |
| Supervisor polling (5, 10 and 25 ms loops in `src/process.rs`) | About 12 ms of a 34.9 ms mock run | A real model run takes many seconds (`docs/PERFORMANCE.md`). |
| Identity checks | Already concurrent (`src/cli.rs:429`) | Pinned by test. |
| `make install`, warm build | 0.11 s total; each `--completions` call 2.3 ms | The build dominates. |
| `cargo xtask release-tag` | 75 ms; `cargo metadata` 22 ms | Runs once for each CI completion. |
| `cargo xtask prepare-release` | 0.31 s warm, 1.08 s cold | Runs once a day in CI. |
| Generation's three Codex runs | Not measured | The order is a fail-closed guard, and no stub exists (gap G9). |
