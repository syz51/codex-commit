# Security

Security fixes target the current version on `main`. Upgrade to the current version before checking whether a reported issue still applies.

Report suspected vulnerabilities through [GitHub private vulnerability reporting](https://github.com/syz51/codex-commit/security/advisories/new). Include a minimal reproduction, affected versions, and the impact. Do not post credentials, access tokens, private source, or unsanitized session logs in a public issue.

## Execution scope

Both modes use your existing Codex and Git authentication and inherit Codex's sandbox configuration by default. `--profile` selects a Codex profile; `--sandbox` overrides the sandbox for one run. The wrapper does not modify your configuration or automatically escalate blocked operations.

Unattended `codex exec` cannot prompt for human approval; configured automatic review may handle eligible requests. `--interactive` supports human approvals and inherits Codex's configured approval policy. `--ask-for-approval` overrides it with `on-request` (interactive only) or `never`. Full local access can be inherited from Codex, configured in the global `codex-commit` file, or requested with `--sandbox danger-full-access -a never`; use it only in repositories you trust. Configured and managed Codex policies still apply. The wrapper reads only its global configuration or an explicit `--config` path; `--no-config` bypasses it. It does not search for project configuration files of its own.

Commit scope, `--push`, and the no-history-rewrite rules are instructions to the model, not OS-enforced Git or network restrictions. Repository content and model behavior can affect execution. Interrupting a run does not reverse completed commits or pushes; inspect repository and remote state before retrying.
