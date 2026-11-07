//! Daily release planning: decide whether `main` ships anything new since the
//! latest stable tag, prepare the next patch version, and recognize merged
//! release commits that still need their tag.

use crate::{Result, cargo_package_version, git, validate_version, write_atomic};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::ops::Range;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version([u64; 3]);

impl Version {
    fn parse(text: &str) -> Result<Self> {
        validate_version(text)?;
        let mut parts = [0; 3];
        for (part, text) in parts.iter_mut().zip(text.split('.')) {
            *part = text.parse()?;
        }
        Ok(Self(parts))
    }

    fn next_patch(self) -> Result<Self> {
        let [major, minor, patch] = self.0;
        let patch = patch.checked_add(1).ok_or("patch version overflows")?;
        Ok(Self([major, minor, patch]))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.0;
        write!(formatter, "{major}.{minor}.{patch}")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// No shipped file changed since the latest tag.
    Unchanged { version: Version },
    /// `main` already carries a version newer than the latest tag; its
    /// release commit is waiting for CI and tagging.
    Pending { tagged: Version, version: Version },
    /// The working tree now holds the next patch release.
    Prepared { previous: Version, version: Version },
}

impl Plan {
    /// `key=value` lines for `$GITHUB_OUTPUT`.
    pub(crate) fn outputs(&self) -> String {
        match self {
            Self::Unchanged { version } => format!("status=unchanged\nversion={version}\n"),
            Self::Pending { tagged, version } => {
                format!("status=pending\nprevious={tagged}\nversion={version}\n")
            }
            Self::Prepared { previous, version } => {
                format!("status=prepared\nprevious={previous}\nversion={version}\n")
            }
        }
    }
}

/// Prepare the next patch release in the clean working tree at `root` when a
/// shipped file changed since the latest stable tag reachable from `HEAD`.
/// The release notes are written to `notes` only for a prepared release.
pub(crate) fn prepare(root: &Path, notes: &Path) -> Result<Plan> {
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        return Err("prepare-release needs a clean working tree".into());
    }
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    let current = checked_package_version(root, &manifest)?;
    let tagged = latest_tag(root)?;
    if current < tagged {
        return Err(format!("Cargo.toml version {current} is older than tag v{tagged}").into());
    }
    if current > tagged {
        return Ok(Plan::Pending {
            tagged,
            version: current,
        });
    }
    let tag = format!("v{tagged}");
    let mut shipped = package_files(root)?;
    shipped.extend(package_files_at(root, &tag)?);
    let changed: Vec<String> = changed_paths(root, &tag)?
        .intersection(&shipped)
        .cloned()
        .collect();
    if changed.is_empty() {
        return Ok(Plan::Unchanged { version: current });
    }

    let next = current.next_patch()?;
    let changelog = fs::read_to_string(root.join("CHANGELOG.md"))?;
    let mut entries = unreleased_entries(&changelog)?.to_owned();
    if entries.is_empty() {
        entries = commit_entries(root, &tag, &changed)?;
    }
    let changelog = release_changelog(&changelog, current, next, &entries)?;
    let manifest = bump_manifest(&manifest, next)?;
    let lock = bump_lock(&fs::read_to_string(root.join("Cargo.lock"))?, current, next)?;
    let readme = replace_version(&fs::read_to_string(root.join("README.md"))?, current, next)?;
    write_atomic(&root.join("Cargo.toml"), manifest.as_bytes())?;
    write_atomic(&root.join("Cargo.lock"), lock.as_bytes())?;
    write_atomic(&root.join("README.md"), readme.as_bytes())?;
    write_atomic(&root.join("CHANGELOG.md"), changelog.as_bytes())?;
    if checked_package_version(root, &manifest)? != next {
        return Err("prepared manifest does not carry the next version".into());
    }
    write_atomic(notes, format!("{entries}\n").as_bytes())?;
    Ok(Plan::Prepared {
        previous: current,
        version: next,
    })
}

/// The tag for `HEAD` when it is a release commit: its first parent carries
/// an older version, and no tag for the new version exists yet.
pub(crate) fn release_tag(root: &Path) -> Result<Option<String>> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    let current = checked_package_version(root, &manifest)?;
    let previous = manifest_version(&git(root, &["show", "HEAD^1:Cargo.toml"])?)?.1;
    if current == previous {
        return Ok(None);
    }
    if current < previous {
        return Err(format!("version decreased from {previous} to {current}").into());
    }
    let tag = format!("v{current}");
    if !git(root, &["tag", "--list", &tag])?.is_empty() {
        let tagged = git(root, &["rev-parse", &format!("refs/tags/{tag}^{{commit}}")])?;
        if tagged == git(root, &["rev-parse", "HEAD"])? {
            return Ok(None);
        }
        return Err(format!("{tag} already exists at another commit").into());
    }
    let latest = latest_tag(root)?;
    if current <= latest {
        return Err(format!("{tag} is not newer than the latest tag v{latest}").into());
    }
    let changelog = fs::read_to_string(root.join("CHANGELOG.md"))?;
    if !changelog
        .lines()
        .any(|line| line == format!("## {current}"))
    {
        return Err(format!("CHANGELOG.md has no ## {current} section").into());
    }
    Ok(Some(tag))
}

fn checked_package_version(root: &Path, manifest: &str) -> Result<Version> {
    let version = manifest_version(manifest)?.1;
    if Version::parse(&cargo_package_version(root)?)? != version {
        return Err("Cargo.toml [package] version disagrees with cargo metadata".into());
    }
    Ok(version)
}

fn latest_tag(root: &Path) -> Result<Version> {
    git(root, &["tag", "--merged", "HEAD", "--list", "v*"])?
        .lines()
        .filter_map(|tag| Version::parse(tag.strip_prefix('v')?).ok())
        .max()
        .ok_or_else(|| "no stable vX.Y.Z tag is reachable from HEAD".into())
}

fn changed_paths(root: &Path, tag: &str) -> Result<BTreeSet<String>> {
    Ok(git(
        root,
        &["diff", "--name-only", "--no-renames", "-z", tag, "HEAD"],
    )?
    .split('\0')
    .filter(|path| !path.is_empty())
    .map(str::to_owned)
    .collect())
}

/// Files Cargo would publish from the package at `directory`; the release
/// archives ship a subset of these (README.md and LICENSE).
fn package_files(directory: &Path) -> Result<BTreeSet<String>> {
    let output = Command::new("cargo")
        .args(["package", "--list", "--locked", "--manifest-path"])
        .arg(directory.join("Cargo.toml"))
        .current_dir(directory)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo package --list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Package files at `tag`, so files deleted or excluded since then count too.
fn package_files_at(root: &Path, tag: &str) -> Result<BTreeSet<String>> {
    let archive = Command::new("git")
        .args(["archive", "--format=tar", tag])
        .current_dir(root)
        .output()?;
    if !archive.status.success() {
        return Err(format!(
            "git archive {tag} failed: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        )
        .into());
    }
    let directory = tempfile::tempdir()?;
    tar::Archive::new(archive.stdout.as_slice()).unpack(directory.path())?;
    package_files(directory.path())
}

fn commit_entries(root: &Path, tag: &str, paths: &[String]) -> Result<String> {
    let range = format!("{tag}..HEAD");
    let mut args = vec![
        "log",
        "--reverse",
        "--no-merges",
        "--format=%s",
        &range,
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let entries: Vec<String> = git(root, &args)?
        .lines()
        .map(|subject| format!("- {subject}"))
        .collect();
    if entries.is_empty() {
        return Err("shipped files changed but no commit since the tag touches them".into());
    }
    Ok(entries.join("\n"))
}

/// Split CHANGELOG.md around the Unreleased section:
/// (through the Unreleased heading, its entries, from the next heading on).
fn changelog_parts(changelog: &str) -> Result<(&str, &str, &str)> {
    const HEADING: &str = "\n## Unreleased\n";
    let start = changelog
        .find(HEADING)
        .ok_or("CHANGELOG.md has no ## Unreleased section")?;
    let body = start + HEADING.len();
    let rest = changelog[body..]
        .find("\n## ")
        .map(|offset| body + offset + 1)
        .ok_or("CHANGELOG.md has no released version after Unreleased")?;
    Ok((
        &changelog[..body],
        &changelog[body..rest],
        &changelog[rest..],
    ))
}

fn unreleased_entries(changelog: &str) -> Result<&str> {
    Ok(changelog_parts(changelog)?.1.trim())
}

fn release_changelog(
    changelog: &str,
    previous: Version,
    next: Version,
    entries: &str,
) -> Result<String> {
    let (head, _, rest) = changelog_parts(changelog)?;
    if !rest.starts_with(&format!("## {previous}\n")) {
        return Err(format!("CHANGELOG.md does not continue with ## {previous}").into());
    }
    if changelog.lines().any(|line| line == format!("## {next}")) {
        return Err(format!("CHANGELOG.md already has ## {next}").into());
    }
    Ok(format!("{head}\n## {next}\n\n{entries}\n\n{rest}"))
}

/// The byte range and value of the literal `version` in `[package]`.
fn manifest_version(manifest: &str) -> Result<(Range<usize>, Version)> {
    let mut in_package = false;
    let mut offset = 0;
    let mut found = None;
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
        } else if in_package
            && let Some(value) = trimmed
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        {
            if found.is_some() {
                return Err("Cargo.toml [package] has more than one version".into());
            }
            let start = offset + line.find('"').expect("matched a quoted value") + 1;
            found = Some((start..start + value.len(), Version::parse(value)?));
        }
        offset += line.len();
    }
    found.ok_or_else(|| "Cargo.toml [package] has no literal version = \"X.Y.Z\"".into())
}

fn bump_manifest(manifest: &str, next: Version) -> Result<String> {
    let (range, _) = manifest_version(manifest)?;
    let mut bumped = manifest.to_owned();
    bumped.replace_range(range, &next.to_string());
    Ok(bumped)
}

fn bump_lock(lock: &str, previous: Version, next: Version) -> Result<String> {
    let entry =
        |version| format!("\n[[package]]\nname = \"codex-commit\"\nversion = \"{version}\"\n");
    let old = entry(previous);
    if lock.matches(&old).count() != 1 {
        return Err(format!("Cargo.lock needs exactly one codex-commit {previous} entry").into());
    }
    Ok(lock.replacen(&old, &entry(next), 1))
}

/// Replace whole-version occurrences of `previous`, such as `v0.6.7`,
/// `=0.6.7`, and `codex-commit-v0.6.7-...`, but not `10.6.7` or `0.6.70`.
fn replace_version(text: &str, previous: Version, next: Version) -> Result<String> {
    let previous = previous.to_string();
    let next = next.to_string();
    let mut replaced = String::with_capacity(text.len());
    let mut last = 0;
    for (index, _) in text.match_indices(&previous) {
        let before = text[..index].chars().next_back();
        let after = &text[index + previous.len()..];
        let numeric = |text: &str| text.starts_with(|c: char| c.is_ascii_digit());
        if before.is_some_and(|c| c.is_ascii_digit() || c == '.')
            || numeric(after)
            || after.strip_prefix('.').is_some_and(numeric)
        {
            continue;
        }
        replaced.push_str(&text[last..index]);
        replaced.push_str(&next);
        last = index + previous.len();
    }
    if last == 0 {
        return Err(format!("README.md does not mention version {previous}").into());
    }
    replaced.push_str(&text[last..]);
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    const MANIFEST: &str = "[package]\nname = \"codex-commit\"\nversion = \"0.2.0\"\nedition = \"2024\"\ninclude = [\"Cargo.toml\", \"Cargo.lock\", \"src/**\", \"README.md\"]\n\n[dependencies]\n";
    const LOCK: &str = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"codex-commit\"\nversion = \"0.2.0\"\n";

    #[test]
    fn versions_order_numerically_and_bump_patch() {
        assert!(version("0.10.0") > version("0.9.9"));
        assert_eq!(version("0.6.7").next_patch().unwrap(), version("0.6.8"));
        assert!(Version::parse("0.6").is_err());
        assert!(Version::parse("0.06.1").is_err());
    }

    #[test]
    fn manifest_version_reads_only_the_literal_package_version() {
        let manifest = "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"x\"\nversion = \"0.6.7\"\n\n[dependencies]\ntoml = { version = \"1.1.8\" }\n";
        let (range, found) = manifest_version(manifest).unwrap();
        assert_eq!(found, version("0.6.7"));
        assert_eq!(&manifest[range], "0.6.7");
        assert_eq!(
            bump_manifest(manifest, version("0.6.8")).unwrap(),
            manifest.replace("version = \"0.6.7\"", "version = \"0.6.8\"")
        );
        assert!(manifest_version("[package]\nversion.workspace = true\n").is_err());
        assert!(manifest_version("[package]\nversion = \"1.0.0\"\nversion = \"1.0.1\"\n").is_err());
    }

    #[test]
    fn lock_bump_changes_only_the_root_package_entry() {
        let lock = format!("{LOCK}\n[[package]]\nname = \"other\"\nversion = \"0.2.0\"\n");
        let bumped = bump_lock(&lock, version("0.2.0"), version("0.2.1")).unwrap();
        assert_eq!(
            bumped,
            lock.replacen(
                "version = \"0.2.0\"\n\n[[package]]\nname = \"other\"",
                "version = \"0.2.1\"\n\n[[package]]\nname = \"other\"",
                1
            )
        );
        assert!(bumped.contains("name = \"other\"\nversion = \"0.2.0\""));
        assert!(bump_lock(LOCK, version("0.3.0"), version("0.3.1")).is_err());
    }

    #[test]
    fn readme_replaces_whole_versions_only() {
        let readme = "[v0.6.7](x/v0.6.7) codex-commit-v0.6.7-aarch64 \"=0.6.7\" 10.6.7 0.6.70 0.6.7.1 v0.6.7.\n";
        assert_eq!(
            replace_version(readme, version("0.6.7"), version("0.6.8")).unwrap(),
            "[v0.6.8](x/v0.6.8) codex-commit-v0.6.8-aarch64 \"=0.6.8\" 10.6.7 0.6.70 0.6.7.1 v0.6.8.\n"
        );
        assert!(replace_version("no version", version("0.6.7"), version("0.6.8")).is_err());
    }

    #[test]
    fn changelog_moves_unreleased_entries_under_the_new_version() {
        let changelog =
            "# Changelog\n\n## Unreleased\n\n- New thing.\n\n## 0.6.7\n\n- Old thing.\n";
        assert_eq!(unreleased_entries(changelog).unwrap(), "- New thing.");
        assert_eq!(
            release_changelog(
                changelog,
                version("0.6.7"),
                version("0.6.8"),
                "- New thing."
            )
            .unwrap(),
            "# Changelog\n\n## Unreleased\n\n## 0.6.8\n\n- New thing.\n\n## 0.6.7\n\n- Old thing.\n"
        );
        let empty = "# Changelog\n\n## Unreleased\n\n## 0.6.7\n\n- Old thing.\n";
        assert_eq!(unreleased_entries(empty).unwrap(), "");
        assert!(release_changelog(changelog, version("0.6.6"), version("0.6.8"), "-").is_err());
        assert!(release_changelog(changelog, version("0.6.6"), version("0.6.7"), "-").is_err());
        assert!(unreleased_entries("# Changelog\n\n## 0.6.7\n").is_err());
    }

    struct Fixture {
        directory: tempfile::TempDir,
        notes: tempfile::TempDir,
    }

    impl Fixture {
        /// A repository with release v0.2.0 tagged at its first commit.
        fn new() -> Self {
            let fixture = Self {
                directory: tempfile::tempdir().unwrap(),
                notes: tempfile::tempdir().unwrap(),
            };
            fixture.write("Cargo.toml", MANIFEST);
            fixture.write("Cargo.lock", LOCK);
            fixture.write(".gitignore", "/target\n");
            fixture.write("src/main.rs", "fn main() {}\n");
            fixture.write(
                "README.md",
                "Install v0.2.0 with codex-commit = \"=0.2.0\".\n",
            );
            fixture.write(
                "CHANGELOG.md",
                "# Changelog\n\n## Unreleased\n\n## 0.2.0\n\n- First release.\n",
            );
            fixture.git(&["init", "-b", "main"]);
            fixture.commit("Release v0.2.0");
            fixture.git(&["tag", "-a", "v0.2.0", "-m", "v0.2.0"]);
            fixture
        }

        fn root(&self) -> &Path {
            self.directory.path()
        }

        fn write(&self, path: &str, contents: &str) {
            let path = self.root().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.root().join(path)).unwrap()
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.root())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Release Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Release Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "--all"]);
            self.git(&["commit", "-m", message]);
        }

        fn prepare(&self) -> Result<Plan> {
            prepare(self.root(), &self.notes.path().join("notes.md"))
        }

        fn notes(&self) -> String {
            fs::read_to_string(self.notes.path().join("notes.md")).unwrap()
        }
    }

    #[test]
    fn unshipped_changes_do_not_release() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Unchanged {
                version: version("0.2.0")
            }
        );
        fixture.write("docs/notes.md", "Internal notes\n");
        fixture.write(".github/workflows/ci.yml", "name: CI\n");
        fixture.commit("Document internals");
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Unchanged {
                version: version("0.2.0")
            }
        );
        assert_eq!(fixture.git(&["status", "--porcelain"]), "");
    }

    #[test]
    fn shipped_change_prepares_patch_release_from_commit_subjects() {
        let fixture = Fixture::new();
        fixture.write("docs/notes.md", "Internal notes\n");
        fixture.commit("Document internals");
        fixture.write("src/main.rs", "fn main() { println!(); }\n");
        fixture.commit("Print a blank line (#7)");
        let plan = fixture.prepare().unwrap();
        assert_eq!(
            plan,
            Plan::Prepared {
                previous: version("0.2.0"),
                version: version("0.2.1")
            }
        );
        assert_eq!(
            plan.outputs(),
            "status=prepared\nprevious=0.2.0\nversion=0.2.1\n"
        );
        assert_eq!(fixture.notes(), "- Print a blank line (#7)\n");
        assert_eq!(
            fixture.read("Cargo.toml"),
            MANIFEST.replace("0.2.0", "0.2.1")
        );
        assert_eq!(fixture.read("Cargo.lock"), LOCK.replace("0.2.0", "0.2.1"));
        assert_eq!(
            fixture.read("README.md"),
            "Install v0.2.1 with codex-commit = \"=0.2.1\".\n"
        );
        assert_eq!(
            fixture.read("CHANGELOG.md"),
            "# Changelog\n\n## Unreleased\n\n## 0.2.1\n\n- Print a blank line (#7)\n\n## 0.2.0\n\n- First release.\n"
        );
    }

    #[test]
    fn written_unreleased_entries_take_precedence_and_deleted_files_count() {
        let fixture = Fixture::new();
        fixture.write("src/extra.rs", "\n");
        fixture.write("Cargo.toml", &MANIFEST.replace("0.2.0", "0.2.1"));
        fixture.write("Cargo.lock", &LOCK.replace("0.2.0", "0.2.1"));
        fixture.write("README.md", "Install v0.2.1.\n");
        fixture.write(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n## 0.2.1\n\n- Extra.\n\n## 0.2.0\n\n- First release.\n",
        );
        fixture.commit("Release v0.2.1");
        fixture.git(&["tag", "-a", "v0.2.1", "-m", "v0.2.1"]);
        // Only a deleted shipped file and an unshipped changelog change.
        fs::remove_file(fixture.root().join("src/extra.rs")).unwrap();
        fixture.write(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n- Remove the extra module.\n\n## 0.2.1\n\n- Extra.\n\n## 0.2.0\n\n- First release.\n",
        );
        fixture.commit("Remove extra module");
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Prepared {
                previous: version("0.2.1"),
                version: version("0.2.2")
            }
        );
        assert_eq!(fixture.notes(), "- Remove the extra module.\n");
        assert_eq!(fixture.read("README.md"), "Install v0.2.2.\n");
        assert!(fixture.read("CHANGELOG.md").starts_with(
            "# Changelog\n\n## Unreleased\n\n## 0.2.2\n\n- Remove the extra module.\n\n## 0.2.1\n"
        ));
    }

    #[test]
    fn merged_release_commit_is_pending_until_tagged() {
        let fixture = Fixture::new();
        fixture.write("src/main.rs", "fn main() { println!(); }\n");
        fixture.commit("Print a blank line");
        assert_eq!(release_tag(fixture.root()).unwrap(), None);
        fixture.prepare().unwrap();
        fixture.commit("Release v0.2.1");
        assert_eq!(
            release_tag(fixture.root()).unwrap(),
            Some("v0.2.1".to_owned())
        );
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Pending {
                tagged: version("0.2.0"),
                version: version("0.2.1")
            }
        );
        fixture.git(&["tag", "-a", "v0.2.1", "-m", "v0.2.1"]);
        assert_eq!(release_tag(fixture.root()).unwrap(), None);
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Unchanged {
                version: version("0.2.1")
            }
        );
        fixture.write("README.md", "Install v0.2.1 today.\n");
        fixture.commit("Reword install");
        assert_eq!(release_tag(fixture.root()).unwrap(), None);
    }

    #[test]
    fn release_tag_rejects_untracked_versions_and_dirty_preparation() {
        let fixture = Fixture::new();
        fixture.write("Cargo.toml", &MANIFEST.replace("0.2.0", "0.2.1"));
        fixture.write("Cargo.lock", &LOCK.replace("0.2.0", "0.2.1"));
        fixture.commit("Bump without changelog");
        assert!(
            release_tag(fixture.root())
                .unwrap_err()
                .to_string()
                .contains("no ## 0.2.1 section")
        );
        fixture.write("src/main.rs", "fn main() { println!(); }\n");
        assert!(
            fixture
                .prepare()
                .unwrap_err()
                .to_string()
                .contains("clean working tree")
        );
    }
}
