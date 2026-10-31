//! Private per-invocation temporary files and tool caches.

use crate::cli::CliError;
use std::env;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::{Builder, TempDir};

pub(crate) struct TaskScratch {
    directory: TempDir,
    environment: [(&'static str, String); 3],
}

impl TaskScratch {
    pub(crate) fn new(repository: &Path) -> Result<Self, CliError> {
        let base = env::var_os("TMPDIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let repository = fs::canonicalize(repository).map_err(|error| {
            CliError::new(1, format!("could not resolve Git working tree: {error}"))
        })?;
        let scratch = Self::create_in(&base)?;
        if scratch.path().starts_with(&repository) {
            // Never let Git inspection or staging see the launcher's own cache
            // and test artifacts, even when TMPDIR points inside the repository.
            let message = "task temporary directory must be outside the Git working tree";
            return match scratch.close() {
                Ok(()) => Err(CliError::new(1, message)),
                Err(cleanup) => Err(CliError::new(1, format!("{message}; {}", cleanup.message))),
            };
        }
        Ok(scratch)
    }

    fn create_in(base: &Path) -> Result<Self, CliError> {
        // Resolve before Codex changes its working directory. In particular,
        // relative TMPDIR values belong to the invoking directory, not the repo.
        let base = fs::canonicalize(base).map_err(|error| {
            CliError::new(
                1,
                format!(
                    "could not resolve temporary directory {}: {error}",
                    base.display()
                ),
            )
        })?;
        let base_text = base.to_str().ok_or_else(|| {
            CliError::new(
                1,
                "temporary directory must be valid UTF-8 for Codex configuration",
            )
        })?;
        let directory = Builder::new()
            .prefix("codex-commit-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(&base)
            .map_err(|error| {
                CliError::new(
                    1,
                    format!("could not create task temporary directory in {base_text}: {error}"),
                )
            })?;
        // The validated base and tempfile's ASCII suffix guarantee UTF-8 here.
        let root = directory
            .path()
            .to_str()
            .expect("validated temporary path")
            .to_owned();
        let cache = directory.path().join("uv-cache");
        if let Err(error) = fs::create_dir(&cache) {
            let mut error = CliError::new(
                1,
                format!(
                    "could not create task uv cache {}: {error}",
                    cache.display()
                ),
            );
            if let Err(cleanup) = directory.close() {
                error.message.push_str(&format!(
                    "; could not remove task temporary directory {root}: {cleanup}"
                ));
            }
            return Err(error);
        }
        let environment = [
            ("TMPDIR", root.clone()),
            ("UV_CACHE_DIR", format!("{root}/uv-cache")),
            ("PYTHONDONTWRITEBYTECODE", "1".to_owned()),
        ];
        Ok(Self {
            directory,
            environment,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn configure(&self, command: &mut Command) -> Result<(), CliError> {
        command.arg("--add-dir").arg(self.path());
        for (key, value) in &self.environment {
            let encoded = toml_string(value).map_err(|error| {
                CliError::new(1, format!("could not encode task {key}: {error}"))
            })?;
            command
                .env(key, value)
                .arg("-c")
                .arg(format!("shell_environment_policy.set.{key}={encoded}"));
        }
        Ok(())
    }

    pub(crate) fn close(self) -> Result<(), CliError> {
        let path = self.directory.path().to_owned();
        self.directory.close().map_err(|error| {
            CliError::new(
                1,
                format!(
                    "could not remove task temporary directory {}: {error}",
                    path.display()
                ),
            )
        })
    }
}

fn toml_string(value: &str) -> Result<String, serde_json::Error> {
    // serde_json escapes quotes, backslashes, and U+0000..001F using escapes
    // shared by TOML basic strings, but TOML additionally forbids literal DEL.
    // Unicode is emitted directly, avoiding invalid TOML surrogate escapes.
    serde_json::to_string(value).map(|encoded| encoded.replace('\u{7f}', "\\u007f"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    fn fixture() -> TempDir {
        let base = env::var_os("CODEX_COMMIT_TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        Builder::new()
            .prefix("scratch-tests-")
            .tempdir_in(base)
            .unwrap()
    }

    fn create(base: &Path) -> TaskScratch {
        TaskScratch::create_in(base).unwrap_or_else(|error| panic!("{}", error.message))
    }

    #[test]
    fn private_absolute_root_contains_cache_and_preserves_neighbor_symlink_target() {
        let fixture = fixture();
        let neighbor = fixture.path().join("neighbor");
        fs::create_dir(&neighbor).unwrap();
        fs::write(neighbor.join("keep"), "unrelated").unwrap();
        let scratch = create(fixture.path());
        let path = scratch.path().to_owned();
        assert!(path.is_absolute());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(path.join("uv-cache").is_dir());
        fs::write(path.join("uv-cache/cache"), "cache data").unwrap();
        symlink(&neighbor, path.join("outside")).unwrap();
        scratch
            .close()
            .unwrap_or_else(|error| panic!("{}", error.message));
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(neighbor.join("keep")).unwrap(),
            "unrelated"
        );
    }

    #[test]
    fn paths_and_child_environment_round_trip_through_toml_without_shell_evaluation() {
        let fixture = fixture();
        let unusual = fixture
            .path()
            .join("'quotes' \"double\" $dollar `command` \\ newline\n\t\u{7f} 🦀");
        fs::create_dir(&unusual).unwrap();
        let scratch = create(&unusual);
        let mut command = Command::new("codex");
        command
            .env("KEEP", "unchanged")
            .args(["-c", "shell_environment_policy.inherit=\"core\""]);
        scratch
            .configure(&mut command)
            .unwrap_or_else(|error| panic!("{}", error.message));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], "shell_environment_policy.inherit=\"core\"");
        assert_eq!(args[2], "--add-dir");
        assert_eq!(args[3], scratch.path().as_os_str());
        let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment[std::ffi::OsStr::new("KEEP")].unwrap(),
            "unchanged"
        );
        for pair in args[4..].chunks_exact(2) {
            assert_eq!(pair[0], "-c");
            let setting = pair[1].to_str().unwrap();
            let parsed: toml::Value = toml::from_str(setting).unwrap();
            let (key, expected) = scratch
                .environment
                .iter()
                .find(|(key, _)| {
                    setting.starts_with(&format!("shell_environment_policy.set.{key}="))
                })
                .unwrap();
            assert_eq!(
                parsed["shell_environment_policy"]["set"][key].as_str(),
                Some(expected.as_str())
            );
            assert_eq!(
                environment[std::ffi::OsStr::new(key)].unwrap(),
                expected.as_str()
            );
        }
        scratch
            .close()
            .unwrap_or_else(|error| panic!("{}", error.message));
    }

    #[test]
    fn toml_basic_strings_round_trip_all_controls_and_non_bmp_unicode() {
        let value: String = (0..=0x1f)
            .chain(std::iter::once(0x7f))
            .map(|code| char::from_u32(code).unwrap())
            .chain("'quotes' \"double\" \\ $dollar `command` 🦀".chars())
            .collect();
        let encoded = toml_string(&value).unwrap();
        let parsed: toml::Value = toml::from_str(&format!("value={encoded}")).unwrap();
        assert_eq!(parsed["value"].as_str(), Some(value.as_str()));
    }

    #[test]
    fn invalid_base_reports_error_before_creating_any_scratch() {
        let fixture = fixture();
        let missing = fixture.path().join("missing");
        let error = TaskScratch::create_in(&missing)
            .err()
            .expect("invalid temporary base");
        assert!(
            error
                .message
                .contains("could not resolve temporary directory")
        );
        assert!(error.message.contains(missing.to_str().unwrap()));
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 0);
    }

    #[test]
    fn cleanup_error_reports_the_owned_path_instead_of_succeeding_silently() {
        let fixture = fixture();
        let scratch = create(fixture.path());
        let path = scratch.path().to_owned();
        // A regular file in place of the owned directory makes recursive
        // directory removal fail independently of the test user's privileges.
        fs::remove_dir_all(&path).unwrap();
        fs::write(&path, "replacement file").unwrap();
        let error = scratch.close().expect_err("cleanup must fail");
        assert!(
            error
                .message
                .contains("could not remove task temporary directory")
        );
        assert!(error.message.contains(path.to_str().unwrap()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement file");
    }

    #[test]
    fn non_utf8_temporary_base_is_rejected_without_lossy_configuration() {
        let fixture = fixture();
        let invalid = fixture
            .path()
            .join(std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec()));
        if let Err(error) = fs::create_dir(&invalid) {
            if [
                Some(nix::libc::EILSEQ),
                Some(nix::libc::EINVAL),
                Some(nix::libc::ENOTSUP),
            ]
            .contains(&error.raw_os_error())
            {
                eprintln!("filesystem rejects non-UTF-8 filenames; Linux CI exercises this case");
                return;
            }
            panic!("create raw-byte temporary base: {error}");
        }
        let error = TaskScratch::create_in(&invalid)
            .err()
            .expect("non-UTF-8 base");
        assert!(error.message.contains("must be valid UTF-8"));
        assert_eq!(fs::read_dir(&invalid).unwrap().count(), 0);
    }
}
