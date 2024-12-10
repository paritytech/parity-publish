use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{read_dir, read_to_string};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use cargo::core::{Package, Workspace};
use semver::VersionReq;
use termcolor::{Color, ColorSpec, WriteColor};
use toml_edit::{Formatted, Item, Table, Value};

use crate::changed::{find_indirect_changes, get_changed_crates, Change, ChangeKind};
use crate::cli::{Args, Prdoc, Semver};
use crate::plan::BumpKind;
use crate::public_api::{self, print_diff};
use crate::shared::read_stdin;

#[derive(Debug)]
pub struct DepChange {
    pub name: String,
    pub path: PathBuf,
    pub dep: String,
    pub breaking: bool,
}

#[derive(serde::Deserialize)]
struct Document {
    crates: Vec<Crates>,
}

#[derive(serde::Deserialize)]
struct Crates {
    name: String,
    #[serde(default)]
    bump: String,
}

pub fn get_prdocs(
    args: &Args,
    workspace: &Workspace,
    path: &Path,
    deps: bool,
    filter: &[String],
) -> Result<Vec<Change>> {
    let mut stderr = args.stderr();
    let mut entries = HashMap::new();

    if !path.exists() {
        stderr.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)).set_bold(true))?;
        write!(stderr, "warning: ")?;
        stderr.set_color(&ColorSpec::new())?;
        writeln!(stderr, "no PR Doc")?;
        return Ok(Vec::new());
    } else if path.is_file() {
        read_prdoc(path, workspace, &mut entries)?;
    } else {
        let dirs = read_dir(path).context("failed to read prdoc dir")?;

        for dir in dirs {
            let dir = dir.context("failed to read prdoc dir")?;

            if dir.path().extension().unwrap_or_default() != "prdoc" {
                continue;
            }

            read_prdoc(&dir.path(), workspace, &mut entries)?;
        }
    }

    let mut entries = entries.into_values().into_iter().collect::<Vec<_>>();

    if !filter.is_empty() {
        entries.retain(|e| filter.contains(&e.name));
    }

    if deps {
        find_indirect_changes(workspace, &mut entries);
    }
    Ok(entries)
}

fn read_prdoc(
    path: &Path,
    workspace: &Workspace<'_>,
    entries: &mut HashMap<String, Change>,
) -> Result<(), anyhow::Error> {
    let prdoc = read_to_string(path).context("failed to read prdoc")?;
    let prdoc: Document = serde_yaml::from_str(&prdoc)?;
    Ok(for c in prdoc.crates {
        let Some(package) = workspace.members().find(|m| m.name().as_str() == c.name) else {
            continue;
        };
        if package.publish().is_some() {
            continue;
        }
        let path = package.root().strip_prefix(workspace.root()).unwrap();
        let kind = ChangeKind::Files;
        let bump = match c.bump.as_str() {
            "patch" => BumpKind::Patch,
            "minor" => BumpKind::Minor,
            "none" => BumpKind::None,
            _ => BumpKind::Major,
        };
        let entry = entries.entry(c.name.to_string()).or_insert(Change {
            name: c.name.into(),
            path: path.into(),
            kind,
            bump,
        });
        entry.bump = entry.bump.max(bump);
    })
}

pub fn handle_prdoc(args: Args, mut prdoc: Prdoc) -> Result<()> {
    read_stdin(&mut prdoc.crates)?;
    let mut stdout = args.stdout();
    let config = cargo::GlobalContext::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let deps = !prdoc.no_deps;

    if prdoc.validate {
        return validate(&args, &prdoc, &workspace);
    }

    let prdocs = get_prdocs(&args, &workspace, &prdoc.prdoc_path, deps, &prdoc.crates)?;

    for c in prdocs {
        if prdoc.major && c.bump != BumpKind::Major {
            continue;
        }

        if prdoc.paths >= 2 {
            writeln!(stdout, "{}", c.path.join("Cargo.toml").display())?;
        } else if prdoc.paths == 1 {
            writeln!(stdout, "{}", c.path.display())?;
        } else if prdoc.quiet {
            writeln!(stdout, "{}", c.name)?;
        } else {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", c.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", c.path.display())?;
            writeln!(stdout, "    {}", c.kind)?;
            writeln!(stdout, "    {}", c.bump)?;
            writeln!(stdout)?;
        }
    }

    Ok(())
}

pub fn manifest_deps_changed(
    workspace: &Workspace,
    old: &Path,
    _new: &Path,
) -> Result<Vec<DepChange>> {
    let mut changes = Vec::new();
    let old_workspace = Workspace::new(&old.join("Cargo.toml"), workspace.gctx())?;
    let old_root =
        toml_edit::DocumentMut::from_str(&read_to_string(old_workspace.root_manifest())?)?;
    let new_root = toml_edit::DocumentMut::from_str(&read_to_string(workspace.root_manifest())?)?;

    for c in workspace.members() {
        let Some(old_c) = old_workspace.members().find(|o| o.name() == c.name()) else {
            continue;
        };
        if c.publish().is_some() {
            continue;
        }
        let new = toml_edit::DocumentMut::from_str(&read_to_string(c.manifest_path())?)?;
        let old = toml_edit::DocumentMut::from_str(&read_to_string(old_c.manifest_path())?)?;

        compare_deps(workspace, &mut changes, c, &old_root, &new_root, &old, &new)?;
    }

    Ok(changes)
}

fn compare_deps(
    workspace: &Workspace,
    changes: &mut Vec<DepChange>,
    c: &Package,
    old_root: &toml_edit::DocumentMut,
    new_root: &toml_edit::DocumentMut,
    old: &toml_edit::DocumentMut,
    new: &toml_edit::DocumentMut,
) -> Result<()> {
    let t = Table::new();

    let deps = new
        .get("dependencies")
        .and_then(|d| d.as_table())
        .unwrap_or(&t);

    for (name, _) in deps {
        let ((_old_pkg, old_dep, old_root_dep), (new_pkg, new_dep, new_root_dep)) = match (
            get_dep(workspace, name, old, old_root),
            get_dep(workspace, name, new, new_root),
        ) {
            (Err(_), Ok(_)) | (Ok(_), Err(_)) => {
                // removed or added
                changes.push(DepChange {
                    name: c.name().to_string(),
                    path: c
                        .root()
                        .strip_prefix(workspace.root())
                        .unwrap()
                        .to_path_buf(),
                    dep: name.to_string(),
                    breaking: false,
                });
                continue;
            }
            (Err(_), Err(_)) => {
                // strange, not found in both?
                continue;
            }
            (Ok(o), Ok(n)) => (o, n),
        };

        if workspace.members().any(|c| c.name() == new_pkg.as_str()) {
            continue;
        }

        let old_version = old_root_dep
            .as_ref()
            .and_then(|d| d.get("version"))
            .and_then(|d| d.as_str())
            .or_else(|| {
                old_root_dep
                    .as_ref()
                    .and_then(|d| d.get("version"))
                    .and_then(|d| d.as_str())
            });

        let new_version = new_root_dep
            .as_ref()
            .and_then(|d| d.get("version"))
            .and_then(|d| d.as_str())
            .map(|d| d.to_string())
            .or_else(|| {
                new_root_dep
                    .as_ref()
                    .and_then(|d| d.get("version"))
                    .and_then(|d| d.as_str())
                    .map(|d| d.to_string())
            });

        if let Some(old_version) = old_version {
            if let Some(new_version) = &new_version {
                let old_version = VersionReq::parse(&old_version)?;
                let new_version = VersionReq::parse(&new_version)?;
                if old_version.comparators[0].major != new_version.comparators[0].major
                    || (old_version.comparators[0].major == 0
                        && old_version.comparators[0].minor != new_version.comparators[0].minor)
                {
                    changes.push(DepChange {
                        name: c.name().to_string(),
                        path: c
                            .root()
                            .strip_prefix(workspace.root())
                            .unwrap()
                            .to_path_buf(),
                        dep: name.to_string(),
                        breaking: true,
                    });
                    continue;
                }
            }
        }

        if old_version.is_some() != new_version.is_some() {
            changes.push(DepChange {
                name: c.name().to_string(),
                path: c
                    .root()
                    .strip_prefix(workspace.root())
                    .unwrap()
                    .to_path_buf(),
                dep: name.to_string(),
                breaking: false,
            });
            continue;
        }

        let mut old_s = old_dep.clone();
        old_s.sort_values();
        Table::fmt(&mut old_s);

        let mut new_s = new_dep.clone();
        new_s.sort_values();
        Table::fmt(&mut new_s);

        if old_s.to_string() != new_s.to_string() {
            changes.push(DepChange {
                name: c.name().to_string(),
                path: c
                    .root()
                    .strip_prefix(workspace.root())
                    .unwrap()
                    .to_path_buf(),
                dep: name.to_string(),
                breaking: false,
            });
            continue;
        }

        let mut old_s = old_root_dep.clone().unwrap_or(Table::new());
        old_s.sort_values();
        Table::fmt(&mut old_s);

        let mut new_s = new_root_dep.unwrap_or(Table::new());
        new_s.sort_values();
        Table::fmt(&mut new_s);

        if old_s.to_string() != new_s.to_string() {
            changes.push(DepChange {
                name: c.name().to_string(),
                path: c
                    .root()
                    .strip_prefix(workspace.root())
                    .unwrap()
                    .to_path_buf(),
                dep: name.to_string(),
                breaking: false,
            });
            continue;
        }
    }

    Ok(())
}

fn get_dep<'a>(
    _workspace: &Workspace,
    name: &'a str,
    manifest: &toml_edit::DocumentMut,
    root_manifest: &toml_edit::DocumentMut,
) -> Result<(String, Table, Option<Table>)> {
    let dep = manifest
        .get("dependencies")
        .and_then(|d| d.as_table())
        .and_then(|d| d.get(name))
        .and_then(|d| {
            d.as_str()
                .map(|v| {
                    let mut t = Table::new();
                    t.insert(
                        "version",
                        Item::Value(Value::String(Formatted::new(v.to_string()))),
                    );
                    t
                })
                .or_else(|| {
                    d.as_inline_table()
                        .map(|d| d.clone().into_table())
                        .or_else(|| d.clone().into_table().ok())
                })
        })
        .context("no dep")?;
    let root_dep = root_manifest
        .get("workspace")
        .and_then(|d| d.get("dependencies"))
        .and_then(|d| d.as_table())
        .and_then(|d| d.get(name))
        .and_then(|d| {
            d.as_str()
                .map(|v| {
                    let mut t = Table::new();
                    t.insert(
                        "version",
                        Item::Value(Value::String(Formatted::new(v.to_string()))),
                    );
                    t
                })
                .or_else(|| {
                    d.as_inline_table()
                        .map(|d| d.clone().into_table())
                        .or_else(|| d.clone().into_table().ok())
                })
        });

    let pkg = root_dep
        .as_ref()
        .and_then(|d| d.get("package"))
        .and_then(|d| d.as_str())
        .or_else(|| dep.get("package").and_then(|d| d.as_str()))
        .unwrap_or(name)
        .to_string();
    Ok((pkg, dep, root_dep))
}

fn validate(args: &Args, prdoc: &Prdoc, w: &Workspace) -> Result<()> {
    let mut stdout = args.stdout();

    let Some(from) = &prdoc.since else {
        bail!("--since must be specified for --validate");
    };

    writeln!(stdout, "PR Doc validation is best effort")?;
    writeln!(
        stdout,
        "We can only detect the minimum guaranteed semver change"
    )?;
    writeln!(
        stdout,
        "It's possible to not detect changes or for changes to be greater than detected"
    )?;
    writeln!(stdout, "Always reason about semver changes yourself")?;
    writeln!(stdout)?;

    writeln!(stdout, "validating prdocs...")?;
    let prdocs = get_prdocs(args, w, &prdoc.prdoc_path, false, &prdoc.crates)?;

    let max_bump = prdoc.max_bump;

    writeln!(stdout, "checking file changes...")?;
    let mut changes = get_changed_crates(w, false, from, "HEAD")?;
    let mut ok = true;

    let mut crates = prdocs
        .iter()
        .map(|p| p.name.clone())
        //.chain(changes.iter().map(|c| c.name.clone()))
        .collect::<Vec<_>>();
    crates.sort();
    crates.dedup();
    let breaking = Semver {
        paths: 0,
        quiet: true,
        major: false,
        verbose: false,
        minimum_nightly_rust_version: false,
        since: Some(from.clone()),
        crates,
        toolchain: prdoc.toolchain.clone(),
    };

    let (tmp, upstreams) = public_api::get_from_commit(&w, &breaking, from)?;

    writeln!(stdout, "checking dep changes...")?;
    let dep_changes = manifest_deps_changed(w, tmp.path(), w.root())?;

    if !prdocs.is_empty() {
        writeln!(stdout, "checking semver changes...")?;
        let breaking =
            public_api::get_changes(args, w, upstreams, &breaking, &dep_changes, prdoc.verbose)?;

        writeln!(stdout)?;

        for prdoc in &prdocs {
            let changed = changes.iter().any(|c| c.name == prdoc.name);
            let api_change = breaking.iter().find(|c| c.name == prdoc.name);

            let predicted = api_change.map(|b| b.bump).unwrap_or_else(|| {
                if changed {
                    BumpKind::Patch
                } else {
                    BumpKind::None
                }
            });

            if prdoc.bump == predicted
                || (prdoc.bump == BumpKind::None && predicted == BumpKind::Patch)
            {
                continue;
            }

            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", prdoc.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", prdoc.path.display())?;
            write!(stdout, "    Change stated in PR Doc: ")?;
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            writeln!(stdout, "{}", prdoc.bump)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            write!(stdout, "    Predicted semver change: ")?;
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            writeln!(stdout, "{}", predicted)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;

            if let Some(max_allowed_bump) = max_bump {
                let prdoc_bad = prdoc.bump > max_allowed_bump;
                let predicted_bad = predicted > max_allowed_bump;

                if prdoc_bad || predicted_bad {
                    let location = if prdoc_bad && predicted_bad {
                        "Specified and detected"
                    } else if prdoc_bad {
                        "Specified"
                    } else {
                        "Detected"
                    };

                    write!(stdout, "    {} bump exceeds allowed bump level: ", location,)?;
                    stdout.set_color(ColorSpec::new().set_bold(true))?;
                    write!(stdout, "{}", prdoc.bump.max(predicted))?;
                    stdout.set_color(ColorSpec::new().set_bold(false))?;
                    write!(stdout, " > ")?;
                    stdout.set_color(ColorSpec::new().set_bold(true))?;
                    writeln!(stdout, "{}", max_allowed_bump)?;
                    stdout.set_color(ColorSpec::new().set_bold(false))?;
                    ok = false;
                }
            }

            if let Some(api_change) = api_change {
                if api_change.bump == BumpKind::Major && prdoc.bump != BumpKind::Major {
                    writeln!(
                        stdout,
                        "    Major API change found but prdoc specified {}",
                        prdoc.bump
                    )?;
                    ok = false;
                }
                if api_change.bump == BumpKind::Minor && prdoc.bump == BumpKind::Patch {
                    // just warn don't return 1 for this
                    writeln!(
                        stdout,
                        "    Minor API change found but prdoc specified {}",
                        prdoc.bump
                    )?;
                    ok = false;
                }
                print_diff(args, &api_change)?;
            }

            writeln!(stdout)?;
        }
    }

    for pkg in dep_changes {
        if !changes.iter().any(|c| c.name == pkg.name) {
            changes.push(Change {
                name: pkg.name.clone(),
                path: pkg.path.clone(),
                kind: ChangeKind::Dependency,
                bump: BumpKind::Minor,
            });
        }
    }
    //changes.extend(dep_changes);
    changes.dedup_by(|a, b| a.name == b.name);
    for change in &changes {
        if prdocs.iter().any(|p| p.name == change.name) {
            continue;
        }

        stdout.set_color(ColorSpec::new().set_bold(true))?;
        write!(stdout, "{}", change.name)?;
        stdout.set_color(ColorSpec::new().set_bold(false))?;
        writeln!(stdout, " ({}):", change.path.display())?;
        match change.kind {
            ChangeKind::Files => {
                writeln!(stdout, "    Files changed but crate not listed in PR Doc")?
            }
            ChangeKind::Manifest => writeln!(
                stdout,
                "    Cargo.toml changed but crate not listed in PR Doc"
            )?,
            ChangeKind::Dependency => writeln!(stdout, "    Dependency changed")?,
        }
        ok = false;
        writeln!(stdout)?;
    }

    if !ok {
        std::process::exit(1);
    }

    Ok(())
}
