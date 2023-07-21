use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::Result;
use cargo::core::{dependency::DepKind, Workspace};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Plan, shared};

#[derive(serde::Serialize, Default)]
struct Planner {
    publish: Vec<Publish>,
}

#[derive(serde::Serialize, Default)]
struct Publish {
    name: String,
    from: String,
    to: String,
    bump: String,
    path: PathBuf,
}

pub async fn handle_plan(plan: Plan) -> Result<()> {
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = plan.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let _cratesio = shared::cratesio()?;
    let crates = workspace
        .members()
        .filter(|m| m.publish().is_none())
        .map(|m| (m.name().as_str(), m))
        .collect::<HashMap<_, _>>();
    let mut deps = HashMap::new();
    let mut order = Vec::new();

    let _stdout = StandardStream::stdout(ColorChoice::Auto);
    let _stderr = StandardStream::stderr(ColorChoice::Auto);

    // map name to dpes
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

    for c in order {
        let c = crates.get(c).unwrap();
        planner.publish.push(Publish {
            name: c.name().to_string(),
            from: c.version().to_string(),
            to: c.version().to_string(),
            bump: "unkown".to_string(),
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
    println!("{}", output);

    Ok(())
}
