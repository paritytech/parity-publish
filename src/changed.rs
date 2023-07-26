use std::collections::HashSet;
use std::env::temp_dir;
use std::fs::{create_dir_all, metadata, remove_dir_all, OpenOptions};
use std::io::Write;
use std::process::Command;
use std::{fs::create_dir, io::Cursor, path::Path};

use crate::cli::Changed;
use crate::shared;
use anyhow::{ensure, Result};
use cargo::core::{Package, Workspace};
use cargo::sources::PathSource;
use cargo::Config;
use crates_io_api::FullCrate;
use termcolor::{ColorChoice, StandardStream};

pub async fn handle_changed(diff: Changed) -> Result<()> {
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = diff.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let cratesio = shared::cratesio()?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let mut upstreams = Vec::new();

    for member in workspace.members().filter(|p| p.publish().is_none()) {
        let entry = cratesio.full_crate(&member.name(), false).await?;
        upstreams.push(entry);
    }

    download_crates(&workspace, &upstreams).await?;

    for member in workspace.members().filter(|p| p.publish().is_none()) {
        let entry = upstreams
            .iter()
            .find(|u| u.name == member.name().as_str())
            .unwrap();
        let version = if let Some(ver) = entry.versions.iter().find(|v| v.num == entry.max_version)
        {
            ver
        } else {
            continue;
        };

        if diff_crate(diff.verbose, &config, member, &version.num)? {
            writeln!(stdout, "{}", member.name())?;
        }
    }

    Ok(())
}

pub async fn download_crates(workspace: &Workspace<'_>, upstreams: &[FullCrate]) -> Result<()> {
    let _ = create_dir("download");
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    for member in workspace.members().filter(|p| p.publish().is_none()) {
        let entry = upstreams
            .iter()
            .find(|u| u.name == member.name().as_str())
            .unwrap();
        let version = &entry.max_version;

        let max_ver =
            if let Some(max_ver) = entry.versions.iter().find(|v| v.num == entry.max_version) {
                max_ver
            } else {
                continue;
            };

        let url = format!("https://crates.io{}", max_ver.dl_path);

        let path =
            Path::new("download").join(format!("{}-{}.crate.tar.gz", member.name(), version));

        if let Ok(stat) = metadata(&path) {
            if stat.len() != 0 {
                continue;
            }
        }

        let mut file = std::fs::File::create(path)?;

        writeln!(stderr, "downloading {}-{}...", member.name(), version)?;

        let response = reqwest::get(url).await?;
        let mut content = Cursor::new(response.bytes().await?);
        std::io::copy(&mut content, &mut file)?;
    }

    Ok(())
}

pub fn diff_crate(verbose: bool, config: &Config, member: &Package, version: &str) -> Result<bool> {
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    if verbose {
        writeln!(stderr, "diffing {}-{}...", member.name(), version)?;
    }

    let dir = temp_dir().join("parity-publish").join("crate");
    let prefix = dir.join(format!("{}-{}", member.name(), version));
    let _ = remove_dir_all(&dir);
    create_dir_all(&dir)?;

    let status = Command::new("tar")
        .arg("-xf")
        .arg(format!(
            "download/{}-{}.crate.tar.gz",
            member.name(),
            version
        ))
        .arg("-C")
        .arg(&dir)
        .status()?;
    ensure!(status.success(), "tar exited non 0");

    std::fs::rename(prefix.join("Cargo.toml.orig"), prefix.join("Cargo.toml"))?;

    let files =
        PathSource::new(&dir, member.package_id().source_id(), config).list_files(member)?;
    let mut files = files
        .into_iter()
        .map(|f| {
            f.strip_prefix(member.manifest_path().parent().unwrap())
                .unwrap()
                .to_path_buf()
        })
        .collect::<HashSet<_>>();

    let upstream_files = walkdir::WalkDir::new(&prefix)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let mut upstream_files = upstream_files
        .into_iter()
        .skip(1)
        .filter(|f| f.path().is_file())
        .map(|f| f.path().strip_prefix(&prefix).unwrap().to_path_buf())
        .collect::<HashSet<_>>();

    upstream_files.remove(Path::new(".cargo_vcs_info.json"));
    upstream_files.remove(Path::new("Cargo.toml.orig"));
    upstream_files.remove(Path::new("Cargo.lock"));
    files.remove(Path::new("Cargo.lock"));
    let mut changed = false;

    for file in &files {
        if !upstream_files.contains(file) {
            if verbose {
                writeln!(stderr, "new file {}", file.display())?;
            }
            changed = true;
        }
    }

    for file in &upstream_files {
        if !files.contains(file) {
            if verbose {
                writeln!(stderr, "file {} was deleted", file.display())?;
            }
            changed = true;
        }
    }

    for file in &upstream_files {
        if files.contains(file) {
            let f1 = OpenOptions::new().read(true).open(prefix.join(file))?;
            let f2 = OpenOptions::new()
                .read(true)
                .open(member.manifest_path().parent().unwrap().join(file))?;

            if f1.metadata()?.len() != f2.metadata()?.len()
                || std::fs::read(prefix.join(file))?
                    != std::fs::read(member.manifest_path().parent().unwrap().join(file))?
            {
                if verbose {
                    writeln!(stderr, "file changed {}", file.display())?;
                }
                changed = true;
            }
        }
    }

    Ok(changed)
}