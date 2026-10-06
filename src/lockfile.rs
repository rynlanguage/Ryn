use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::manifest::{Dependency, Manifest};

const LOCK_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Lockfile {
    lock_version: u32,
    root: String,
    packages: Vec<LockedPackage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct LockedPackage {
    parent: String,
    name: String,
    version: String,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
}

pub fn write_project_lock(project: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(project)
        .map_err(|error| format!("error[R0430]: cannot resolve project: {error}"))?;
    let manifest = Manifest::load_project(&root).map_err(|error| error.to_string())?;
    let mut packages = Vec::new();
    let mut visited = HashSet::new();
    collect(
        &root,
        &manifest,
        &mut visited,
        &mut packages,
        &root,
        None,
        true,
    )?;
    packages.sort();
    let lock = Lockfile {
        lock_version: LOCK_VERSION,
        root: manifest.name,
        packages,
    };
    let path = root.join("ryn.lock");
    let text = serde_yaml::to_string(&lock)
        .map_err(|error| format!("error[R0430]: cannot serialize lockfile: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("error[R0430]: cannot write {}: {error}", path.display()))?;
    Ok(path)
}

pub fn validate_project_lock_if_present(project: &Path) -> Result<(), String> {
    let root = fs::canonicalize(project)
        .map_err(|error| format!("error[R0430]: cannot resolve project: {error}"))?;
    let path = root.join("ryn.lock");
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("error[R0430]: cannot read {}: {error}", path.display()))?;
    let locked: Lockfile = serde_yaml::from_str(&text)
        .map_err(|error| format!("error[R0430]: invalid {}: {error}", path.display()))?;
    let manifest = Manifest::load_project(&root).map_err(|error| error.to_string())?;
    let mut packages = Vec::new();
    let mut visited = HashSet::new();
    collect(
        &root,
        &manifest,
        &mut visited,
        &mut packages,
        &root,
        Some(&locked),
        false,
    )?;
    packages.sort();
    let current = Lockfile {
        lock_version: LOCK_VERSION,
        root: manifest.name,
        packages,
    };
    if locked != current {
        return Err(format!(
            "error[R0431]: {} does not match ryn.yaml or the resolved path dependencies; run `ryn lock {}` to update it",
            path.display(),
            root.display()
        ));
    }
    Ok(())
}

fn collect(
    root: &Path,
    manifest: &Manifest,
    visited: &mut HashSet<PathBuf>,
    packages: &mut Vec<LockedPackage>,
    project_root: &Path,
    lock: Option<&Lockfile>,
    refresh_git: bool,
) -> Result<(), String> {
    let canonical = fs::canonicalize(root).map_err(|error| {
        format!(
            "error[R0430]: cannot resolve package {}: {error}",
            root.display()
        )
    })?;
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }
    for (name, dependency) in &manifest.dependencies {
        let (dependency_root, source, branch, revision) = match dependency {
            Dependency::Path { path } => {
                let resolved = fs::canonicalize(canonical.join(path)).map_err(|error| {
                    format!(
                        "error[R0430]: cannot resolve path dependency `{name}` at {}: {error}",
                        canonical.join(path).display()
                    )
                })?;
                (resolved, format!("path:{}", path.display()), None, None)
            }
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch.as_deref() {
                    validate_git_branch(branch)?;
                }
                let old = lock.and_then(|lock| {
                    lock.packages
                        .iter()
                        .find(|entry| entry.parent == manifest.name && entry.name == *name)
                });
                if !refresh_git
                    && old.is_none_or(|entry| {
                        entry.source != format!("git:{git}") || entry.branch != *branch
                    })
                {
                    return Err(format!(
                        "error[R0431]: Git dependency `{name}` in `{}` is missing from or differs from ryn.lock; run `ryn lock {}`",
                        manifest.name,
                        project_root.display()
                    ));
                }
                let pinned_revision = (!refresh_git)
                    .then(|| old.and_then(|entry| entry.revision.as_deref()))
                    .flatten();
                let (resolved, commit) =
                    resolve_git(project_root, git, branch.as_deref(), pinned_revision)?;
                (resolved, format!("git:{git}"), branch.clone(), Some(commit))
            }
            Dependency::Version(_) => {
                return Err(format!(
                    "error[R0432]: cannot lock dependency `{name}` in `{}`: registry resolution is not implemented",
                    manifest.name
                ));
            }
        };
        let dependency_manifest =
            Manifest::load_project(&dependency_root).map_err(|error| error.to_string())?;
        if dependency_manifest.name != *name {
            return Err(format!(
                "error[R0430]: dependency key `{name}` resolves to package `{}`",
                dependency_manifest.name
            ));
        }
        packages.push(LockedPackage {
            parent: manifest.name.clone(),
            name: dependency_manifest.name.clone(),
            version: dependency_manifest.version.clone(),
            source,
            branch,
            revision,
        });
        collect(
            &dependency_root,
            &dependency_manifest,
            visited,
            packages,
            project_root,
            lock,
            refresh_git,
        )?;
    }
    Ok(())
}

pub fn resolve_locked_git_dependency(
    project: &Path,
    parent: &str,
    name: &str,
    url: &str,
    branch: Option<&str>,
) -> Result<PathBuf, String> {
    let root = fs::canonicalize(project)
        .map_err(|error| format!("error[R0430]: cannot resolve project: {error}"))?;
    let lock_path = root.join("ryn.lock");
    let text = fs::read_to_string(&lock_path).map_err(|error| {
        format!(
            "error[R0431]: Git dependency `{name}` requires {} (`ryn lock {}`): {error}",
            lock_path.display(),
            root.display()
        )
    })?;
    let lock: Lockfile = serde_yaml::from_str(&text)
        .map_err(|error| format!("error[R0430]: invalid {}: {error}", lock_path.display()))?;
    let entry = lock
        .packages
        .iter()
        .find(|entry| entry.parent == parent && entry.name == name)
        .ok_or_else(|| {
            format!("error[R0431]: Git dependency `{name}` is not pinned in ryn.lock")
        })?;
    if entry.source != format!("git:{url}") || entry.branch.as_deref() != branch {
        return Err(format!(
            "error[R0431]: Git dependency `{name}` does not match ryn.lock; run `ryn lock {}`",
            root.display()
        ));
    }
    let revision = entry
        .revision
        .as_deref()
        .ok_or_else(|| format!("error[R0431]: Git dependency `{name}` has no pinned commit"))?;
    if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "error[R0430]: invalid pinned commit for Git dependency `{name}`"
        ));
    }
    resolve_git(&root, url, branch, Some(revision)).map(|(path, _)| path)
}

fn resolve_git(
    project: &Path,
    url: &str,
    branch: Option<&str>,
    revision: Option<&str>,
) -> Result<(PathBuf, String), String> {
    use std::process::Command;

    if revision.is_some_and(|revision| {
        !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err("error[R0430]: invalid pinned Git commit hash".into());
    }
    if url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
    {
        return Err(
            "error[R0430]: Git URLs must not embed credentials; use a Git credential helper".into(),
        );
    }

    let mut hash = 0xcbf29ce484222325u64;
    for byte in url.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let cache = project
        .join(".ryn")
        .join("git")
        .join(format!("{hash:016x}"));
    let git_cache = git_compatible_path(&cache);
    if !cache.join(".git").is_dir() {
        if cache.exists() {
            fs::remove_dir_all(&cache).map_err(|error| {
                format!(
                    "error[R0433]: cannot replace Git cache {}: {error}",
                    cache.display()
                )
            })?;
        }
        if let Some(parent) = cache.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("error[R0433]: cannot create Git cache: {error}"))?;
        }
        let mut command = Command::new("git");
        command.arg("clone").arg("--depth").arg("1");
        if let Some(branch) = branch {
            command.arg("--branch").arg(branch);
        }
        let output = command
            .arg("--")
            .arg(url)
            .arg(&git_cache)
            .output()
            .map_err(|error| {
                format!("error[R0433]: cannot start Git while fetching `{url}`: {error}")
            })?;
        if !output.status.success() {
            return Err(format!(
                "error[R0433]: Git clone failed for `{url}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let mut checkout = Command::new("git");
    checkout.arg("-C").arg(&git_cache);
    if let Some(revision) = revision {
        let output = checkout
            .arg("checkout")
            .arg("--detach")
            .arg(revision)
            .output()
            .map_err(|error| format!("error[R0433]: cannot run Git checkout: {error}"))?;
        if !output.status.success() {
            let fetch = Command::new("git")
                .arg("-C")
                .arg(&git_cache)
                .arg("fetch")
                .arg("--depth")
                .arg("1")
                .arg("origin")
                .arg(revision)
                .output()
                .map_err(|error| {
                    format!("error[R0433]: cannot fetch pinned Git commit: {error}")
                })?;
            if !fetch.status.success() {
                return Err(format!(
                    "error[R0433]: cannot fetch locked commit {revision}: {}",
                    String::from_utf8_lossy(&fetch.stderr).trim()
                ));
            }
            let output = Command::new("git")
                .arg("-C")
                .arg(&git_cache)
                .arg("checkout")
                .arg("--detach")
                .arg(revision)
                .output()
                .map_err(|error| format!("error[R0433]: cannot run Git checkout: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "error[R0433]: cannot checkout locked commit {revision}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        }
    } else if revision.is_none() {
        let mut fetch_command = Command::new("git");
        fetch_command
            .arg("-C")
            .arg(&git_cache)
            .arg("fetch")
            .arg("--depth")
            .arg("1")
            .arg("origin");
        if let Some(branch) = branch {
            fetch_command.arg(branch);
        }
        let fetch = fetch_command
            .output()
            .map_err(|error| format!("error[R0433]: cannot refresh Git branch: {error}"))?;
        if !fetch.status.success() {
            return Err(format!(
                "error[R0433]: cannot fetch Git ref {branch:?} from `{url}`: {}",
                String::from_utf8_lossy(&fetch.stderr).trim()
            ));
        }
        let reset = Command::new("git")
            .arg("-C")
            .arg(&git_cache)
            .args(["reset", "--hard", "FETCH_HEAD"])
            .output()
            .map_err(|error| format!("error[R0433]: cannot update Git checkout: {error}"))?;
        if !reset.status.success() {
            return Err("error[R0433]: cannot update Git checkout".to_string());
        }
    }
    let clean = Command::new("git")
        .arg("-C")
        .arg(&git_cache)
        .args(["clean", "-fdx"])
        .output()
        .map_err(|error| format!("error[R0433]: cannot clean managed Git cache: {error}"))?;
    if !clean.status.success() {
        return Err("error[R0433]: cannot clean managed Git cache".to_string());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&git_cache)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("error[R0433]: cannot read Git revision: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "error[R0433]: cannot read Git revision for `{url}`"
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((cache, commit))
}

fn git_compatible_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(path) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\{path}"));
        }
        if let Some(path) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
    }
    path.to_path_buf()
}

fn validate_git_branch(branch: &str) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("check-ref-format")
        .arg("--branch")
        .arg(branch)
        .output()
        .map_err(|error| format!("error[R0433]: cannot validate Git branch name: {error}"))?;
    if !output.status.success() {
        return Err(format!("error[R0430]: invalid Git branch name `{branch}`"));
    }
    Ok(())
}
