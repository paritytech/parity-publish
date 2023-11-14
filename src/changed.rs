use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;

use crate::cli::Changed;
use anyhow::{bail, Result};
use cargo::core::dependency::DepKind;
use cargo::core::Workspace;

pub struct Change {
    pub name: String,
    pub kind: ChangeKind,
}

#[derive(Debug)]
pub enum ChangeKind {
    Files,
    Manifest,
    Dependency,
}

pub async fn handle_changed(diff: Changed) -> Result<()> {
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = diff.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let crates = get_changed_crates(&workspace, &diff.from, &diff.to)?;

    for c in crates {
        println!("{} {:?}", c.name, c.kind);
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
            let change = Change {
                name: c.to_string(),
                kind: ChangeKind::Dependency,
            };
            changed.push(change);
        }
    }
}

pub fn get_changed_crates(w: &Workspace, from: &str, to: &str) -> Result<Vec<Change>> {
    let changed_files = get_changed_files(w, from, to)?;
    let mut changed = Vec::new();
    let config = w.config();

    for c in w.members() {
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
                    kind: ChangeKind::Manifest,
                };
                changed.push(change);
            }
        } else if !src_files.is_empty() {
            let change = Change {
                name: c.name().to_string(),
                kind: ChangeKind::Files,
            };
            changed.push(change);
        }
    }

    find_indirect_changes(w, &mut changed);

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
    let new = get_file(root, path, to)?;
    let old = if let Ok(old) = get_file(root, path, from) {
        old
    } else {
        return Ok(false);
    };

    let mut old = cargo::util::toml_mut::manifest::Manifest::from_str(&old)?;
    let mut new = cargo::util::toml_mut::manifest::Manifest::from_str(&new)?;

    for c in [&mut old, &mut new] {
        c.data.remove("dependencies");
        c.data.remove("build-dependencies");
        c.data.remove("dev-dependencies");
        c.data
            .get_mut("package")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .remove("version");
    }

    let changed = old.data.to_string() != new.data.to_string();
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
