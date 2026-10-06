use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
};

/// Project manifest parsed from `ryn.yaml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_owner")]
    pub owner: String,
    #[serde(default, deserialize_with = "null_default")]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    pub build: BuildConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Git { git: String, branch: Option<String> },
    Path { path: PathBuf },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    #[serde(default)]
    pub optimize: Optimize,
    #[serde(default)]
    pub debug: Option<Optimize>,
    #[serde(default)]
    pub release: Option<Optimize>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            optimize: Optimize::Speed,
            debug: None,
            release: None,
        }
    }
}

impl BuildConfig {
    pub fn optimize_for_profile(&self, release: bool) -> Optimize {
        if release {
            self.release.unwrap_or(self.optimize)
        } else {
            self.debug.unwrap_or(self.optimize)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Optimize {
    #[default]
    Speed,
    Size,
    None,
}

impl Optimize {
    pub(crate) fn codegen_value(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Size => "speed_and_size",
            Self::None => "none",
        }
    }
}

fn default_owner() -> String {
    "guest".into()
}

fn default_version() -> String {
    "0.1.0".into()
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Debug)]
pub struct ManifestError {
    pub path: PathBuf,
    message: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "error[R0410]: invalid ryn.yaml\n  --> {}\n{}",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    pub fn load_project(directory: &Path) -> Result<Self, ManifestError> {
        let path = directory.join("ryn.yaml");
        let text = fs::read_to_string(&path).map_err(|error| ManifestError {
            path: path.clone(),
            message: format!("cannot read manifest: {error}"),
        })?;
        Self::parse(&text).map_err(|message| ManifestError { path, message })
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = serde_yaml::from_str(text).map_err(|error| error.to_string())?;
        if manifest.name.trim().is_empty() {
            return Err("field `name` must not be empty".into());
        }
        if manifest.version.trim().is_empty() {
            return Err("field `version` must not be empty".into());
        }
        if manifest.owner.trim().is_empty() {
            return Err("field `owner` must not be empty".into());
        }
        if let Some(name) = manifest
            .dependencies
            .keys()
            .find(|name| name.trim().is_empty())
        {
            return Err(format!("dependency name `{name}` must not be empty"));
        }
        for (name, dependency) in &manifest.dependencies {
            let valid = match dependency {
                Dependency::Version(version) => !version.trim().is_empty(),
                Dependency::Git { git, branch } => {
                    !git.trim().is_empty()
                        && branch
                            .as_deref()
                            .is_none_or(|branch| !branch.trim().is_empty())
                }
                Dependency::Path { path } => !path.as_os_str().is_empty(),
            };
            if !valid {
                return Err(format!("dependency `{name}` has an empty source"));
            }
        }
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::{Dependency, Manifest, Optimize};

    #[test]
    fn parses_generated_style_manifest_and_empty_dependencies() {
        let manifest = Manifest::parse(
            "name: my-game\nversion: 0.1.0\nowner: guest\ndependencies:\nbuild:\n  optimize: speed\n",
        )
        .expect("manifest is valid");
        assert_eq!(manifest.name, "my-game");
        assert!(manifest.dependencies.is_empty());
        assert_eq!(manifest.build.optimize, Optimize::Speed);
    }

    #[test]
    fn reports_missing_name() {
        let error = Manifest::parse("version: 0.1.0\n").expect_err("name is required");
        assert!(error.contains("missing field `name`"));
    }

    #[test]
    fn rejects_malformed_yaml_and_unknown_optimization() {
        assert!(Manifest::parse("name: [unfinished\n").is_err());
        let error = Manifest::parse("name: demo\nbuild:\n  optimize: tiny\n")
            .expect_err("optimization name must be supported");
        assert!(error.contains("optimize"));
    }

    #[test]
    fn maps_supported_optimization_modes_to_cranelift_levels() {
        assert_eq!(
            Manifest::parse("name: demo\nbuild:\n  optimize: size\n")
                .unwrap()
                .build
                .optimize
                .codegen_value(),
            "speed_and_size"
        );
        assert_eq!(
            Manifest::parse("name: demo\nbuild:\n  optimize: none\n")
                .unwrap()
                .build
                .optimize
                .codegen_value(),
            "none"
        );
    }

    #[test]
    fn parses_registry_git_and_path_dependency_shapes() {
        let manifest = Manifest::parse(
            "name: demo\ndependencies:\n  gfx: 0.2.0\n  physics:\n    git: https://example.invalid/physics\n    branch: main\n  engine:\n    path: ../engine\n",
        )
        .expect("dependency source variants parse");
        assert!(matches!(
            manifest.dependencies["gfx"],
            Dependency::Version(_)
        ));
        assert!(matches!(
            manifest.dependencies["physics"],
            Dependency::Git { .. }
        ));
        assert!(matches!(
            manifest.dependencies["engine"],
            Dependency::Path { .. }
        ));
    }
}
