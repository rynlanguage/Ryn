//! Client for the pods package registry.
//!
//! Network access goes through the system `curl` and archives through the
//! system `tar`, the same way Git dependencies use the system `git`, so the
//! compiler needs no TLS or compression code of its own. Every downloaded
//! archive is checked against the SHA-256 pinned in `ryn.lock` before it is
//! unpacked.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::manifest::{Dependency, Manifest};

pub const DEFAULT_REGISTRY: &str = "https://pods.ryn-lang.xyz";
/// Largest archive `ryn publish` uploads; the registry enforces the same limit.
const MAX_ARCHIVE_BYTES: u64 = 10 * 1024 * 1024;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The registry base URL: `RYN_REGISTRY` when set, otherwise pods.
pub fn registry_url() -> String {
    env::var("RYN_REGISTRY")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
}

/// The `source` value recorded in `ryn.lock` for registry packages.
pub fn lock_source(registry: &str) -> String {
    format!("registry:{registry}")
}

#[derive(Debug, Deserialize)]
struct ResolvedJson {
    version: String,
    checksum: String,
}

#[derive(Debug, Deserialize)]
struct ErrorJson {
    errors: Vec<ErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    detail: String,
}

#[derive(Debug, Deserialize)]
struct PublishedJson {
    name: String,
    version: String,
    url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub version: String,
    pub checksum: String,
}

// ---------------------------------------------------------------------------
// Versions

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

fn parse_version(text: &str) -> Option<(Version, bool)> {
    let text = text.trim();
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, !pre.is_empty()),
        None => (text, false),
    };
    let mut parts = core.split('.');
    let mut number = || -> Option<u64> {
        let part = parts.next()?;
        if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
            return None;
        }
        part.parse().ok()
    };
    let version = Version {
        major: number()?,
        minor: number()?,
        patch: number()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((version, pre))
}

/// Checks a Cargo-style requirement: `1.2.3`/`^1.2.3` (compatible), `~1.2.3`
/// (patch updates), `=1.2.3` (exact) or `*`. Pre-releases only match exactly.
pub fn version_matches(requirement: &str, candidate: &str) -> bool {
    let Some((version, pre)) = parse_version(candidate) else {
        return false;
    };
    let requirement = requirement.trim();
    if requirement.is_empty() || requirement == "*" {
        return !pre;
    }
    let (op, rest) = match requirement.as_bytes()[0] {
        b'^' | b'~' | b'=' => (requirement.as_bytes()[0], requirement[1..].trim()),
        _ => (b'^', requirement),
    };
    let Some((base, base_pre)) = parse_version(rest) else {
        return false;
    };
    if pre || base_pre {
        return op != b'~' && rest.trim() == candidate.trim();
    }
    match op {
        b'=' => version == base,
        b'~' => version >= base && version.major == base.major && version.minor == base.minor,
        _ => {
            version >= base
                && if base.major > 0 {
                    version.major == base.major
                } else if base.minor > 0 {
                    version.major == 0 && version.minor == base.minor
                } else {
                    version.major == 0 && version.minor == 0 && version.patch == base.patch
                }
        }
    }
}

pub fn valid_requirement(requirement: &str) -> bool {
    let requirement = requirement.trim();
    if requirement.is_empty() || requirement == "*" {
        return true;
    }
    let rest = requirement.trim_start_matches(['^', '~', '=']).trim();
    parse_version(rest).is_some()
}

// ---------------------------------------------------------------------------
// HTTP through curl

struct Response {
    status: u16,
    body: Vec<u8>,
}

fn temp_path(label: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("ryn-{label}-{}-{id}", std::process::id()))
}

/// Runs curl and returns the HTTP status and body. `headers` are written to a
/// temporary file so tokens never appear on a command line.
fn curl(
    method: &str,
    url: &str,
    headers: &[String],
    upload: Option<&Path>,
) -> Result<Response, String> {
    let body_file = temp_path("http-body");
    let header_file = temp_path("http-headers");
    let mut command = Command::new("curl");
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--proto")
        .arg("=https,http")
        .arg("--max-time")
        .arg("300")
        .arg("--request")
        .arg(method)
        .arg("--output")
        .arg(&body_file)
        .arg("--write-out")
        .arg("%{http_code}");
    if !headers.is_empty() {
        fs::write(&header_file, headers.join("\n") + "\n")
            .map_err(|error| format!("error[R0434]: cannot prepare request headers: {error}"))?;
        command
            .arg("--header")
            .arg(format!("@{}", header_file.display()));
    }
    if let Some(upload) = upload {
        command
            .arg("--data-binary")
            .arg(format!("@{}", upload.display()));
    }
    let output = command.arg("--").arg(url).output();
    let _ = fs::remove_file(&header_file);
    let output = output.map_err(|error| {
        format!("error[R0434]: cannot start curl to reach the package registry: {error}")
    })?;
    let body = fs::read(&body_file).unwrap_or_default();
    let _ = fs::remove_file(&body_file);
    if !output.status.success() {
        return Err(format!(
            "error[R0434]: cannot reach {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let status = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    Ok(Response { status, body })
}

fn error_message(response: &Response) -> String {
    serde_yaml::from_slice::<ErrorJson>(&response.body)
        .ok()
        .and_then(|errors| errors.errors.into_iter().next())
        .map(|error| error.detail)
        .unwrap_or_else(|| format!("HTTP {}", response.status))
}

fn encode(component: &str) -> String {
    let mut out = String::new();
    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Asks the registry for the newest non-yanked version matching `requirement`.
pub fn resolve(registry: &str, name: &str, requirement: &str) -> Result<Resolved, String> {
    if !valid_requirement(requirement) {
        return Err(format!(
            "error[R0435]: dependency `{name}` has an invalid version requirement `{requirement}`"
        ));
    }
    let url = format!(
        "{registry}/api/v1/packages/{}/resolve?req={}",
        encode(name),
        encode(requirement.trim())
    );
    let response = curl("GET", &url, &[], None)?;
    if response.status != 200 {
        return Err(format!(
            "error[R0435]: cannot resolve `{name}` {requirement} from {registry}: {}",
            error_message(&response)
        ));
    }
    let resolved: ResolvedJson = serde_yaml::from_slice(&response.body).map_err(|error| {
        format!("error[R0435]: unexpected registry response for `{name}`: {error}")
    })?;
    if !version_matches(requirement, &resolved.version) || !is_sha256(&resolved.checksum) {
        return Err(format!(
            "error[R0435]: registry returned an invalid version for `{name}` {requirement}"
        ));
    }
    Ok(Resolved {
        version: resolved.version,
        checksum: resolved.checksum.to_ascii_lowercase(),
    })
}

pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Lowercase hex SHA-256, the checksum format pods and ryn.lock use.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let data = fs::read(path)
        .map_err(|error| format!("error[R0436]: cannot read {}: {error}", path.display()))?;
    Ok(sha256_hex(&data))
}

/// The system tar. On Windows the bundled bsdtar is used explicitly: a GNU tar
/// from Git or MSYS on PATH would read `C:\...` as a remote host.
fn tar() -> Command {
    if cfg!(windows) {
        let system = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("System32")
            .join("tar.exe");
        if system.is_file() {
            return Command::new(system);
        }
    }
    Command::new("tar")
}

/// Downloads, verifies and unpacks one package version into the project's
/// registry cache, returning the package root (the directory with ryn.yaml).
pub fn fetch(
    project: &Path,
    registry: &str,
    name: &str,
    version: &str,
    checksum: &str,
) -> Result<PathBuf, String> {
    if !is_sha256(checksum) {
        return Err(format!(
            "error[R0436]: ryn.lock has an invalid checksum for `{name}` {version}"
        ));
    }
    if parse_version(version).is_none() || !valid_package_name(name) {
        return Err(format!(
            "error[R0436]: invalid locked package `{name}` {version}"
        ));
    }
    let cache = project
        .join(".ryn")
        .join("registry")
        .join(format!("{name}-{version}"));
    let marker = cache.join(".ryn-checksum");
    if fs::read_to_string(&marker).is_ok_and(|text| text.trim() == checksum) {
        return package_root(&cache, name);
    }
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|error| {
            format!("error[R0436]: cannot replace {}: {error}", cache.display())
        })?;
    }
    let url = format!(
        "{registry}/api/v1/packages/{}/{}/download",
        encode(name),
        encode(version)
    );
    let archive = temp_path("package.tar.gz");
    let response = curl("GET", &url, &[], None)?;
    if response.status != 200 {
        return Err(format!(
            "error[R0436]: cannot download `{name}` {version}: {}",
            error_message(&response)
        ));
    }
    fs::write(&archive, &response.body)
        .map_err(|error| format!("error[R0436]: cannot store the download: {error}"))?;
    let actual = sha256_file(&archive)?;
    if actual != checksum {
        let _ = fs::remove_file(&archive);
        return Err(format!(
            "error[R0436]: checksum mismatch for `{name}` {version}: ryn.lock pins {checksum}, the download is {actual}"
        ));
    }
    let unpack = cache.with_extension("unpack");
    let _ = fs::remove_dir_all(&unpack);
    fs::create_dir_all(&unpack)
        .map_err(|error| format!("error[R0436]: cannot create {}: {error}", unpack.display()))?;
    let output = tar()
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpack)
        .output()
        .map_err(|error| format!("error[R0436]: cannot start tar: {error}"));
    let _ = fs::remove_file(&archive);
    let output = output?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&unpack);
        return Err(format!(
            "error[R0436]: cannot unpack `{name}` {version}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    fs::rename(&unpack, &cache)
        .map_err(|error| format!("error[R0436]: cannot move package into the cache: {error}"))?;
    fs::write(&marker, checksum)
        .map_err(|error| format!("error[R0436]: cannot write {}: {error}", marker.display()))?;
    package_root(&cache, name)
}

/// Archives made with `tar -C dir name` keep a top-level `name/` directory.
fn package_root(cache: &Path, name: &str) -> Result<PathBuf, String> {
    if cache.join("ryn.yaml").is_file() {
        return Ok(cache.to_path_buf());
    }
    let nested = cache.join(name);
    if nested.join("ryn.yaml").is_file() {
        return Ok(nested);
    }
    Err(format!(
        "error[R0436]: package `{name}` in {} has no ryn.yaml",
        cache.display()
    ))
}

pub fn valid_package_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

// ---------------------------------------------------------------------------
// Credentials

fn credentials_path() -> PathBuf {
    let base = if cfg!(windows) {
        env::var_os("APPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.unwrap_or_else(env::temp_dir)
        .join("ryn")
        .join("credentials")
}

/// Saves a registry token for `ryn publish`.
pub fn login(token: &str) -> Result<PathBuf, String> {
    let token = token.trim();
    if token.is_empty()
        || token
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err("error[R0437]: the token must be a single word, such as pods_xxxxxxxx".into());
    }
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("error[R0437]: cannot create {}: {error}", parent.display())
        })?;
    }
    fs::write(&path, format!("{token}\n"))
        .map_err(|error| format!("error[R0437]: cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn token() -> Result<String, String> {
    if let Some(token) = env::var("RYN_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    {
        return Ok(token.trim().to_string());
    }
    let path = credentials_path();
    fs::read_to_string(&path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            "error[R0437]: no registry token; run `ryn login <token>` or set RYN_TOKEN".into()
        })
}

// ---------------------------------------------------------------------------
// Publishing

fn copy_tree(from: &Path, to: &Path, total: &mut u64) -> Result<(), String> {
    fs::create_dir_all(to)
        .map_err(|error| format!("error[R0438]: cannot create {}: {error}", to.display()))?;
    let entries = fs::read_dir(from)
        .map_err(|error| format!("error[R0438]: cannot read {}: {error}", from.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("error[R0438]: cannot read directory: {error}"))?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "error[R0438]: cannot inspect {}: {error}",
                entry.path().display()
            )
        })?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || file_type.is_symlink() {
            continue;
        }
        let target = to.join(&name);
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target, total)?;
        } else if file_type.is_file() {
            *total += fs::copy(entry.path(), &target).map_err(|error| {
                format!(
                    "error[R0438]: cannot copy {}: {error}",
                    entry.path().display()
                )
            })?;
        }
    }
    Ok(())
}

/// Packs a project the way `ryn publish` uploads it and returns the archive.
/// Only ryn.yaml, src/, and top-level README, LICENSE, and CHANGELOG files
/// are included.
pub fn package(project: &Path) -> Result<(PathBuf, Manifest), String> {
    let manifest = Manifest::load_project(project).map_err(|error| error.to_string())?;
    if !valid_package_name(&manifest.name) {
        return Err(format!(
            "error[R0438]: package name `{}` must start with a lowercase letter and use only a-z, 0-9, `-` and `_`",
            manifest.name
        ));
    }
    if parse_version(&manifest.version).is_none() {
        return Err(format!(
            "error[R0438]: version `{}` must be MAJOR.MINOR.PATCH",
            manifest.version
        ));
    }
    for (name, dependency) in &manifest.dependencies {
        match dependency {
            Dependency::Version(requirement) if valid_requirement(requirement) => {}
            Dependency::Version(requirement) => {
                return Err(format!(
                    "error[R0438]: dependency `{name}` has an invalid version requirement `{requirement}`"
                ));
            }
            Dependency::Path { .. } | Dependency::Git { .. } => {
                return Err(format!(
                    "error[R0438]: dependency `{name}` uses a path or Git source; published packages may only depend on registry packages"
                ));
            }
        }
    }
    let src = project.join("src");
    if !src.is_dir() {
        return Err("error[R0438]: a package needs its sources in src/".into());
    }
    let stage = temp_path("publish");
    let root = stage.join(&manifest.name);
    let mut total = 0;
    copy_tree(&src, &root.join("src"), &mut total)?;
    fs::copy(project.join("ryn.yaml"), root.join("ryn.yaml"))
        .map_err(|error| format!("error[R0438]: cannot copy ryn.yaml: {error}"))?;
    if let Ok(entries) = fs::read_dir(project) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let upper = name.to_ascii_uppercase();
            if entry.path().is_file()
                && ["README", "LICENSE", "LICENCE", "CHANGELOG", "COPYING"]
                    .iter()
                    .any(|prefix| upper.starts_with(prefix))
            {
                total += fs::copy(entry.path(), root.join(&name)).unwrap_or(0);
            }
        }
    }
    let archive = stage.join(format!("{}-{}.tar.gz", manifest.name, manifest.version));
    let output = tar()
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage)
        .arg(&manifest.name)
        .output()
        .map_err(|error| format!("error[R0438]: cannot start tar: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "error[R0438]: cannot create the package archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let size = fs::metadata(&archive)
        .map(|meta| meta.len())
        .unwrap_or(total);
    if size > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "error[R0438]: the package archive is {} KB; the registry accepts at most {} KB",
            size / 1024,
            MAX_ARCHIVE_BYTES / 1024
        ));
    }
    Ok((archive, manifest))
}

/// Uploads a packed project. Returns the published package's page URL.
pub fn publish(project: &Path) -> Result<String, String> {
    let token = token()?;
    let (archive, manifest) = package(project)?;
    let registry = registry_url();
    let response = curl(
        "PUT",
        &format!("{registry}/api/v1/packages/new"),
        &[
            format!("Authorization: Bearer {token}"),
            "Content-Type: application/gzip".into(),
            format!("X-Ryn-Version: {}", env!("CARGO_PKG_VERSION")),
        ],
        Some(&archive),
    );
    if let Some(stage) = archive.parent() {
        let _ = fs::remove_dir_all(stage);
    }
    let response = response?;
    if response.status != 200 {
        return Err(format!(
            "error[R0439]: publishing {} {} failed: {}",
            manifest.name,
            manifest.version,
            error_message(&response)
        ));
    }
    let published: PublishedJson = serde_yaml::from_slice(&response.body)
        .map_err(|error| format!("error[R0439]: unexpected registry response: {error}"))?;
    Ok(published.url.unwrap_or_else(|| {
        format!(
            "{registry}/packages/{}/{}",
            published.name, published.version
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::{valid_package_name, valid_requirement, version_matches};

    #[test]
    fn caret_tilde_exact_and_any_requirements() {
        assert!(version_matches("0.2.0", "0.2.3"));
        assert!(!version_matches("0.2.0", "0.3.0"));
        assert!(!version_matches("0.2.5", "0.2.4"));
        assert!(version_matches("^1.2.0", "1.9.9"));
        assert!(!version_matches("^1.2.0", "2.0.0"));
        assert!(version_matches("0.0.3", "0.0.3"));
        assert!(!version_matches("0.0.3", "0.0.4"));
        assert!(version_matches("~1.4.0", "1.4.7"));
        assert!(!version_matches("~1.4.0", "1.5.0"));
        assert!(version_matches("=1.4.2", "1.4.2"));
        assert!(!version_matches("=1.4.2", "1.4.3"));
        assert!(version_matches("*", "3.1.4"));
        assert!(!version_matches("*", "3.1.4-beta"));
        assert!(version_matches("=2.0.0-beta", "2.0.0-beta"));
        assert!(!version_matches("1.0.0", "1.0"));
    }

    #[test]
    fn requirement_and_name_validation() {
        assert!(
            valid_requirement("1.2.3") && valid_requirement("^0.1.0") && valid_requirement("*")
        );
        assert!(!valid_requirement("1.2") && !valid_requirement("latest"));
        assert!(valid_package_name("rynix") && valid_package_name("vec-math_2"));
        assert!(
            !valid_package_name("Rynix") && !valid_package_name("2d") && !valid_package_name("")
        );
    }
}
