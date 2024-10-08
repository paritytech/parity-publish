use crate::{
    cli::{Args, Check},
    shared::{cratesio, get_owners, Owner},
};

use std::{
    collections::{BTreeMap, BTreeSet},
    env::current_dir,
    io::Write,
    path::PathBuf,
    process::exit,
    sync::Arc,
};

use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, Workspace},
    util::VersionExt,
};
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};

struct NamePath {
    name: String,
    path: PathBuf,
}

#[derive(Default)]
struct Issues {
    name: String,
    path: PathBuf,
    no_desc: bool,
    no_repo: bool,
    no_license: bool,
    unpublished: bool,
    taken: bool,
    broken_readme: bool,
    prerelease: bool,
    version_zero: bool,
    needs_publish: Option<Vec<NamePath>>,
}

impl Issues {
    fn has_issue(&self) -> bool {
        self.no_license
            || self.taken
            || self.broken_readme
            || self.needs_publish.is_some()
            || self.no_desc
            || self.no_repo
            || self.unpublished
            || self.prerelease
            || self.version_zero
    }

    fn ret_err(&self, check: &Check) -> bool {
        let no_desc = self.no_desc && !check.allow_nonfatal;
        let no_repo = self.no_repo && !check.allow_nonfatal;
        let unpublished = self.no_desc && !check.allow_unpublished;
        self.no_license
            || self.taken
            || self.broken_readme
            || self.needs_publish.is_some()
            || self.prerelease
            || self.version_zero
            || no_desc
            || no_repo
            || unpublished
    }

    fn print(&self, check: &Check, stdout: &mut StandardStream) -> Result<()> {
        if !self.has_issue() {
            return Ok(());
        }

        if check.paths >= 2 {
            writeln!(stdout, "{}", self.path.join("Cargo.toml").display())?;
        } else if check.paths == 1 {
            writeln!(stdout, "{}", self.path.display())?;
        } else if check.quiet {
            writeln!(stdout, "{}", self.name)?;
        } else {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", self.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", self.path.display())?;

            if self.no_desc {
                writeln!(stdout, "    no description")?;
            }
            if self.no_repo {
                writeln!(stdout, "    no repository")?;
            }
            if self.no_license {
                writeln!(stdout, "    no license")?;
            }
            if self.unpublished {
                writeln!(stdout, "    unpublished on crates.io")?;
            }
            if self.taken {
                writeln!(stdout, "    owned by some one else on crates.io")?;
            }
            if self.broken_readme {
                writeln!(stdout, "    readme specified in Cargo.toml doesnt exist")?;
            }
            if self.version_zero {
                writeln!(stdout, "    version is 0.0.0. Should be at least 0.1.0")?;
            }
            if self.prerelease {
                writeln!(stdout, "    version should not be prerelease")?;
            }
            if let Some(ref deps) = self.needs_publish {
                writeln!(
                    stdout,
                    "    'publish = false' is set but this crate is a dependency of others"
                )?;
                writeln!(
                    stdout,
                    "    either publish this crate or don't publish the dependants:"
                )?;
                for dep in deps {
                    writeln!(stdout, "        {} ({})", dep.name, dep.path.display())?;
                }
            }

            writeln!(stdout)?;
        }

        Ok(())
    }
}

pub async fn handle_check(args: Args, chk: Check) -> Result<()> {
    exit(check(&args, chk).await?)
}

pub async fn check(args: &Args, check: Check) -> Result<i32> {
    let mut stdout = args.stdout();
    let issues = issues(&check).await?;

    for issue in &issues {
        issue.print(&check, &mut stdout)?;
    }

    if issues.iter().any(|i| i.ret_err(&check)) {
        Ok(1)
    } else {
        Ok(0)
    }
}

async fn issues(check: &Check) -> Result<Vec<Issues>> {
    let mut all_issues = Vec::new();

    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let path = current_dir()?.join("Cargo.toml");
    let config = cargo::GlobalContext::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path, &config)?;

    writeln!(stderr, "looking up crate data, this may take a while....")?;

    let owners = if check.no_check_owner {
        vec![Owner::Us; workspace.members().count()]
    } else {
        get_owners(&workspace, &Arc::new(cratesio()?)).await
    };

    writeln!(stderr, "checking crates....")?;

    let mut new_publish = BTreeMap::new();
    let mut should_publish = workspace
        .members()
        .filter(|c| c.publish().is_none())
        .flat_map(|c| c.dependencies())
        .filter(|d| d.kind() != DepKind::Development)
        .map(|d| d.package_name().as_str())
        .map(|d| (d, BTreeSet::new()))
        .collect::<BTreeMap<_, BTreeSet<&str>>>();

    loop {
        new_publish = workspace
            .members()
            .filter(|c| new_publish.contains_key(c.name().as_str()))
            .flat_map(|c| c.dependencies())
            .filter(|d| d.kind() != DepKind::Development)
            .map(|d| d.package_name().as_str())
            .map(|d| (d, BTreeSet::new()))
            .collect();

        if new_publish.is_empty() {
            break;
        }

        should_publish.extend(new_publish);
        new_publish = BTreeMap::new();
    }

    workspace
        .members()
        .filter(|c| c.publish().is_none())
        .for_each(|c| {
            should_publish.remove(c.name().as_str());
        });

    for c in workspace.members() {
        for dep in c
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
        {
            should_publish
                .entry(dep.package_name().as_str())
                .and_modify(|d| {
                    d.insert(c.name().as_str());
                });
        }
    }

    if check.recursive {
        loop {
            let mut did_something = false;
            for c in workspace.members() {
                for dep in c
                    .dependencies()
                    .iter()
                    .filter(|d| d.kind() != DepKind::Development)
                {
                    for deps in should_publish
                        .values_mut()
                        .filter(|d| d.contains(dep.package_name().as_str()))
                    {
                        did_something |= deps.insert(c.name().as_str());
                    }
                }
            }
            if !did_something {
                break;
            }
        }
    }

    for deps in should_publish.values_mut() {
        deps.retain(|dep| {
            workspace
                .members()
                .find(|c| c.name().as_str() == *dep)
                .map(|c| c.publish().is_none())
                .unwrap_or(false)
        })
    }

    for (c, owner) in workspace.members().zip(owners) {
        let path = c.root().strip_prefix(workspace.root())?;

        let mut issues = Issues {
            name: c.name().to_string(),
            path: path.to_path_buf(),
            ..Issues::default()
        };

        if c.publish().is_none() {
            match owner {
                Owner::Us => (),
                Owner::None => issues.unpublished = true,
                Owner::Other => issues.taken = true,
            }

            issues.no_desc = c.manifest().metadata().description.is_none();
            issues.no_repo = c.manifest().metadata().repository.is_none();
            issues.no_license = c.manifest().metadata().license.is_none()
                && c.manifest().metadata().license_file.is_none();

            if let Some(readme) = &c.manifest().metadata().readme {
                if !c
                    .manifest_path()
                    .parent()
                    .context("no parent")?
                    .join(readme)
                    .exists()
                {
                    issues.broken_readme = true;
                }
            }

            if c.version().major == 0 && c.version().minor == 0 {
                issues.version_zero = true;
            }
            if c.version().is_prerelease() {
                issues.prerelease = true;
            }
        }

        issues.needs_publish = should_publish.get(c.name().as_str()).map(|deps| {
            deps.iter()
                .map(|d| {
                    workspace
                        .members()
                        .find(|c| c.name().as_str() == *d)
                        .unwrap()
                })
                .map(|c| NamePath {
                    name: c.name().to_string(),
                    path: c
                        .root()
                        .strip_prefix(workspace.root())
                        .unwrap()
                        .to_path_buf(),
                })
                .collect()
        });

        all_issues.push(issues);
    }

    Ok(all_issues)
}
