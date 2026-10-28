use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
const USAGE: &str = "Usage:
  cargo xtask package --target TRIPLE --version X.Y.Z --output DIR
  cargo xtask checksums --version X.Y.Z --dir DIST
  cargo xtask homebrew --version X.Y.Z --checksums PATH --output PATH
  cargo xtask verify-release-tag --tag vX.Y.Z";

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is a workspace subdirectory")
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(USAGE.into());
    };
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    let command = command.to_str().ok_or("command must be UTF-8")?;
    let allowed = match command {
        "package" => &["--target", "--version", "--output"][..],
        "checksums" => &["--version", "--dir"][..],
        "homebrew" => &["--version", "--checksums", "--output"][..],
        "verify-release-tag" => &["--tag"][..],
        _ => return Err(format!("unknown command: {command}\n{USAGE}").into()),
    };
    let options = options(args, allowed)?;
    match command {
        "package" => {
            let target = text_option(&options, "--target")?;
            let version = text_option(&options, "--version")?;
            let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
                Some(path) => std::env::current_dir()?.join(path),
                None => workspace().join("target"),
            };
            let path = package(
                workspace(),
                &target_dir,
                target,
                version,
                Path::new(&options["--output"]),
            )?;
            println!("{}", path.display());
        }
        "checksums" => {
            let path = checksums(
                text_option(&options, "--version")?,
                Path::new(&options["--dir"]),
            )?;
            println!("{}", path.display());
        }
        "homebrew" => {
            let formula = render_formula(
                text_option(&options, "--version")?,
                &fs::read_to_string(&options["--checksums"])?,
            )?;
            let output = Path::new(&options["--output"]);
            write_atomic(output, formula.as_bytes())?;
            println!("{}", output.display());
        }
        "verify-release-tag" => {
            verify_release_tag(workspace(), text_option(&options, "--tag")?)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn options(
    args: impl Iterator<Item = OsString>,
    allowed: &[&str],
) -> Result<BTreeMap<String, OsString>> {
    let mut args = args;
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        let key = key.to_str().ok_or("option name must be UTF-8")?;
        if !allowed.contains(&key) {
            return Err(format!("unexpected option: {key}").into());
        }
        let value = args.next().ok_or_else(|| format!("{key} needs a value"))?;
        if options.insert(key.to_owned(), value).is_some() {
            return Err(format!("duplicate option: {key}").into());
        }
    }
    for key in allowed {
        if !options.contains_key(*key) {
            return Err(format!("missing required option: {key}").into());
        }
    }
    Ok(options)
}

fn text_option<'a>(options: &'a BTreeMap<String, OsString>, key: &str) -> Result<&'a str> {
    options[key]
        .to_str()
        .ok_or_else(|| format!("{key} must be UTF-8").into())
}

fn validate_version(version: &str) -> Result<()> {
    let components: Vec<_> = version.split('.').collect();
    if components.len() != 3
        || components.iter().any(|component| {
            component.is_empty()
                || !component.bytes().all(|byte| byte.is_ascii_digit())
                || (component.len() > 1 && component.starts_with('0'))
        })
    {
        return Err(
            "version must be X.Y.Z with three numeric components and no leading zeros".into(),
        );
    }
    Ok(())
}

fn asset_name(version: &str, target: &str) -> String {
    format!("codex-commit-v{version}-{target}.tar.gz")
}

fn archive_names(version: &str) -> BTreeSet<String> {
    TARGETS
        .iter()
        .map(|target| asset_name(version, target))
        .collect()
}

fn package(
    root: &Path,
    target_dir: &Path,
    target: &str,
    version: &str,
    output: &Path,
) -> Result<PathBuf> {
    validate_version(version)?;
    if !TARGETS.contains(&target) {
        return Err(format!("unsupported release target: {target}").into());
    }
    let binary = target_dir.join(target).join("release/codex-commit");
    validate_binary(&binary, target, version)?;
    fs::create_dir_all(output)?;
    let archive = output.join(asset_name(version, target));
    create_archive(&archive, &binary, root)?;
    Ok(archive)
}

fn validate_binary(binary: &Path, target: &str, version: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err("macOS release archives must be packaged on a native macOS host".into());
    }
    if cfg!(target_os = "macos") {
        let architecture = Command::new("lipo").arg("-archs").arg(binary).output()?;
        let expected = if target == TARGETS[0] {
            "arm64"
        } else {
            "x86_64"
        };
        if !architecture.status.success()
            || String::from_utf8(architecture.stdout)?.trim() != expected
        {
            return Err(
                format!("binary architecture does not match release target {target}").into(),
            );
        }
    }
    let actual = Command::new(binary).arg("--version").output()?;
    if !actual.status.success()
        || String::from_utf8(actual.stdout)?.trim_end_matches('\n')
            != format!("codex-commit {version}")
    {
        return Err("binary version does not match release version".into());
    }
    Ok(())
}

fn create_archive(archive: &Path, binary: &Path, root: &Path) -> Result<()> {
    let directory = archive.parent().ok_or("archive needs a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    {
        let encoder = GzEncoder::new(temporary.as_file_mut(), Compression::default());
        let mut tar = tar::Builder::new(encoder);
        // Construct headers from bytes instead of filesystem metadata: no
        // symlinks, xattrs, AppleDouble files, or host-specific ownership.
        for (name, source, mode) in [
            ("codex-commit", binary.to_path_buf(), 0o755),
            ("LICENSE", root.join("LICENSE"), 0o644),
            ("README.md", root.join("README.md"), 0o644),
        ] {
            let bytes = fs::read(source)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())?;
        }
        tar.into_inner()?.finish()?;
    }
    persist_public_file(temporary, archive)
}

fn digest_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn checksums(version: &str, directory: &Path) -> Result<PathBuf> {
    validate_version(version)?;
    let expected = archive_names(version);
    let found: BTreeSet<_> = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|name| name.ends_with(".tar.gz"))
        .collect();
    if found != expected {
        return Err(
            "release directory must contain exactly the two expected macOS archives".into(),
        );
    }
    let mut contents = String::new();
    for name in expected {
        contents.push_str(&format!(
            "{}  {name}\n",
            digest_file(&directory.join(&name))?
        ));
    }
    let output = directory.join("SHA256SUMS");
    write_atomic(&output, contents.as_bytes())?;
    Ok(output)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(contents)?;
    persist_public_file(temporary, path)
}

fn persist_public_file(mut temporary: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    temporary.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    temporary.persist(path)?;
    Ok(())
}

fn parse_checksums(version: &str, contents: &str) -> Result<BTreeMap<String, String>> {
    validate_version(version)?;
    let expected = archive_names(version);
    let mut parsed = BTreeMap::new();
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let bytes = line.as_bytes();
        if bytes.len() < 67
            || !bytes[..64].iter().all(u8::is_ascii_hexdigit)
            || bytes[64] != b' '
            || !matches!(bytes[65], b' ' | b'*')
        {
            return Err("malformed SHA256SUMS entry".into());
        }
        let filename = &line[66..];
        if !expected.contains(filename) {
            return Err(format!("unexpected release asset: {filename}").into());
        }
        if parsed
            .insert(filename.to_owned(), line[..64].to_ascii_lowercase())
            .is_some()
        {
            return Err(format!("duplicate release asset: {filename}").into());
        }
    }
    if parsed.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err("SHA256SUMS must contain both macOS release archives".into());
    }
    Ok(parsed)
}

fn render_formula(version: &str, contents: &str) -> Result<String> {
    let parsed = parse_checksums(version, contents)?;
    let arm = asset_name(version, TARGETS[0]);
    let intel = asset_name(version, TARGETS[1]);
    let arm_digest = &parsed[&arm];
    let intel_digest = &parsed[&intel];
    let base = format!("https://github.com/syz51/codex-commit/releases/download/v{version}");
    Ok(format!(
        r##"class CodexCommit < Formula
  desc "Commit Git changes with Codex Luna at Max reasoning, with optional push"
  homepage "https://github.com/syz51/codex-commit"
  license "MIT"

  depends_on :macos

  on_macos do
    on_arm do
      url "{base}/{arm}"
      sha256 "{arm_digest}"
    end
    on_intel do
      url "{base}/{intel}"
      sha256 "{intel_digest}"
    end
  end

  def install
    bin.install "codex-commit"
    generate_completions_from_executable(bin/"codex-commit", "--completions")
  end

  def caveats
    <<~EOS
      Git must be available on PATH; macOS Command Line Tools Git is supported.
      Install Codex CLI separately and run `codex login` before using codex-commit.
      The default mode grants Codex full local command access. Use --push to enable pushing.
    EOS
  end

  test do
    assert_match "codex-commit {version}", shell_output("#{{bin}}/codex-commit --version")
    assert_match "Usage:", shell_output("#{{bin}}/codex-commit --help")
    assert_match "#compdef codex-commit", (zsh_completion/"_codex-commit").read
    assert_match "--push", (zsh_completion/"_codex-commit").read
    assert_path_exists bash_completion/"codex-commit"
    assert_path_exists fish_completion/"codex-commit.fish"
    with_env(PATH: "/usr/bin:/bin") do
      system "git", "init", "-b", "main"
      output = shell_output("#{{bin}}/codex-commit --dry-run")
      assert_match "Push: disabled", output
      assert_match "Model: gpt-6-luna", output
      assert_match "Reasoning: max", output
    end
  end
end
"##
    ))
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?
        .trim_end_matches('\n')
        .to_owned())
}

fn verify_release_tag(root: &Path, tag: &str) -> Result<()> {
    let version = tag.strip_prefix('v').ok_or("release tag must be vX.Y.Z")?;
    validate_version(version)?;
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .current_dir(root)
        .output()?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr).trim()
        )
        .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let manifest = root.join("Cargo.toml");
    let package = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .find(|package| {
            package["manifest_path"]
                .as_str()
                .is_some_and(|path| Path::new(path) == manifest)
        })
        .ok_or("root package missing from cargo metadata")?;
    if package["version"].as_str() != Some(version) {
        return Err("release tag does not match root Cargo.toml package version".into());
    }
    let tagged = git(
        root,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )?;
    if git(root, &["rev-parse", "--verify", "HEAD"])? != tagged {
        return Err("HEAD does not match the release tag".into());
    }
    git(
        root,
        &[
            "merge-base",
            "--is-ancestor",
            &tagged,
            "refs/remotes/origin/main",
        ],
    )?;
    println!("Verified {tag} at {tagged}, reachable from origin/main.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    fn valid_checksums() -> String {
        TARGETS
            .iter()
            .map(|target| format!("{}  {}\n", "a".repeat(64), asset_name("0.2.0", target)))
            .collect()
    }

    #[test]
    fn formula_binds_both_assets_and_digests_to_the_version() {
        let formula = render_formula("0.2.0", &valid_checksums()).unwrap();
        assert_eq!(
            formula
                .matches(&format!("sha256 \"{}\"", "a".repeat(64)))
                .count(),
            2
        );
        for target in TARGETS {
            assert!(formula.contains(&format!("/v0.2.0/{}", asset_name("0.2.0", target))));
        }
        assert!(!formula.contains("depends_on \"git\""));
        assert!(formula.find("depends_on :macos").unwrap() < formula.find("on_macos do").unwrap());
        assert!(formula.contains("with_env(PATH: \"/usr/bin:/bin\")"));
        assert!(formula.contains("assert_match \"Push: disabled\""));
        assert!(formula.contains(
            "generate_completions_from_executable(bin/\"codex-commit\", \"--completions\")"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn generated_public_files_have_homebrew_compatible_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("codex-commit.rb");
        let formula = render_formula("0.2.0", &valid_checksums()).unwrap();
        write_atomic(&path, formula.as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), formula);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn metadata_rejects_missing_duplicate_unexpected_and_malformed_assets() {
        let valid = valid_checksums();
        let first = valid.lines().next().unwrap();
        for contents in [
            first.to_owned(),
            format!("{valid}{first}\n"),
            valid.replace("v0.2.0", "v0.3.0"),
            valid.replace(&"a".repeat(64), &"z".repeat(64)),
        ] {
            assert!(render_formula("0.2.0", &contents).is_err());
        }
    }

    #[test]
    fn version_rejects_injection_paths_and_non_semver_values() {
        for version in [
            "0.2.0\"; system \"false",
            "../0.2.0",
            "v0.2.0",
            "01.2.0",
            "0.2",
            "0.2.0-beta.1",
            "0.2.0\n",
        ] {
            assert!(render_formula(version, &valid_checksums()).is_err());
        }
    }

    #[test]
    fn archive_has_exact_public_files_regular_types_and_fixed_permissions() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("binary"), b"native binary fixture").unwrap();
        fs::write(root.join("LICENSE"), b"MIT").unwrap();
        fs::write(root.join("README.md"), b"public readme").unwrap();
        fs::write(root.join("._README.md"), b"must not ship").unwrap();
        let output = root.join("archive.tar.gz");
        create_archive(&output, &root.join("binary"), root).unwrap();
        let mut archive = tar::Archive::new(GzDecoder::new(File::open(output).unwrap()));
        let entries: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.header().entry_type().is_file());
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().mode().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("codex-commit".to_owned(), 0o755),
                ("LICENSE".to_owned(), 0o644),
                ("README.md".to_owned(), 0o644)
            ]
        );
    }

    #[test]
    fn checksum_generation_hashes_exact_assets_and_rejects_an_extra_archive() {
        let temporary = tempfile::tempdir().unwrap();
        for target in TARGETS {
            fs::write(temporary.path().join(asset_name("0.2.0", target)), b"abc").unwrap();
        }
        let path = checksums("0.2.0", temporary.path()).unwrap();
        let contents = fs::read_to_string(path).unwrap();
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(
            parse_checksums("0.2.0", &contents)
                .unwrap()
                .values()
                .collect::<Vec<_>>(),
            [&digest.to_owned(), &digest.to_owned()]
        );
        fs::write(temporary.path().join("unexpected.tar.gz"), b"extra").unwrap();
        assert!(checksums("0.2.0", temporary.path()).is_err());
    }

    #[test]
    fn package_rejects_unsupported_target_before_reading_any_binary() {
        let temporary = tempfile::tempdir().unwrap();
        let error = package(
            temporary.path(),
            temporary.path(),
            "x86_64-pc-windows-msvc",
            "0.2.0",
            temporary.path(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported release target"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_package_checks_version_and_architecture_before_creating_archive() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let target = if std::env::consts::ARCH == "aarch64" {
            TARGETS[0]
        } else {
            TARGETS[1]
        };
        let binary = root
            .join("target")
            .join(target)
            .join("release/codex-commit");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let source = root.join("fixture.rs");
        fs::write(&source, "fn main() { println!(\"codex-commit 0.2.0\"); }\n").unwrap();
        assert!(
            Command::new("rustc")
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("LICENSE"), b"MIT").unwrap();
        fs::write(root.join("README.md"), b"public readme").unwrap();
        let output = root.join("dist");
        let mismatch = package(root, &root.join("target"), target, "0.3.0", &output).unwrap_err();
        assert!(mismatch.to_string().contains("version does not match"));
        assert!(!output.exists());
        assert!(
            package(root, &root.join("target"), target, "0.2.0", &output)
                .unwrap()
                .is_file()
        );
        let other = if target == TARGETS[0] {
            TARGETS[1]
        } else {
            TARGETS[0]
        };
        assert!(
            validate_binary(&binary, other, "0.2.0")
                .unwrap_err()
                .to_string()
                .contains("architecture does not match")
        );
    }

    #[test]
    fn release_tag_must_match_package_head_and_main_ancestry() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"0.2.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"release-fixture\"\nversion = \"0.2.0\"\n",
        )
        .unwrap();
        let fixture_git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(root)
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
        };
        fixture_git(&["init", "-b", "main"]);
        fixture_git(&["add", "Cargo.toml", "Cargo.lock", "src/main.rs"]);
        fixture_git(&["commit", "-m", "Initial release fixture"]);
        fixture_git(&["tag", "v0.2.0"]);
        fixture_git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        verify_release_tag(root, "v0.2.0").unwrap();
        assert!(
            verify_release_tag(root, "v0.3.0")
                .unwrap_err()
                .to_string()
                .contains("package version")
        );
        fs::write(root.join("README.md"), "New fixture commit\n").unwrap();
        fixture_git(&["add", "README.md"]);
        fixture_git(&["commit", "-m", "Next fixture commit"]);
        assert!(
            verify_release_tag(root, "v0.2.0")
                .unwrap_err()
                .to_string()
                .contains("HEAD does not match")
        );
        // This is a private local test tag, never a published release.
        fixture_git(&["tag", "--force", "v0.2.0"]);
        assert!(
            verify_release_tag(root, "v0.2.0")
                .unwrap_err()
                .to_string()
                .contains("merge-base")
        );
    }
}
