# Parity Publish - Codebase Analysis

## Overview

**Parity Publish** is a specialized CLI tool developed by Parity Technologies for managing and publishing large Cargo workspaces to crates.io. It is specifically designed to handle the complexities of publishing the polkadot-sdk monorepo, which contains hundreds of interdependent crates.

- **Version**: 0.10.13
- **License**: Apache-2.0
- **Repository**: https://github.com/paritytech/parity-publish

## Purpose and Problem Domain

Publishing large Rust monorepos to crates.io presents unique challenges:

1. **Dependency ordering**: Crates must be published in topological order (dependencies before dependents)
2. **Version management**: Coordinating versions across hundreds of crates
3. **Rate limiting**: crates.io enforces rate limits (~5 minutes per crate, leading to 29+ hours for 350 crates)
4. **Crate squatting prevention**: Claiming crate names before others take them
5. **Git → Registry dependency conversion**: Converting workspace path/git dependencies to registry versions
6. **Semver compliance**: Ensuring version bumps match actual API changes
7. **PR Doc validation**: Integrating with Parity's PR documentation system for change tracking

## Architecture

### Entry Point (`main.rs`)

```
main() → CLI parsing → Command dispatch
```

The application uses `clap` for CLI parsing and `tokio` for async operations. Commands are dispatched to their respective handlers.

### Core Modules

| Module | Purpose | Lines |
|--------|---------|-------|
| `cli.rs` | Command-line argument definitions | ~310 |
| `plan.rs` | Release planning and version calculations | ~732 |
| `apply.rs` | Manifest rewriting, sequential/parallel publishing | ~558 |
| `edit.rs` | Cargo.toml manipulation | ~551 |
| `restore.rs` | Post-publish manifest restoration | ~125 |
| `check.rs` | Pre-publish validation | ~317 |
| `claim.rs` | Crate name claiming on crates.io | ~144 |
| `changed.rs` | Git-based change detection | ~317 |
| `prdoc.rs` | PR Doc integration | ~578 |
| `public_api.rs` | Semver checking via public API diffing | ~394 |
| `status.rs` | Workspace/crates.io status reporting | ~114 |
| `config.rs` | Plan.config file handling | ~94 |
| `registry.rs` | crates.io registry operations | ~56 |
| `workspace.rs` | Cargo workspace queries | ~111 |
| `shared.rs` | Shared utilities | ~82 |

## Key Data Structures

### Plan.toml Structure (`plan.rs`)

```rust
pub struct Planner {
    pub options: Options,           // Global options (e.g., description suffix)
    pub crates: Vec<Publish>,       // Ordered list of crates to publish
    pub remove_crates: Vec<RemoveCrate>,  // Crates to remove from workspace
}

pub struct Publish {
    pub name: String,               // Crate name
    pub from: String,               // Current/old version
    pub to: String,                 // Target version
    pub bump: BumpKind,             // none/patch/minor/major
    pub reason: Option<PublishReason>,  // Why this crate is being published
    pub publish: bool,              // Whether to actually publish
    pub verify: bool,               // Whether to run cargo verify
    pub rewrite_dep: Vec<RewriteDep>,   // Dependencies to rewrite
    pub remove_dep: Vec<RemoveDep>,     // Dependencies to remove
    pub remove_feature: Vec<RemoveFeature>, // Features to remove
}

pub enum BumpKind { None, Patch, Minor, Major }
```

### Change Detection (`changed.rs`)

```rust
pub struct Change {
    pub name: String,
    pub path: PathBuf,
    pub kind: ChangeKind,  // Files, Manifest, or Dependency
    pub bump: BumpKind,
}
```

## Command Reference

### 1. `check` - Pre-publish Validation

**Location**: `src/check.rs`

Validates workspace crates for publishing readiness:

- **Missing description** (non-fatal)
- **Missing license** (fatal)
- **Missing repository** (non-fatal)
- **Broken README paths** (fatal)
- **Unpublished on crates.io** (configurable)
- **Owned by someone else** (fatal)
- **publish=false but depended on by publishable crates** (fatal)
- **Version 0.0.0** (fatal - must be at least 0.1.0)
- **Prerelease versions** (fatal)

```bash
parity-publish check [--allow-nonfatal] [--no-check-owner] [--recursive]
```

### 2. `claim` - Crate Name Reservation

**Location**: `src/claim.rs`

Publishes empty v0.0.0 placeholder crates to reserve names on crates.io:

```bash
# Requires PARITY_PUBLISH_CRATESIO_TOKEN env var
parity-publish claim [--dry-run]
```

Creates minimal crates with:
- Empty `lib.rs`
- Empty `LICENSE`
- Description: "Reserved by Parity while we work on an official release"

Handles crates.io rate limiting (10 min + 5 sec delay between publishes).

### 3. `status` - Workspace Overview

**Location**: `src/status.rs`

Displays crate status comparison between local workspace and crates.io:

```bash
parity-publish status [-m/--missing] [-e/--external] [-v/--version] [-q/--quiet]
```

Output columns: Crate name, Local version, crates.io version, Owner (Parity/External)

### 4. `changed` - Change Detection

**Location**: `src/changed.rs`

Detects which crates changed between git commits:

```bash
parity-publish changed <FROM> [TO=HEAD] [--no-deps] [--manifests]
```

Change types detected:
- **Files**: Source code changes
- **Manifest**: Cargo.toml changes (intelligently filters out version/description/license changes)
- **Dependency**: Transitive changes from dependencies

Uses `git diff --name-only` and maps files to their owning crates.

### 5. `plan` - Release Planning

**Location**: `src/plan.rs`

Generates a `Plan.toml` file specifying the release:

```bash
# New plan for all crates
parity-publish plan --new --all

# New plan for changed crates since a git ref
parity-publish plan --new --since=v1.2.3

# New plan from PR docs
parity-publish plan --new --prdoc=/path/to/prdocs

# Patch bump specific crates in existing plan
parity-publish plan --patch crate1 crate2

# Options
  --pre=dev.1          # Add prerelease suffix
  --description="..."  # Add description suffix to READMEs
  --hold-version       # Don't bump versions
  --skip-check         # Skip check during planning
```

**Key algorithms**:

1. **Topological ordering** (`order()` function):
   - Builds dependency graph
   - Iteratively removes crates with no remaining workspace dependencies
   - Ensures publish order respects dependencies

2. **Version calculation** (`get_version()`, `apply_bump()`):
   - Fetches current crates.io versions
   - Calculates next version based on bump kind
   - For Major: bumps major (or minor if major=0)
   - For Minor: bumps minor (or patch if major=0)
   - For Patch: bumps patch
   - Ensures version doesn't conflict with existing crates.io versions

3. **Plan expansion** (`expand_plan()`):
   - Adds `rewrite_dep` entries for all workspace dependencies
   - Adds `remove_dep` entries for unpublished git dependencies
   - Handles git dependencies by looking up crates.io versions

### 6. `apply` - Execute Release

**Location**: `src/apply.rs`

Applies the plan and optionally publishes:

```bash
# Just rewrite manifests
parity-publish apply

# Rewrite and publish (sequential)
parity-publish apply --publish [--dry-run] [--allow-dirty] [--no-verify]

# Parallel publishing
parity-publish apply --publish -j 8 [--no-verify]

# Publish to staging registry
parity-publish apply --publish --staging

# Use registry versions instead of paths
parity-publish apply --registry

# List crates that need publishing
parity-publish apply --print
```

**Environment setup** (done before `GlobalContext::default()`):
- Sets `CARGO` env var to real cargo binary (prevents build scripts from running `parity-publish metadata`)
- Sets `RUSTUP_TOOLCHAIN` to active toolchain (prevents old toolchain installations during verification)

**Transformations applied**:

1. **Version updates**: Sets each crate to its target version
2. **Rust-version removal**: Strips `package.rust-version` to prevent old toolchain installations
3. **Dependency rewriting**:
   - Path dependencies → version + path (for local dev)
   - Or path dependencies → version only (with `--registry`)
   - Git dependencies → registry versions
   - Dev deps: `workspace = true` → explicit path with `default-features` preserved from workspace definition
4. **Feature cleanup**: Removes features that reference dev-only dependencies
5. **README updates**: Adds release description section if configured

**Sequential publishing** (default, `-j 1`):
- Uses `cargo::ops::publish()` library API
- 15 second delay between publishes
- Skips already-published versions

**Parallel publishing** (`-j N`):
- Uses `cargo publish` CLI subprocesses (not library API)
- Groups crates into dependency levels via `compute_publish_levels()` (topological sort)
- Publishes up to N crates simultaneously within each level
- 30 second wait between levels for crates.io index propagation
- Handles "already published" gracefully

### 7. `config` - Configuration Management

**Location**: `src/config.rs`

Applies changes from `Plan.config`:

```bash
parity-publish config --apply
```

`Plan.config` format:
```toml
[[remove_crate]]
name = "crate-to-remove"

[[crate]]
name = "some-crate"
[[crate.remove_feature]]
feature = "feature-name"
[[crate.remove_dep]]
name = "dep-name"
```

### 8. `prdoc` - PR Doc Integration

**Location**: `src/prdoc.rs`

Parses Parity's PR documentation format:

```bash
parity-publish prdoc /path/to/prdocs [--validate] [--since=ref] [--major]
```

PR Doc format (YAML):
```yaml
crates:
  - name: crate-name
    bump: major|minor|patch|none
```

**Validation mode** (`--validate`):
- Compares declared bumps against detected changes
- Checks semver compliance
- Reports missing PR docs for changed crates
- Validates against `--max-bump` if specified

### 9. `semver` - Semver Checking

**Location**: `src/public_api.rs`

Analyzes public API changes:

```bash
parity-publish semver [crates...] [--since=ref] [--major] [-v/--verbose]
```

Uses:
- `cargo-semver-checks` for semver violation detection
- `public-api` crate for API diffing
- `rustdoc-json` for generating API information

Compares either:
- Against last crates.io release (default)
- Against a specific git commit (`--since`)

### 10. `restore` - Post-publish Manifest Restoration

**Location**: `src/restore.rs`

Restores clean Cargo.toml files after publishing, reverting dependency rewrites and formatting changes while keeping only version bumps:

```bash
# Restore from the commit before apply (default: HEAD~1)
parity-publish restore

# Restore from a specific git ref
parity-publish restore --from HEAD~2

# Preview changes
parity-publish restore --dry-run
```

**How it works**:

1. Reads Plan.toml for crates with `bump != none`
2. Runs `git checkout <ref> -- **/Cargo.toml Cargo.toml Cargo.lock` to restore clean manifests
3. Re-opens workspace and uses `edit::set_version()` (format-preserving) to bump only version fields
4. Runs `cargo update --workspace` to sync the lockfile

This reverts: dep rewrites (workspace→path/registry), formatting changes, `default-features` additions, version fields added to workspace deps in root Cargo.toml.

### 11. `workspace` - Workspace Queries

**Location**: `src/workspace.rs`

Query workspace information:

```bash
# List crate paths
parity-publish workspace crate1 crate2 [-p/--paths]

# Find which crate owns a file
parity-publish workspace --owns path/to/file.rs
```

## Dependencies

### Core Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `cargo` | 0.94.0 | Workspace parsing, publishing, dependency resolution |
| `clap` | 4.5.57 | CLI argument parsing |
| `tokio` | 1.49.0 | Async runtime (with `process` and `time` features for parallel publishing) |
| `crates_io_api` | 0.12.0 | crates.io API client |
| `semver` | 1.0.27 | Version parsing and comparison |
| `toml` / `toml_edit` | 0.8.19 / 0.23.10 | TOML parsing and editing |
| `cargo-semver-checks` | 0.46.0 | Semver violation detection |
| `public-api` | 0.50.3 | Public API extraction |
| `rustdoc-json` | 0.9.8 | Rustdoc JSON generation |
| `termcolor` | 1.4.1 | Colored terminal output |

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `PARITY_PUBLISH_CRATESIO_TOKEN` | API token for publishing to crates.io |
| `PARITY_PUBLISH_STAGING_CRATESIO_TOKEN` | API token for staging.crates.io (with `--staging`) |
| `PARITY_CRATE_OWNER_ID` | Override default Parity owner ID (150167) |

## File Artifacts

| File | Purpose |
|------|---------|
| `Plan.toml` | Release plan specifying all crates, versions, and transformations |
| `Plan.config` | Persistent configuration for features/deps to remove |

## Typical Workflow

### Initial Release

```bash
# 1. Check workspace health
parity-publish check

# 2. Claim any unclaimed crate names
parity-publish claim

# 3. Generate release plan
parity-publish plan --new --since=v1.0.0

# 4. Review and edit Plan.toml manually if needed

# 5. Apply manifest changes
parity-publish apply

# 6. Review changes, commit

# 7. Publish to crates.io (parallel with --no-verify for speed)
parity-publish apply --publish -j 8 --no-verify

# 8. Restore clean manifests (reverts dep rewrites, keeps version bumps)
parity-publish restore

# 9. Commit and push clean state
```

### Patch Release

```bash
# 1. Cherry-pick fix
git cherry-pick <commit>

# 2. Bump affected crates
parity-publish plan --patch crate1 crate2

# 3. Apply and publish
parity-publish apply --publish

# 4. Restore clean manifests
parity-publish restore
```

## Key Implementation Details

### Topological Sort (`plan.rs:order()`)

The ordering algorithm:
1. Maps each crate to its non-dev dependencies
2. Iteratively:
   - Find crates with no remaining dependencies
   - Add them to the order
   - Remove them from other crates' dependency lists
3. Result: crates ordered so dependencies come before dependents

### Manifest Editing (`edit.rs`)

Uses `toml_edit` for format-preserving TOML editing. Key operations:
- `rewrite_deps()`: Converts workspace/path/git deps to versioned deps
- `remove_dep()`: Removes dependencies and cascades feature removal
- `remove_feature()`: Removes feature flags and cleans up references
- `remove_crate()`: Removes crate from workspace members

### Change Detection (`changed.rs`)

1. Gets changed files via `git diff --name-only`
2. For each workspace member:
   - Gets list of files belonging to the crate
   - Checks if any changed files match
   - If only Cargo.toml changed, does semantic diff (ignoring version/description/license)
3. Propagates changes to dependents if `--no-deps` not specified

### Rate Limit Handling

- `claim.rs`: 10 minute + 5 second delay after rate limit hit
- `apply.rs` (sequential): 60 second delay between each publish
- `apply.rs` (parallel): up to N concurrent publishes per level, 30 second wait between levels

## Limitations and Future Plans

1. **Semver bumping**: Currently defaults to major bumps; planned to integrate with prdoc for smarter versioning
2. **Change detection**: Manifest diffing could be more semantic
3. **Publishing time**: Sequential is ~5 min per crate × 350 crates = 29+ hours; parallel (`-j 8`) significantly reduces this

## Code Quality Notes

- Well-structured with clear separation of concerns
- Uses async for crates.io API calls
- Comprehensive error handling with `anyhow`
- Colored terminal output for better UX
- Supports both quiet and verbose modes

## Testing Considerations

When working with this codebase:
- Use `--dry-run` flags to test without side effects
- Test against small workspaces first
- Be aware of crates.io rate limits during integration testing
- The semver checking requires a nightly Rust toolchain
