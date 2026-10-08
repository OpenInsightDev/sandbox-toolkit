# dotagents

Parse the resources of an [Agent Plugins](https://agent-plugins.org/) plugin
(the `.agents/` directory) into typed models: `mcp` for `mcp.json`, `skill` for
`SKILL.md`, and `plugin`, which composes the two. Each has a pure `parse` and an
async `load` over `tokio::fs`.

```rust
let plugin = dotagents::plugin::load(".agents").await?;
```

`DotAgents` watches a whole `.agents/` directory — its own skills and `mcp.json`
plus every plugin under `plugins/` — and hands out its resources, or why it
holds none, alongside the changes as `watch::Event`s:

```rust
let agents = dotagents::DotAgents::open(".agents").await?;
let resources = agents.state()?;
let mut events = agents.subscribe();
```

`watch::Event::{Plugin, Skill, Mcp}` name the resource that moved, carrying its
new content; a directory that stops holding resources at all is one
`watch::Event::Invalid` carrying the reason.

## Acknowledgements

Ported from [**agent-plugin-rs**](https://github.com/Toasterson/agent-plugin-rs)
by [@Toasterson](https://github.com/Toasterson), the reference implementation of
the Agent Plugins v1.0.0 specification. Upstream is under the Mozilla Public
License 2.0; keep derived files under a compatible license.
