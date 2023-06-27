# Parity Publish

WIP tool to publish and manage publishing cargo workspaces to crates.io


## Previous Work and Rationale

https://github.com/paritytech/subpub
https://github.com/paritytech/cargo-unleash
https://github.com/paritytech/releng-scripts (partly)

This sort of tool has been attempted a few times before at parity with varying ammounts of success. Though it's been decided to start over as the existing tools were projects of people who have left the company and are thus no longer here to maintain it.

So, instead of trying to pick up and modify existing tools, I think it's a better idea to start over *fast*. Get a barebones tool up and running with the features we currently want right now before fleshing out to a more general and polished tool.

Though these tools act as a useful reference and should be referred to when thinking how to impllement something.


## What The Tool Does (And Will Do)

The tool is split up into sub commands. The current plan is to rapidly iterate on subcomamnds that are useful now and eventually build up a full tool and workflow. Building further more complex functionality from the simpler subcommands that we've build.

### Status

```
parity-publish status <workspace>
```

This command displays information about the crates in a workspace. Compares the local version to crates.io and tells us if parity owns the crate.

### Claim

```
parity-publish claim <workspace>
```

This command claims all the crates in a workspace by publishing v0.0.0 stub packages under the parity crate account.

TODO: integrate into github action for automatic claiming.

### Changed

```
parity-publish changed <workspace>
```

This command tells you which crates have been modified since being published to crates.io.

### Plan (WIP)

```
parity-publish plan <workspace>
```

This command plans a version bump and publish, writing the plan to some sort of lock file. The rationale here is that the tool should figure out everything it needs to do ahead of time and output this in a clear way. This allows the release team to fully review a publish before running it and so we can edit the file to make tweaks as needed. As with a set of crates as complicated as this, one of changes and manual intervention sometimes need to be made.

When planning releases, each crate is it's own separate thing and is bumped as needed and adhering to  semver. We may want to group some crates together to have them share version numbers but I don't know if this is useful at the moment.

The general workflow for this will be:

- Calculate which creates have changed so need to be published
    - This code is already written inside the changed command
- Calculate what sort of version bump to perform
    - To do this we need to get the list of commits/PRs that are new to this release
        - The current plan is to tag $crate-v$version for each crate release so we can easy calculate this
    - We then need to find out which crates each PR/commit touch and what kind of changes they make to each crate
        - The plan here is some sort of labeling in each commit/PR
            - This needs to be done by the devs. The release team can't automate this or go chasing up.
            - this issue talks about one way to implement this https://github.com/paritytech/release-engineering/issues/165
            - We will need some sort of CI to enfource these labels otherwise people will forget them
    - Then bump all the versions in Crates.into
    - Then bump crates that had a dependency do a major bump. if they expose that dependency in the public API.
        - We can probably scan the source code for this
    - Then bump the dependency table section of each crate
    - Then decide publish order
    - Publish!


### Apply (WIP)

```
parity-publish apply <workspace>
```

This command applies the plan. See plan for the details.
