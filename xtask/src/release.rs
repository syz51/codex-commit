//! Daily release planning: decide whether `main` ships anything new since the
//! latest stable tag, prepare the next patch version, and find the release
//! commit that still owes its tag.

use crate::{Result, cargo_package_version, git, tool, validate_version, write_atomic};
use flate2::read::GzDecoder;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Read;
use std::ops::Range;
use std::path::Path;

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
    /// The published crate would be identical to the latest tag's.
    Unchanged { version: Version },
    /// `main` already carries a version newer than the latest tag; its
    /// release commit still owes its tag.
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

/// Prepare the next patch release in the clean working tree at `root` when
/// the crate Cargo would publish differs from the one at the latest stable
/// tag reachable from `HEAD`. The release archives ship the same binary
/// sources plus README.md and LICENSE, which the crate also contains. The
/// release notes are written to `notes` only for a prepared release.
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
    if git(root, &["diff", "--quiet", &tag, "HEAD"]).is_ok() {
        return Ok(Plan::Unchanged { version: current });
    }
    let changed =
        changed_package_files(&package_contents(root)?, &package_contents_at(root, &tag)?);
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

/// A stable tag still owed by the version `HEAD` carries, and the release
/// commit that introduced that version.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OwedTag {
    pub(crate) tag: String,
    pub(crate) commit: String,
}

/// The tag owed by `HEAD`'s version, if it has not been released yet. The
/// tag belongs on the oldest commit of `HEAD`'s first-parent chain that
/// carries the version without interruption, so a later commit can recover a
/// release commit whose own tagging run was lost.
pub(crate) fn release_tag(root: &Path) -> Result<Option<OwedTag>> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    let current = checked_package_version(root, &manifest)?;
    let tag = format!("v{current}");
    if !git(root, &["tag", "--list", &tag])?.is_empty() {
        let tagged = format!("refs/tags/{tag}^{{commit}}");
        git(root, &["merge-base", "--is-ancestor", &tagged, "HEAD"])
            .map_err(|_| format!("{tag} exists but is not an ancestor of HEAD"))?;
        return Ok(None);
    }
    let latest = latest_tag(root)?;
    if current <= latest {
        return Err(format!("{tag} is not newer than the latest tag v{latest}").into());
    }
    let mut commit = None;
    for candidate in git(root, &["rev-list", "--first-parent", "HEAD"])?.lines() {
        let manifest = git(root, &["show", &format!("{candidate}:Cargo.toml")])?;
        if manifest_version(&manifest)?.1 != current {
            break;
        }
        commit = Some(candidate.to_owned());
    }
    let commit = commit.ok_or("HEAD does not carry its working tree version")?;
    if !has_heading(
        &git(root, &["show", &format!("{commit}:CHANGELOG.md")])?,
        current,
    ) {
        return Err(format!("CHANGELOG.md has no ## {current} section at {commit}").into());
    }
    Ok(Some(OwedTag { tag, commit }))
}

/// The literal `[package]` version, checked against Cargo, with
/// `Cargo.lock` checked to resolve unchanged.
fn checked_package_version(root: &Path, manifest: &str) -> Result<Version> {
    let version = manifest_version(manifest)?.1;
    if Version::parse(&cargo_package_version(root)?)? != version {
        return Err("Cargo.toml [package] version disagrees with cargo metadata".into());
    }
    let resolved = tool("cargo")
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .current_dir(root)
        .output()?;
    if !resolved.status.success() {
        return Err(format!(
            "Cargo.lock does not match the manifests: {}",
            String::from_utf8_lossy(&resolved.stderr).trim()
        )
        .into());
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

/// The files of the crate Cargo would publish from the package at
/// `directory`, by path inside the crate. The VCS stamp and the original
/// manifest are left out: Cargo builds from the normalized `Cargo.toml` and
/// the package's own pruned `Cargo.lock`, so workspace-only changes, such as
/// `xtask` dependencies, do not count.
fn package_contents(directory: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let target = tempfile::tempdir()?;
    let output = tool("cargo")
        .args(["package", "--no-verify", "--locked", "--manifest-path"])
        .arg(directory.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target.path())
        .current_dir(directory)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo package failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let crates: Vec<_> = fs::read_dir(target.path().join("package"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "crate")
        })
        .collect();
    let [package] = crates.as_slice() else {
        return Err("cargo package must produce exactly one crate".into());
    };
    let mut contents = BTreeMap::new();
    let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(package)?));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path: std::path::PathBuf = entry.path()?.components().skip(1).collect();
        let path = path.to_str().ok_or("crate path must be UTF-8")?.to_owned();
        if path == ".cargo_vcs_info.json" || path == "Cargo.toml.orig" {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        contents.insert(path, bytes);
    }
    Ok(contents)
}

/// The published crate at `tag`, from an exported copy of its tree.
fn package_contents_at(root: &Path, tag: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let archive = tool("git")
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
    package_contents(directory.path())
}

/// Crate paths added, removed, or changed; each is also its repository path.
fn changed_package_files(
    current: &BTreeMap<String, Vec<u8>>,
    tagged: &BTreeMap<String, Vec<u8>>,
) -> Vec<String> {
    current
        .keys()
        .chain(tagged.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| current.get(*path) != tagged.get(*path))
        .cloned()
        .collect()
}

fn commit_entries(root: &Path, tag: &str, paths: &[String]) -> Result<String> {
    let range = format!("{tag}..HEAD");
    let mut args = vec![
        "--literal-pathspecs",
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
        return Err("the crate changed but no commit since the tag touches its files".into());
    }
    Ok(entries.join("\n"))
}

fn has_heading(changelog: &str, version: Version) -> bool {
    let heading = format!("## {version}");
    changelog.lines().any(|line| line == heading)
}

/// Split CHANGELOG.md around the Unreleased section:
/// (through the Unreleased heading, its entries, from the next heading on).
fn changelog_parts(changelog: &str) -> Result<(&str, &str, &str)> {
    const HEADING: &str = "## Unreleased\n";
    let start = if changelog.starts_with(HEADING) {
        0
    } else {
        changelog
            .find(&format!("\n{HEADING}"))
            .ok_or("CHANGELOG.md has no ## Unreleased section")?
            + 1
    };
    // Search from the heading's own newline so an adjacent heading is found.
    let newline = start + HEADING.len() - 1;
    let rest = changelog[newline..]
        .find("\n## ")
        .map(|offset| newline + offset + 1)
        .ok_or("CHANGELOG.md has no released version after Unreleased")?;
    let body = newline + 1;
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
    if has_heading(changelog, next) {
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
        // An adjacent version heading, and Unreleased on the first line.
        let adjacent = "## Unreleased\n## 0.6.7\n\n- Old thing.\n";
        assert_eq!(unreleased_entries(adjacent).unwrap(), "");
        assert_eq!(
            release_changelog(adjacent, version("0.6.7"), version("0.6.8"), "- New.").unwrap(),
            "## Unreleased\n\n## 0.6.8\n\n- New.\n\n## 0.6.7\n\n- Old thing.\n"
        );
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
            let output = tool("git")
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
    fn workspace_only_changes_do_not_release() {
        let fixture = Fixture::new();
        // A workspace member changes the root manifest and Cargo.lock, but
        // not the normalized manifest or the pruned lock Cargo publishes.
        fixture.write(
            "Cargo.toml",
            &format!("{MANIFEST}\n[workspace]\nmembers = [\"helper\"]\n"),
        );
        fixture.write(
            "helper/Cargo.toml",
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        fixture.write("helper/src/lib.rs", "\n");
        fixture.write(
            "Cargo.lock",
            &format!("{LOCK}\n[[package]]\nname = \"helper\"\nversion = \"0.1.0\"\n"),
        );
        fixture.commit("Add a workspace helper");
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Unchanged {
                version: version("0.2.0")
            }
        );
        fixture.write(
            "helper/Cargo.toml",
            "[package]\nname = \"helper\"\nversion = \"0.1.1\"\nedition = \"2024\"\n",
        );
        fixture.write(
            "Cargo.lock",
            &format!("{LOCK}\n[[package]]\nname = \"helper\"\nversion = \"0.1.1\"\n"),
        );
        fixture.commit("Bump the workspace helper");
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Unchanged {
                version: version("0.2.0")
            }
        );
    }

    #[test]
    fn release_commit_owes_its_tag_until_tagged() {
        let fixture = Fixture::new();
        fixture.write("src/main.rs", "fn main() { println!(); }\n");
        fixture.commit("Print a blank line");
        assert_eq!(release_tag(fixture.root()).unwrap(), None);
        fixture.prepare().unwrap();
        fixture.commit("Release v0.2.1");
        let release = fixture.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let owed = Some(OwedTag {
            tag: "v0.2.1".to_owned(),
            commit: release.clone(),
        });
        assert_eq!(release_tag(fixture.root()).unwrap(), owed);
        assert_eq!(
            fixture.prepare().unwrap(),
            Plan::Pending {
                tagged: version("0.2.0"),
                version: version("0.2.1")
            }
        );
        // A later commit still points at the release commit, so a lost
        // tagging run is recovered rather than skipped.
        fixture.write("README.md", "Install v0.2.1 today.\n");
        fixture.commit("Reword install");
        assert_eq!(release_tag(fixture.root()).unwrap(), owed);
        fixture.git(&["tag", "-a", "v0.2.1", "-m", "v0.2.1", &release]);
        assert_eq!(release_tag(fixture.root()).unwrap(), None);
        assert!(matches!(fixture.prepare().unwrap(), Plan::Prepared { .. }));
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
