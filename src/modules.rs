use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    lockfile::resolve_locked_git_dependency,
    manifest::{Dependency, Manifest},
    parser,
    sema::{self, RynIr},
    source::{Diagnostic, SourceFile, Span},
};

#[derive(Clone)]
struct SourceUnit {
    path: PathBuf,
    module_path: String,
    text: String,
}

struct Segment {
    start: usize,
    end: usize,
    source: SourceUnit,
}

pub(crate) fn check_project(project: &Path) -> Result<(RynIr, Vec<PathBuf>), String> {
    let root = fs::canonicalize(project).map_err(|error| {
        format!(
            "error[R0420]: cannot resolve project directory {}: {error}",
            project.display()
        )
    })?;
    let source_root = fs::canonicalize(root.join("src")).map_err(|error| {
        format!(
            "error[R0420]: cannot resolve project source directory {}: {error}",
            root.join("src").display()
        )
    })?;
    let manifest = Manifest::load_project(&root).map_err(|error| error.to_string())?;
    let mut dependency_roots = HashMap::<String, PathBuf>::new();
    let mut visited_packages = HashSet::new();
    collect_path_dependencies(
        &root,
        &root,
        &manifest,
        None,
        &mut dependency_roots,
        &mut visited_packages,
    )?;
    let entry = source_root.join("main.ryn");
    if !entry.is_file() {
        return Err(format!(
            "error[R0420]: project entry point not found: {}",
            entry.display()
        ));
    }

    let mut units = Vec::new();
    let mut discovered = HashMap::<PathBuf, String>::new();
    load_module(
        &entry,
        "".into(),
        &source_root,
        &dependency_roots,
        &mut discovered,
        &mut units,
    )?;

    let mut text = String::new();
    let mut segments = Vec::with_capacity(units.len());
    let mut emitted_aliases = HashSet::new();
    for source in &units {
        if !text.is_empty() {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            // Each source file starts in the root namespace. Keep the synthetic
            // directive outside every source span so diagnostics still map cleanly.
            text.push_str("namespace;\n");
        }
        let importing_program = parser::parse(&source.text)
            .map_err(|error| render_local_diagnostic(error, &source.path, &source.text))?;
        for import in importing_program.uses {
            let module_path = import.path.join("::");
            let Some(module_alias) = import.path.last() else {
                continue;
            };
            for provider in units.iter().filter(|unit| unit.module_path == module_path) {
                let exported = parser::parse(&provider.text).map_err(|error| {
                    render_local_diagnostic(error, &provider.path, &provider.text)
                })?;
                for alias in exported.type_aliases.iter().filter(|alias| alias.public) {
                    let Some(equals) = provider.text[alias.span.start..alias.span.end].find('=')
                    else {
                        continue;
                    };
                    let right_hand_side =
                        provider.text[alias.span.start + equals + 1..alias.span.end].trim();
                    let (alias_namespace, local_name) = alias
                        .name
                        .rsplit_once("::")
                        .map_or((None, alias.name.as_str()), |(namespace, name)| {
                            (Some(namespace), name)
                        });
                    let namespace = alias_namespace.map_or_else(
                        || module_alias.clone(),
                        |namespace| format!("{module_alias}::{namespace}"),
                    );
                    let exported_name = format!("{namespace}::{local_name}");
                    let type_parameters = if alias.type_parameters.is_empty() {
                        String::new()
                    } else {
                        format!("<{}>", alias.type_parameters.join(", "))
                    };
                    if emitted_aliases.insert(exported_name) {
                        text.push_str(&format!(
                            "namespace {namespace};\ntype {local_name}{type_parameters} = {right_hand_side}\nnamespace;\n"
                        ));
                    }
                }
            }
        }
        let start = text.len();
        text.push_str(&source.text);
        let end = text.len();
        segments.push(Segment {
            start,
            end,
            source: source.clone(),
        });
        text.push('\n');
    }

    let mut program = parser::parse_recovering(&text).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| render_project_diagnostic(error, &segments))
            .collect::<Vec<_>>()
            .join("\n\n")
    })?;
    let root_names = program
        .functions
        .iter()
        .filter(|function| {
            segments.iter().any(|segment| {
                segment.source.module_path.is_empty()
                    && segment.start <= function.span.start
                    && function.span.start < segment.end
            })
        })
        .map(|function| function.name.clone())
        .collect::<Vec<_>>();
    for function in &mut program.functions {
        let Some(segment) = segments.iter().find(|segment| {
            segment.start <= function.span.start && function.span.start < segment.end
        }) else {
            continue;
        };
        function.module_path.clone_from(&segment.source.module_path);
        if !function.module_path.is_empty() {
            if root_names.contains(&function.name) {
                return Err(render_project_diagnostic(
                    Diagnostic {
                        code: "R0421",
                        message: format!(
                            "module function `{}` conflicts with a root function",
                            function.name
                        ),
                        span: function.span,
                        help: Some("rename the root or module function".into()),
                    },
                    &segments,
                ));
            }
            function.name = format!("{}::{}", function.module_path, function.name);
        }
    }
    for definition in &mut program.structs {
        if let Some(segment) = segments.iter().find(|segment| {
            segment.start <= definition.span.start && definition.span.start < segment.end
        }) {
            definition
                .module_path
                .clone_from(&segment.source.module_path);
        }
    }
    for definition in &mut program.enums {
        if definition.name.starts_with("$Ryn") {
            continue;
        }
        if let Some(segment) = segments.iter().find(|segment| {
            segment.start <= definition.span.start && definition.span.start < segment.end
        }) {
            definition
                .module_path
                .clone_from(&segment.source.module_path);
        }
    }
    for definition in &mut program.shapes {
        if let Some(segment) = segments.iter().find(|segment| {
            segment.start <= definition.span.start && definition.span.start < segment.end
        }) {
            definition
                .module_path
                .clone_from(&segment.source.module_path);
        }
    }
    for definition in &mut program.type_aliases {
        if let Some(segment) = segments.iter().find(|segment| {
            segment.start <= definition.span.start && definition.span.start < segment.end
        }) {
            definition
                .module_path
                .clone_from(&segment.source.module_path);
        }
    }

    let source_paths = segments
        .iter()
        .map(|segment| segment.source.path.clone())
        .collect();
    let ir = sema::analyze(program).map_err(|error| render_project_diagnostic(error, &segments))?;
    Ok((ir, source_paths))
}

fn collect_path_dependencies(
    project_root: &Path,
    package_root: &Path,
    manifest: &Manifest,
    expected_name: Option<&str>,
    package_roots: &mut HashMap<String, PathBuf>,
    visited_packages: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let package_root = fs::canonicalize(package_root).map_err(|error| {
        format!(
            "error[R0425]: cannot resolve package directory {}: {error}",
            package_root.display()
        )
    })?;
    if let Some(expected_name) = expected_name
        && manifest.name != expected_name
    {
        return Err(format!(
            "error[R0425]: path dependency key `{expected_name}` does not match package name `{}`",
            manifest.name
        ));
    }
    let source_root = fs::canonicalize(package_root.join("src")).map_err(|error| {
        format!(
            "error[R0425]: cannot resolve source directory for package `{}`: {error}",
            manifest.name
        )
    })?;
    if let Some(previous) = package_roots.get(&manifest.name) {
        if previous != &source_root {
            return Err(format!(
                "error[R0425]: package `{}` resolves to multiple local paths: {} and {}",
                manifest.name,
                previous.display(),
                source_root.display()
            ));
        }
    } else {
        package_roots.insert(manifest.name.clone(), source_root);
    }
    if !visited_packages.insert(package_root.clone()) {
        return Ok(());
    }

    for (name, dependency) in &manifest.dependencies {
        let dependency_root = match dependency {
            Dependency::Path { path } => fs::canonicalize(package_root.join(path)).map_err(|error| {
                format!(
                    "error[R0425]: cannot resolve path dependency `{name}` of package `{}` at {}: {error}",
                    manifest.name,
                    package_root.join(path).display()
                )
            })?,
            Dependency::Git { git, branch } => resolve_locked_git_dependency(
                project_root,
                &manifest.name,
                name,
                git,
                branch.as_deref(),
            )?,
            Dependency::Version(_) => {
                return Err(format!(
                    "error[R0425]: registry dependency `{name}` in `{}` cannot be resolved yet",
                    manifest.name
                ));
            }
        };
        let dependency_manifest =
            Manifest::load_project(&dependency_root).map_err(|error| error.to_string())?;
        collect_path_dependencies(
            project_root,
            &dependency_root,
            &dependency_manifest,
            Some(name),
            package_roots,
            visited_packages,
        )?;
    }
    Ok(())
}

fn load_module(
    path: &Path,
    module_path: String,
    source_root: &Path,
    dependency_roots: &HashMap<String, PathBuf>,
    discovered: &mut HashMap<PathBuf, String>,
    units: &mut Vec<SourceUnit>,
) -> Result<(), String> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        format!(
            "error[R0420]: cannot resolve module file {}: {error}",
            path.display()
        )
    })?;
    if !canonical.starts_with(source_root) {
        return Err(format!(
            "error[R0420]: module source escapes {}: {}",
            source_root.display(),
            canonical.display()
        ));
    }
    if let Some(previous) = discovered.get(&canonical) {
        if previous != &module_path {
            return Err(format!(
                "error[R0423]: source file {} is imported under both `{previous}` and `{module_path}`",
                canonical.display()
            ));
        }
        // Marking before following imports gives module cycles defined behavior:
        // the graph is loaded once, then all function signatures are resolved together.
        return Ok(());
    }
    discovered.insert(canonical.clone(), module_path.clone());
    let source = SourceFile::load(&canonical).map_err(|error| {
        format!(
            "error[R0420]: cannot read module {}: {error}",
            canonical.display()
        )
    })?;
    let parsed = parser::parse_recovering(source.text()).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| render_local_diagnostic(error, &canonical, source.text()))
            .collect::<Vec<_>>()
            .join("\n\n")
    })?;
    for import in parsed.uses {
        let imported_path = import.path.join("::");
        let (module_root, module_file) =
            resolve_module_file(source_root, dependency_roots, &import.path).map_err(
                |message| format!("{}\n  imported from {}", message, canonical.display()),
            )?;
        load_module(
            &module_file,
            imported_path,
            &module_root,
            dependency_roots,
            discovered,
            units,
        )?;
    }
    units.push(SourceUnit {
        path: canonical,
        module_path,
        text: source.text().to_owned(),
    });
    Ok(())
}

fn resolve_module_file(
    source_root: &Path,
    dependency_roots: &HashMap<String, PathBuf>,
    components: &[String],
) -> Result<(PathBuf, PathBuf), String> {
    if components.is_empty() {
        return Err("error[R0420]: empty module path".into());
    }
    let (module_root, path_components) = if let Some(root) = dependency_roots.get(&components[0]) {
        (root.as_path(), &components[1..])
    } else {
        (source_root, components)
    };
    if path_components.is_empty() {
        return Err(format!(
            "error[R0420]: module path `{}` must name a module inside its package",
            components.join("::")
        ));
    }
    let mut base = module_root.to_path_buf();
    for component in path_components {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!(
                "error[R0420]: invalid module path `{}`",
                components.join("::")
            ));
        }
        base.push(component);
    }
    let file = base.with_extension("ryn");
    let directory_file = base.join("mod.ryn");
    let selected = if file.is_file() {
        file
    } else if directory_file.is_file() {
        directory_file
    } else {
        return Err(format!(
            "error[R0420]: module `{}` not found (looked for {} and {})",
            components.join("::"),
            file.display(),
            directory_file.display()
        ));
    };
    Ok((module_root.to_path_buf(), selected))
}

fn render_project_diagnostic(diagnostic: Diagnostic, segments: &[Segment]) -> String {
    if let Some(segment) = segments.iter().find(|segment| {
        segment.start <= diagnostic.span.start && diagnostic.span.start <= segment.end
    }) {
        let span = Span {
            start: diagnostic
                .span
                .start
                .saturating_sub(segment.start)
                .min(segment.source.text.len()),
            end: diagnostic
                .span
                .end
                .saturating_sub(segment.start)
                .min(segment.source.text.len()),
        };
        render_local_diagnostic(
            Diagnostic { span, ..diagnostic },
            &segment.source.path,
            &segment.source.text,
        )
    } else {
        diagnostic.to_string()
    }
}

fn render_local_diagnostic(diagnostic: Diagnostic, path: &Path, text: &str) -> String {
    let source = SourceFile::new(path.to_path_buf(), text.to_owned());
    diagnostic.render(&source)
}
