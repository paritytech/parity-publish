use std::{
    collections::{HashMap, HashSet},
    io::Write,
    mem::take,
    path::PathBuf,
};

use anyhow::{ensure, Result};
use cargo::core::{dependency::DepKind, Package, Workspace};
use semver::Prerelease;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    changed::{diff_crate, download_crates},
    cli::Plan,
    shared::{self, parity_crate_owner_id},
};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Planner {
    pub publish: Vec<Publish>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Publish {
    pub name: String,
    pub from: String,
    pub to: String,
    pub bump: String,
    pub reason: String,
    pub path: PathBuf,
    pub rewrite_dep: Vec<RewriteDep>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct RewriteDep {
    pub name: String,
    pub version: String,
}

pub async fn handle_plan(plan: Plan) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = plan.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let mut upstream = HashMap::new();

    let cratesio = shared::cratesio()?;
    let workspace_crates = workspace
        .members()
        .filter(|m| m.publish().is_none())
        .map(|m| (m.name().as_str(), m))
        .collect::<HashMap<_, _>>();

    let mut deps = HashMap::new();
    let mut order = Vec::new();
    let mut own_all = true;

    writeln!(stdout, "looking up crates...",)?;

    for package in workspace_crates.values() {
        if package.publish().is_some() {
            continue;
        }

        let c = cratesio.full_crate(package.name().as_str(), true).await?;
        let parity_own = c
            .owners
            .iter()
            .any(|user| user.id == parity_crate_owner_id());
        if !parity_own {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
            writeln!(
                stdout,
                "{} exists and is owned by someone else",
                package.name()
            )?;
            stdout.set_color(ColorSpec::new().set_fg(None))?;
        }
        upstream.insert(package.name().as_str(), c);
        own_all &= parity_own;
    }

    ensure!(own_all, "we do not own all crates in the workspace");

    download_crates(&workspace, &upstream.values().cloned().collect::<Vec<_>>()).await?;

    println!("calculating plan...");

    // map name to deps
    for member in workspace.members() {
        if member.publish().is_some() {
            continue;
        }

        let deps_list = member
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
            .collect::<Vec<_>>();
        deps.insert(member.name().as_str(), deps_list);
    }

    let mut names = deps.keys().cloned().collect::<HashSet<_>>();

    while !deps.is_empty() {
        // strip out deps that are not in the workspace
        for deps in deps.values_mut() {
            deps.retain(|dep| names.contains(dep.package_name().as_str()))
        }

        deps.retain(|name, deps| {
            if deps.is_empty() {
                order.push(*name);
                names.remove(*name);
                false
            } else {
                true
            }
        });
    }

    let mut planner = Planner::default();
    let mut new_versions = HashMap::new();
    let mut breaking = HashSet::new();

    println!("order: {}", order.len());

    for c in order {
        let upstreamc = upstream.get(c).unwrap();
        let c = *workspace_crates.get(c).unwrap();

        let from =
            semver::Version::parse(upstreamc.max_stable_version.as_deref().unwrap_or("0.0.0"))
                .unwrap();
        let mut to = from.clone();
        let mut rewrite = Vec::new();

        if let Some(ref pre) = plan.pre {
        to.pre = Prerelease::new( pre).unwrap();
        }

        to.minor += 1;
        to.patch = 0;

        // if the version is already taken assume it's from a previous pre release and use this
        // version instead of making a new release
        if !upstreamc.versions.iter().any(|v| v.num == to.to_string()) && to.pre.is_empty() {
            println!("c");
            continue;
        }

        // we need to update the package even if nothing has changed if we're updating the deps in
        // a breaking way.
        let deps_breaking = c.dependencies().iter().any(|d| breaking.contains(d.package_name().as_str()));

        if let Some(ref max_ver) = upstreamc.max_stable_version {
            if let Some(_) = upstreamc.versions.iter().find(|v| &v.num == max_ver) {
                if !deps_breaking && !plan.all && !diff_crate(false, &config, c, &max_ver)? {
                    println!("b");
                    continue;
                }
            }
        }

        if to.major == 0 {
            breaking.insert(c.name().as_str());
        }

        // bump minor if version we want happens to already be taken
        /*loop {
            if !upstreamc.versions.iter().any(|v| v.num == to.to_string()) {
                break;
            }

            to.minor += 1;
        }

        if let Some(pre) = &plan.pre {
            to.pre = Prerelease::new(pre)?;
        } else {
            to.pre = old_pre;
        }*/

        new_versions.insert(c.name().to_string(), to.to_string());

        rewrite_deps(c, &workspace_crates, &new_versions, &upstream, &mut rewrite);

        planner.publish.push(Publish {
            name: c.name().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            bump: "unknown".to_string(),
            reason: "changed".to_string(),
            rewrite_dep: rewrite,
            path: c
                .manifest_path()
                .parent()
                .unwrap()
                .strip_prefix(path.parent().unwrap())
                .unwrap()
                .to_path_buf(),
        });
    }

    let output = toml::to_string_pretty(&planner)?;
    std::fs::write(plan.path.join("Plan.toml"), &output)?;
    writeln!(stdout, "plan generated")?;

    Ok(())
}

fn rewrite_deps(
    cra: &Package,
    workspace_crates: &HashMap<&str, &Package>,
    new_versions: &HashMap<String, String>,
    upstream: &HashMap<&str, crates_io_api::FullCrate>,
    rewrite: &mut Vec<RewriteDep>,
) {
    for dep in cra.dependencies() {
        if dep.kind() == DepKind::Development {
            continue;
        }

        if workspace_crates.contains_key(dep.package_name().as_str()) {
            if dep.source_id().is_git() || dep.source_id().is_path() {
                let new_ver = if let Some(ver) = new_versions.get(dep.package_name().as_str()) {
                    ver.to_string()
                } else {
                    upstream
                        .get(dep.package_name().as_str())
                        .unwrap()
                        .max_stable_version
                        .clone()
                        .unwrap()
                };

                rewrite.push(RewriteDep {
                    name: dep.package_name().to_string(),
                    version: new_ver,
                })
            }
        }
    }
}
