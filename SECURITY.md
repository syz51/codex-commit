# Security

Security fixes target the current version on `main`. Upgrade to the current version before checking whether a reported issue still applies.

Report suspected vulnerabilities through [GitHub private vulnerability reporting](https://github.com/syz51/codex-commit/security/advisories/new). Include a minimal reproduction, affected versions, and the impact. Do not post credentials, access tokens, private source, or unsanitized session logs in a public issue.

## Execution scope

Unattended mode gives Codex full local command access using your existing Codex and Git authentication. Run it only in repositories you trust. `--interactive` uses Codex's workspace-write sandbox and permission prompts instead.

Commit scope, `--push`, and the no-history-rewrite rules are instructions to the model, not OS-enforced Git or network restrictions. Repository content and model behavior can affect execution. Interrupting a run does not reverse completed commits or pushes; inspect repository and remote state before retrying.
