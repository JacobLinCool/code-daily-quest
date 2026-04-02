use std::env;
use std::fs;
use std::io::Cursor;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Archive;

const DEFAULT_REPOSITORY: &str = "JacobLinCool/code-daily-quest";
const GITHUB_API_BASE: &str = "https://api.github.com";
const BINARY_NAME: &str = "code-daily-quest";

#[derive(Debug, Clone, Copy)]
pub enum UpdateMode {
    Check,
    Apply,
}

pub fn run(mode: UpdateMode) -> Result<()> {
    let updater = Updater::new()?;
    match mode {
        UpdateMode::Check => updater.check(),
        UpdateMode::Apply => updater.apply(),
    }
}

struct Updater {
    client: Client,
    repository: String,
    current_version: Version,
    target: &'static str,
}

impl Updater {
    fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            repository: release_repository(),
            current_version: current_version()?,
            target: release_target()?,
        })
    }

    fn check(&self) -> Result<()> {
        let release = self.fetch_latest_release()?;
        if release.version <= self.current_version {
            println!(
                "{BINARY_NAME} {} is up to date for {}",
                self.current_version, self.target
            );
            return Ok(());
        }

        println!(
            "Update available for {}: {} -> {}",
            self.target, self.current_version, release.version
        );
        println!("Release: {}", release.html_url);
        println!("Run `code-daily-quest update apply` to install it.");
        Ok(())
    }

    fn apply(&self) -> Result<()> {
        let release = self.fetch_latest_release()?;
        if release.version <= self.current_version {
            println!(
                "{BINARY_NAME} {} is already the newest published version.",
                self.current_version
            );
            return Ok(());
        }

        let archive_name = archive_name(self.target);
        let checksum_name = checksum_name(self.target);
        let archive_url = release.asset_url(&archive_name)?;
        let checksum_url = release.asset_url(&checksum_name)?;

        let tempdir = tempfile::tempdir().context("unable to create update tempdir")?;
        let archive_path = tempdir.path().join(&archive_name);
        let checksum_path = tempdir.path().join(&checksum_name);

        self.download_to_path(archive_url, &archive_path)?;
        self.download_to_path(checksum_url, &checksum_path)?;
        verify_checksum(&archive_path, &checksum_path)?;

        let extracted_binary = extract_binary(&archive_path, tempdir.path())?;
        let current_executable =
            env::current_exe().context("unable to resolve current executable")?;
        replace_executable(&extracted_binary, &current_executable)?;

        println!(
            "Updated {BINARY_NAME} from {} to {}",
            self.current_version, release.version
        );
        Ok(())
    }

    fn fetch_latest_release(&self) -> Result<LatestRelease> {
        let url = format!(
            "{GITHUB_API_BASE}/repos/{}/releases/latest",
            self.repository
        );
        let response = self
            .client
            .get(&url)
            .send()
            .with_context(|| format!("unable to query latest release from {url}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!("no published release found for {}", self.repository);
        }

        let response = response
            .error_for_status()
            .with_context(|| format!("GitHub release lookup failed with {status}"))?;
        let api_release: GitHubRelease = response
            .json()
            .context("unable to parse latest release metadata")?;
        LatestRelease::from_api(api_release)
    }

    fn download_to_path(&self, url: &str, path: &Path) -> Result<()> {
        let response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("unable to download {url}"))?;
        let status = response.status();
        let response = response
            .error_for_status()
            .with_context(|| format!("download failed with {status} for {url}"))?;
        let bytes = response.bytes().context("unable to read download body")?;
        fs::write(path, &bytes).with_context(|| format!("unable to write {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug)]
struct LatestRelease {
    tag_name: String,
    version: Version,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

impl LatestRelease {
    fn from_api(api_release: GitHubRelease) -> Result<Self> {
        Ok(Self {
            version: parse_version_tag(&api_release.tag_name)?,
            tag_name: api_release.tag_name,
            html_url: api_release.html_url,
            assets: api_release.assets,
        })
    }

    fn asset_url(&self, expected_name: &str) -> Result<&str> {
        self.assets
            .iter()
            .find(|asset| asset.name == expected_name)
            .map(|asset| asset.browser_download_url.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "release {} is missing asset {}",
                    self.tag_name,
                    expected_name
                )
            })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn build_client() -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("code-daily-quest-updater"),
    );

    if let Ok(token) = env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .context("invalid GITHUB_TOKEN header value")?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }

    Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .context("unable to build GitHub release client")
}

fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("invalid package version")
}

fn release_repository() -> String {
    env::var("CODE_DAILY_QUEST_REPO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REPOSITORY.to_string())
}

fn release_target() -> Result<&'static str> {
    release_target_from(env::consts::OS, env::consts::ARCH)
}

fn release_target_from(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        _ => bail!("unsupported release target for {os}/{arch}"),
    }
}

fn archive_name(target: &str) -> String {
    format!("{BINARY_NAME}-{target}.tar.gz")
}

fn checksum_name(target: &str) -> String {
    format!("{}.sha256", archive_name(target))
}

fn parse_version_tag(tag_name: &str) -> Result<Version> {
    let normalized = tag_name.trim().trim_start_matches('v');
    Version::parse(normalized).with_context(|| format!("invalid release tag: {tag_name}"))
}

fn verify_checksum(archive_path: &Path, checksum_path: &Path) -> Result<()> {
    let expected = read_expected_checksum(checksum_path)?;
    let actual = compute_sha256(archive_path)?;
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {}, got {}",
            archive_path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn read_expected_checksum(checksum_path: &Path) -> Result<String> {
    let checksum = fs::read_to_string(checksum_path)
        .with_context(|| format!("unable to read {}", checksum_path.display()))?;
    let token = checksum
        .split_whitespace()
        .next()
        .context("checksum file did not contain a hash")?;
    if token.len() != 64 || !token.chars().all(|ch| ch.is_ascii_hexdigit()) {
        bail!("checksum file {} is malformed", checksum_path.display());
    }
    Ok(token.to_ascii_lowercase())
}

fn compute_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn extract_binary(archive_path: &Path, output_root: &Path) -> Result<PathBuf> {
    let archive_bytes = fs::read(archive_path)
        .with_context(|| format!("unable to read {}", archive_path.display()))?;
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .context("unable to read tar archive entries")?
    {
        let mut entry = entry.context("unable to read tar archive entry")?;
        let path = entry.path().context("archive entry path was invalid")?;
        if path.file_name().and_then(|name| name.to_str()) == Some(BINARY_NAME) {
            let extracted_path = output_root.join(BINARY_NAME);
            entry
                .unpack(&extracted_path)
                .with_context(|| format!("unable to unpack {}", extracted_path.display()))?;
            #[cfg(unix)]
            {
                let mut permissions = fs::metadata(&extracted_path)?.permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&extracted_path, permissions)?;
            }
            return Ok(extracted_path);
        }
    }

    bail!(
        "archive {} did not contain {}",
        archive_path.display(),
        BINARY_NAME
    )
}

fn replace_executable(extracted_binary: &Path, destination: &Path) -> Result<()> {
    let destination_parent = destination
        .parent()
        .context("current executable did not have a parent directory")?;
    let staging_dir = tempfile::Builder::new()
        .prefix("code-daily-quest-update-")
        .tempdir_in(destination_parent)
        .with_context(|| {
            format!(
                "unable to create staging directory in {}",
                destination_parent.display()
            )
        })?;
    let staged_binary = staging_dir.path().join(BINARY_NAME);

    fs::copy(extracted_binary, &staged_binary).with_context(|| {
        format!(
            "unable to stage {} into {}",
            extracted_binary.display(),
            staged_binary.display()
        )
    })?;

    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&staged_binary)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&staged_binary, permissions)?;
    }

    fs::rename(&staged_binary, destination).with_context(|| {
        format!(
            "unable to replace executable at {}. reinstall into a writable directory first",
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Version, parse_version_tag, read_expected_checksum, release_target_from};

    #[test]
    fn supported_release_targets_are_mapped_explicitly() {
        assert_eq!(
            release_target_from("macos", "aarch64").unwrap(),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            release_target_from("macos", "x86_64").unwrap(),
            "x86_64-apple-darwin"
        );
        assert_eq!(
            release_target_from("linux", "x86_64").unwrap(),
            "x86_64-unknown-linux-gnu"
        );
        assert!(release_target_from("linux", "aarch64").is_err());
    }

    #[test]
    fn version_tags_accept_a_v_prefix() {
        assert_eq!(
            parse_version_tag("v1.2.3").unwrap(),
            Version::parse("1.2.3").unwrap()
        );
        assert_eq!(
            parse_version_tag("0.4.0").unwrap(),
            Version::parse("0.4.0").unwrap()
        );
    }

    #[test]
    fn checksum_parser_reads_the_first_column_only() {
        let tempdir = tempfile::tempdir().unwrap();
        let checksum_path = tempdir.path().join("artifact.sha256");
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        fs::write(&checksum_path, format!("{hash}  code-daily-quest.tar.gz\n")).unwrap();

        assert_eq!(read_expected_checksum(&checksum_path).unwrap(), hash);
    }
}
