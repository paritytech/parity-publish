use std::collections::HashSet;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::cli::Changed;
use anyhow::{bail, Result};
use cargo::core::dependency::DepKind;
use cargo::core::Workspace;
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};
use toml_edit::visit_mut::VisitMut;
use toml_edit::Table;

pub struct Change {
    pub name: String,
    pub path: PathBuf,
    pub kind: ChangeKind,
}

#[derive(Debug)]
pub enum ChangeKind {
    Files,
    Manifest,
    Dependency,
}

impl Display for ChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeKind::Files => f.write_str("files"),
            ChangeKind::Manifest => f.write_str("manifest"),
            ChangeKind::Dependency => f.write_str("dependency"),
        }
    }
}

pub async fn handle_changed(diff: Changed) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = diff.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let crates = get_changed_crates(&workspace, !diff.no_deps, &diff.from, &diff.to)?;

    for c in crates {
        if diff.paths >= 2 {
            writeln!(stdout, "{}", c.path.join("Cargo.toml").display())?;
        } else if diff.paths == 1 {
            writeln!(stdout, "{}", c.path.display())?;
        } else if diff.quiet {
            writeln!(stdout, "{}", c.name)?;
        } else {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", c.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", c.path.display())?;
            writeln!(stdout, "    {}", c.kind)?;
            writeln!(stdout)?;
        }
    }

    Ok(())
}

fn find_indirect_changes(w: &Workspace, changed: &mut Vec<Change>) {
    let mut dependants = HashSet::new();

    for c in w.members() {
        for dep in c
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
        {
            if changed
                .iter()
                .any(|ch| ch.name == dep.package_name().as_str())
            {
                dependants.insert(c.name().as_str());
            }
        }
    }

    loop {
        let mut did_something = false;

        for c in w.members() {
            for dep in c
                .dependencies()
                .iter()
                .filter(|d| d.kind() != DepKind::Development)
            {
                if dependants.iter().any(|d| *d == dep.package_name().as_str()) {
                    did_something |= dependants.insert(c.name().as_str());
                }
            }
        }

        if !did_something {
            break;
        }
    }

    for c in dependants {
        if !changed.iter().any(|ch| ch.name == c) {
            let path = w
                .members()
                .find(|cr| cr.name().as_str() == c)
                .unwrap()
                .root()
                .strip_prefix(w.root())
                .unwrap();
            let change = Change {
                name: c.to_string(),
                path: path.to_path_buf(),
                kind: ChangeKind::Dependency,
            };
            changed.push(change);
        }
    }
}

pub fn get_changed_crates(w: &Workspace, deps: bool, from: &str, to: &str) -> Result<Vec<Change>> {
    let changed_files = get_changed_files(w, from, to)?;
    let mut changed = Vec::new();
    let config = w.config();

    for c in w.members() {
        let path = c.root().strip_prefix(w.root()).unwrap();
        let mut src = cargo::sources::PathSource::new(c.root(), c.package_id().source_id(), config);
        src.update().unwrap();
        let src_files = src.list_files(c).unwrap();
        let mut src_files = src_files
            .into_iter()
            .map(|f| f.strip_prefix(w.root()).unwrap().display().to_string())
            .collect::<Vec<_>>();

        src_files.retain(|f| changed_files.contains(f));

        if src_files.len() == 1 && src_files[0].ends_with("/Cargo.toml") {
            if manifest_changed(w.root(), &src_files[0], from, to)? {
                let change = Change {
                    name: c.name().to_string(),
                    path: path.to_path_buf(),
                    kind: ChangeKind::Manifest,
                };
                changed.push(change);
            }
        } else if !src_files.is_empty() {
            let change = Change {
                name: c.name().to_string(),
                path: path.to_path_buf(),
                kind: ChangeKind::Files,
            };
            changed.push(change);
        }
    }

    if deps {
        find_indirect_changes(w, &mut changed);
    }

    changed.retain(|ch| {
        w.members()
            .find(|c| c.name().as_str() == ch.name)
            .unwrap()
            .publish()
            .is_none()
    });

    Ok(changed)
}

fn manifest_changed(root: &Path, path: &str, from: &str, to: &str) -> Result<bool> {
    struct Sorter;

    impl VisitMut for Sorter {
        fn visit_table_like_mut(&mut self, node: &mut dyn toml_edit::TableLike) {
            node.sort_values()
        }

        fn visit_array_mut(&mut self, node: &mut toml_edit::Array) {
            node.sort_by_key(|k| k.as_str().unwrap().to_string())
        }
    }

    let new = get_file(root, path, to)?;
    let old = if let Ok(old) = get_file(root, path, from) {
        old
    } else {
        return Ok(false);
    };

    let mut old = toml_edit::Document::from_str(&old)?;
    let mut new = toml_edit::Document::from_str(&new)?;

    for c in [&mut old, &mut new] {
        c.remove("dependencies");
        c.remove("build-dependencies");
        c.remove("dev-dependencies");

        let package = c.get_mut("package").unwrap().as_table_mut().unwrap();
        package.remove("version");
        package.remove("description");
        package.remove("license");

        Table::fmt(c);
        Sorter.visit_document_mut(c);
    }

    let changed = old.to_string() != new.to_string();
    Ok(changed)
}

fn get_file(root: &Path, path: &str, r: &str) -> Result<String> {
    let file = format!("{}:{}", r, path);

    let res = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(file)
        .output()?;

    if !res.status.success() {
        bail!("git exited non 0");
    }

    Ok(String::from_utf8(res.stdout)?)
}

fn get_changed_files(w: &Workspace, from: &str, to: &str) -> Result<HashSet<String>> {
    let root = w.root();

    let res = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("diff")
        .arg("--name-only")
        .arg(from)
        .arg(to)
        .output()?;

    if !res.status.success() {
        bail!("git exited non 0");
    }

    let files = std::str::from_utf8(&res.stdout)?
        .lines()
        .map(|s| s.to_string())
        .collect();
    Ok(files)
}

/*
pub fn get_crate_hash(c: &Package, r: &str) -> Result<String> {
    let path = c.manifest_path().parent().unwrap();
    let root = c.root();
    let res = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("ls-tree")
        .arg("--object-only")
        .arg(r)
        .arg(path)
        .output()?;

    if !res.status.success() {
        return Ok("".to_string());
    }

    let hash = std::str::from_utf8(&res.stdout)?.trim().to_string();
    Ok(hash)
}
*/
