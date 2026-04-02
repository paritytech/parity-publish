# Parity Publish

Parity publish is a tool to publish and manage publishing cargo workspaces
to crates.io.

Parity publish aims to be be an all in one tool to manage everything to do
with crates, crate.io and publishing releases. Manage maintaining past
releases with backports and patch releases. And ensure polkadot-sdk git repro
stays in a healthy state when it comes to publishing.

## Environment Variables

| Variable | Required for | Description |
|---|---|---|
| `PARITY_PUBLISH_CRATESIO_TOKEN` | Production publishing | API token for crates.io |
| `PARITY_PUBLISH_STAGING_CRATESIO_TOKEN` | Staging publishing (`--staging`) | API token for staging.crates.io |

## Commands

There are a bunch of commands for doing various things.

### Check

Checks crates in the workspace for errors that would prevent publishing.
Currently this tool checks for:

- No description (non fatal)
- No license
- Specified README file doesn't exist
- Crate is unpublished on crates.io (non fatal)
- Crate is taken by some one else on crates.io
- Crate is `publish = false` and is a dependent of a crate with `publish = true`
- Crate is `publish = true` but depends on a crate that is `publish = false`

Publish issues are solved recursively down the dependency chain and up the
dependency chain if `--recursive` is passed.

#### CI

There is a `parity-publish check` CI workflow running on https://github.com/paritytech/polkadot-sdk/.

#### Example

```
morganamilo@songbird % parity-pubish check

pallet-tx-pause (substrate/frame/tx-pause):
    readme specified in Cargo.toml doesn't exist

bp-test-utils (cumulus/bridges/primitives/test-utils):
    'publish = false' is set but this crate is a dependency of others
    either publish this crate or don't publish the dependants:
        pallet-bridge-grandpa (cumulus/bridges/modules/grandpa)

pallet-bridge-grandpa (cumulus/bridges/modules/grandpa):
    no description

pallet-bridge-parachains (cumulus/bridges/modules/parachains):
    no description

cumulus-client-cli (cumulus/client/cli):
    no description
    no license

polkadot-performance-test (polkadot/node/test/performance-test):
    'publish = false' is set but this crate is a dependency of others
    either publish this crate or don't publish the dependants:
        polkadot-cli (polkadot/cli)

cumulus-test-relay-sproof-builder (cumulus/test/relay-sproof-builder):
    'publish = false' is set but this crate is a dependency of others
    either publish this crate or don't publish the dependants:
        cumulus-primitives-parachain-inherent (cumulus/primitives/parachain-inherent)

parachain-info (cumulus/parachains/pallets/parachain-info):
    no description
    no license
    owned by some one else on crates.io

...
```

### Claim

The claim command looks for unpublished crates in the workspace and publishes
empty v0.0.0 releases so that other's can't take our crates between the time
they are added to git and a release happens.

#### CI

There is a `parity-publish claim` CI workflow running on https://github.com/paritytech/polkadot-sdk/.

#### Example

```
morganamilo@songbird % parity-pubish claim

publishing node-executor...
publishing node-testing...
node-inspect is set to not publish
rococo-parachain-runtime is set to not publish
parachain-info exists and is owned by someone else
sp-api-test is set to not publish
sp-application-crypto-test is set to not publish
sp-arithmetic-fuzzer is set to not publish
sp-consensus-sassafras is set to not publish
sp-npos-elections-fuzzer is set to not publish
sp-runtime-interface-test is set to not publish
```

### Status

This command gives a general overview of crate versions and ownership.

#### Example

```
morganamilo@songbird % parity-pubish check

Crate                                             Local Ver       crates.io Ver   Owner
bridge-runtime-common                             0.1.0           0.5.0           Parity
bp-header-chain                                   0.1.0           0.5.0           Parity
bp-runtime                                        0.1.0           0.5.0           Parity
frame-support                                     4.0.0-dev       26.0.0          Parity
frame-support-procedural                          4.0.0-dev       21.0.0          Parity
frame-support-procedural-tools                    4.0.0-dev       9.0.0           Parity
frame-support-procedural-tools-derive             3.0.0           10.0.0          Parity
sp-api                                            4.0.0-dev       24.0.0          Parity
sp-api-proc-macro                                 4.0.0-dev       13.0.0          Parity
sp-core                                           21.0.0          26.0.0          Parity
sp-core-hashing                                   9.0.0           13.0.0          Parity
sp-debug-derive                                   8.0.0           12.0.0          Parity
sp-externalities                                  0.19.0          0.23.0          Parity
sp-std                                            8.0.0           12.0.0          Parity
sp-storage                                        13.0.0          17.0.0          Parity
sp-runtime-interface                              17.0.0          22.0.0          Parity
sp-runtime-interface-proc-macro                   11.0.0          15.0.0          Parity
...
```

### Config

This command applies the changes specified in `Plan.config` to the cargo workspace.

This config file allows you to specify changes you wish to be made every release.

### Changed

The changed command shows which crates have changed between two git commits.

Currently this command can detect three kinds of changes:

- File changes - source code and other file have changed
- Manifest changes - the Cargo.toml has changed
- Dependency - a dependency of a crate has changed

#### Example

```
morganamilo@songbird % parity-pubish changed FROM TO

bridge-runtime-common (bridges/bin/runtime-common):
    manifest

sp-io (substrate/primitives/io):
    files

sp-keystore (substrate/primitives/keystore):
    manifest

sp-state-machine (substrate/primitives/state-machine):
    files

sp-runtime (substrate/primitives/runtime):
    files

sp-application-crypto (substrate/primitives/application-crypto):
    manifest

polkadot-node-subsystem (polkadot/node/subsystem):
    dependency

polkadot-statement-distribution (polkadot/node/network/statement-distribution):
    dependency

sp-mmr-primitives (substrate/primitives/merkle-mountain-range):
    dependency
```

### Plan

The plan command is the starting point for running a release.

The plan command generates a `Plan.toml` which outlines all the crates in the workspace,
which ones should be published, what version numbers should be used and if any git
dependencies should be removed or substituted for registry dependencies.

A plan can be generated a few ways:

```
# generate a new plan where we release all crates
parity-publish plan --new --all

# generate a new plan where we release crates that changed since the v1.2.3 git tag
parity-publish plan --new --since=v1.2.3

# generate a new plan where we release specific crates
parity-publish plan --new foo bar
```

`--pre=dev.1` can be used to generate pre release version numbers.

A plan file doesn't do much on it's own. It's just a record of everything we would
like to be done for the release. Once a plan file is generated, running a release
from the plan should be a reproducible process, always ending up with the same
end result when applied.

Once a release has been ran, patch releases can be done by running `parity-publish plan --patch foo`.
This will patch version bump the crate `foo` in the plan ready to be applied.

### Example

```
morganamilo@songbird % parity-pubish plan --new --since=release-crates-io-v1.4.0

looking up crate data, this may take a while....
checking crates....
349 packages changed 146 indirect
calculating order...
looking up crates...
calculating plan...
plan generated 461 packages 349 to publish
```

### Apply

Apply reads the `Plan.toml` file and makes the changes needed to make crates suitable
for publishing to crates.io. It does quite a handful of things, following the crate order
in `Plan.toml`.

`parity-publish apply` does the following things:

- Removes any reference to dev dependencies from feature values
- Fill in missing descriptions
- Replace workspace path dependencies with crates.io version numbers
- Replaces git dependencies with crates.io releases if there are any
- If there are optional git dependencies with no releases
    - The dependency will be removed completely
    - The features that activate this dependency will be removed completely
    - Features that weakly reference the dependency will be removed from the feature values
    - Any references to the features that have been removed will be removed recursively
    - Any crates that require the feature unconditionally are removed from the workspace

Once the changes have been applied, they can be double checked and commited. Then
`parity-publish apply --publish` will start publishing the crates.

#### Sequential publishing (default)

By default, crates are published one at a time with a 15 second delay between each.
With large workspaces this can take a very long time.

#### Parallel publishing

Use `--jobs` / `-j` to publish multiple crates in parallel:

```
parity-publish apply --publish -j 8
parity-publish apply --publish -j 8 --no-verify
```

#### Staging registry

By default, crates are published to crates.io. Use `--staging` to publish to
staging.crates.io instead. When `--staging` is used, the token is read from
`PARITY_PUBLISH_STAGING_CRATESIO_TOKEN` instead of `PARITY_PUBLISH_CRATESIO_TOKEN`.

```
parity-publish apply --publish --staging
parity-publish apply --publish --staging -j 8 --no-verify
```

Note: the `staging` registry must be configured in `.cargo/config.toml`:
```toml
[registries.staging]
index = "sparse+https://index.staging.crates.io/"
```

Parallel publishing works by grouping crates into dependency levels. Crates within the
same level have no interdependencies and are published simultaneously (up to `-j N` at a
time). Between levels, a 30 second wait allows the crates.io index to update before
dependent crates are published.

`--no-verify` can be used with parallel publishing to avoid concurrent cargo build
conflicts in the shared target directory. Note that this skips build verification,
which may result in publishing broken crates. Consider running `cargo publish --dry-run`
or `cargo package` beforehand to catch issues.

The process is resumable: if it fails partway through, re-running will skip
already-published crates automatically.

#### Post release

After the initial plan has been generated and release pushed out, the plan file can then be
patch bumped and applied as needed for backports and maintenance.

```
git cherry-pick BACKPORT
parity-publish plan --patch foo bar
parity-publish apply --publish
```

#### Example

Sequential:
```
morganamilo@songbird % parity-publish apply --publish

rewriting manifests...
Publishing 349 packages (0 skipped)
(1/349) publishing binary-merkle-tree-12.0.0... (3s)
(2/349) publishing sp-std-14.0.0... (4s)
```

Parallel:
```
morganamilo@songbird % parity-publish apply --publish -j 8 --no-verify

rewriting manifests...
Publishing 349 crates in 45 levels (0 skipped, max 8 parallel)

--- Level 1/45 (12 crates) ---
(  1/349) published binary-merkle-tree-12.0.0
(  2/349) published sp-std-14.0.0
...
    level completed in 5s
Waiting 30s for index update... done

--- Level 2/45 (18 crates) ---
(  13/349) published sp-core-28.0.0
...
```

### Restore

The restore command reverts Cargo.toml changes made by `apply` (dependency rewrites,
formatting changes, `default-features` additions) while keeping only the version bumps.

This is useful after publishing: the `apply` command rewrites workspace dependencies
to versioned registry dependencies, which is needed for `cargo publish` but shouldn't
be committed to the repository long-term. The `restore` command brings back the clean
`{ workspace = true }` style manifests with only the `version` field updated.

```
# After publishing, restore clean manifests
parity-publish restore

# Restore from a specific git ref (default: HEAD~1)
parity-publish restore --from HEAD~2

# Preview what would be done
parity-publish restore --dry-run
```

How it works:

1. Reads Plan.toml for crates with version bumps
2. Restores all Cargo.toml and Cargo.lock from the given git ref (default `HEAD~1`)
3. Re-applies only the `version` field in each bumped crate's Cargo.toml (format-preserving)
4. Runs `cargo update --workspace` to sync the lockfile

#### Example

```
morganamilo@songbird % parity-publish restore --dry-run

Restoring clean manifests (417 crates bumped)

Dry run — would restore from HEAD~1
  binary-merkle-tree -> 16.1.1
  sp-core -> 40.0.0
  frame-support -> 46.0.0
  polkadot-sdk -> 2603.0.0
  ...
Done
```

## Limitations / Future plans

### Changes

Currently the diffing code to calculate changes between commits is not yet perfect but is being
worked on. The idea is to more semantically diff Manifest files to see if what changed is breaking
or not and if the change warrants a release.

### Semver

Currently parity-publish bumps the major version on every release (apart from --patch bumping).
The plan is to eventually make this tool semver aware via https://github.com/paritytech/prdoc.
This would not only be a lot less ugly and annoying for users, but will allow us to skip pushing
out new releases for dependency changes.
