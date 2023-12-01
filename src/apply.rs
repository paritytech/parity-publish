use anyhow::{Context, Result};
use cargo::{
    core::{resolver::CliFeatures, Workspace},
    ops::{Packages, PublishOpts},
    util::{auth::Secret, toml_mut::manifest::LocalManifest},
};

use semver::Version;

use std::{
    env,
    io::Write,
    ops::Add,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Apply, config, edit, plan::Planner, registry};

pub async fn handle_apply(apply: Apply) -> Result<()> {
    let path = apply.path.canonicalize()?;
    env::set_current_dir(&path)?;

    let plan = std::fs::read_to_string(apply.path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let cargo_config = cargo::Config::default()?;
    cargo_config
        .shell()
        .set_verbosity(cargo::core::Verbosity::Quiet);

    let workspace = Workspace::new(&path.join("Cargo.toml"), &cargo_config)?;

    let config = config::read_config(&path)?;

    let token = if apply.publish {
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
            .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?
    } else {
        String::new()
    };

    writeln!(stdout, "rewriting manifests...")?;

    config::apply_config(&workspace, &config)?;

    for pkg in &plan.crates {
        let mut manifest = LocalManifest::try_new(&path.join(&pkg.path).join("Cargo.toml"))?;
        edit::set_version(&mut manifest, &pkg.to)?;
        edit::fix_description(&mut manifest, &pkg.name)?;
        edit::rewrite_deps(&path, &plan, &mut manifest, &pkg.rewrite_dep)?;

        for remove_feature in &pkg.remove_feature {
            edit::remove_feature(&mut manifest, remove_feature)?;
        }

        manifest.write()?;
    }

    if !apply.publish {
        return Ok(());
    }

    publish(&apply, &cargo_config, plan, &path, token)
}

fn publish(
    apply: &Apply,
    config: &cargo::Config,
    plan: Planner,
    path: &Path,
    token: String,
) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut n = 1;

    let workspace = Workspace::new(&path.join("Cargo.toml"), config)?;

    let _lock = config.acquire_package_cache_lock()?;
    let mut reg = registry::get_registry(&workspace)?;
    registry::download_crates(&mut reg, &workspace, false)?;

    let skipped = plan
        .crates
        .iter()
        .filter(|c| c.publish)
        .filter(|pkg| version_exists(&mut reg, &pkg.name, &pkg.to))
        .count();
    let total = plan.crates.iter().filter(|c| c.publish).count() - skipped;

    writeln!(
        stdout,
        "Publishing {} packages ({} skipped)",
        total, skipped
    )?;

    for pkg in plan.crates.iter().filter(|c| c.publish) {
        if version_exists(&mut reg, &pkg.name, &pkg.to) {
            continue;
        }

        writeln!(
            stdout,
            "({:3<}/{:3<}) publishing {}-{}...",
            n, total, pkg.name, pkg.to
        )?;

        n += 1;

        let wait = Duration::from_secs(60);
        let now = Instant::now();

        let opts = PublishOpts {
            config,
            token: Some(Secret::from(token.clone())),
            index: None,
            verify: pkg.verify && !apply.dry_run && !apply.no_verify,
            allow_dirty: apply.allow_dirty,
            jobs: None,
            keep_going: false,
            to_publish: Packages::Packages(vec![pkg.name.clone()]),
            targets: Vec::new(),
            dry_run: apply.dry_run,
            registry: None,
            cli_features: CliFeatures::new_all(false),
        };
        cargo::ops::publish(&workspace, &opts)?;

        if let Some(delay) = now.add(wait).checked_duration_since(Instant::now()) {
            thread::sleep(delay);
        }
    }

    Ok(())
}

fn version_exists(reg: &mut cargo::sources::RegistrySource, name: &str, ver: &str) -> bool {
    let c = registry::get_crate(reg, name.to_string().into());
    let ver = Version::parse(ver).unwrap();

    if let Ok(c) = c {
        if c.iter().any(|v| v.version() == &ver) {
            return true;
        }
    }

    false
}
