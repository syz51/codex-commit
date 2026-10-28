//! Restricted, generation-only commit messages from caller-supplied public diffs.
//! No repository discovery or Git operations are performed. Unix only.
//!
//! Paths and model are explicit. Only the current access credentials are copied;
//! source OAuth refresh credentials, user configuration and instructions are never
//! passed to the generation process. Expired credentials require source CLI
//! reauthentication. Generation never writes back to the source authentication home.
//!
//! ```no_run
//! use codex_commit::{generate_message, GenerationOptions, GenerationRequest};
//! let options = GenerationOptions::new(
//!     "/absolute/path/to/codex".into(),
//!     "/absolute/path/to/auth-home".into(),
//!     "gpt-6.1-sol".into(),
//! );
//! let message = generate_message(GenerationRequest { diff: "public diff" }, &options)?;
//! let rendered = message.render()?;
//! # Ok::<(), codex_commit::Error>(())
//! ```
mod generation;
pub use generation::*;
