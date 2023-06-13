# Parity Publish

WIP tool to publish and manage publishing cargo workspaces to crates.io


## Previous Work and Rationale

https://github.com/paritytech/subpub
https://github.com/paritytech/cargo-unleash
https://github.com/paritytech/releng-scripts (partly)

This sort of tool has been attempted a few times before at parity with varying ammounts of success. Though it's been decided to start over as the existing tools were projects of people who have left the company and are thus no longer here to maintain it.

So, instead of trying to pick up and modify existing tools, I think it's a better idea to start over *fast*. Get a barebones tool up and running with the features we currently want right now before fleshing out to a more general and polished tool.

Though these tools act as a useful reference and should be refered to when thinking how to impllement something.

## Goals

The fist goal here is to have a tool to publish polkadot + substrate + cumulus

For that the first then we need is data on the current crates and what is already published and at what versions.

Then we can use that information to take action and try to at least make sure we have all the crate names owned by parity.

From then on out the gaol is to automatically have crates publish on commit/tags. To do this we need some form of automatic (or semi automatic) verision bumping as well as some github action interaction.


