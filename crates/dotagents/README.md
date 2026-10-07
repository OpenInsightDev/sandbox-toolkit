# dotagents

Parse the resources of an [Agent Plugins](https://agent-plugins.org/) package
(the `.agents/` directory) into typed models: `mcp` for `mcp.json`, `skill` for
`SKILL.md`, and `plugin`, which composes the two. Each has a pure `parse` and an
async `load` over `tokio::fs`.

```rust
let plugin = dotagents::plugin::load(".agents").await?;
```

## Acknowledgements

Ported from [**agent-plugin-rs**](https://github.com/Toasterson/agent-plugin-rs)
by [@Toasterson](https://github.com/Toasterson), the reference implementation of
the Agent Plugins v1.0.0 specification. Upstream is under the Mozilla Public
License 2.0; keep derived files under a compatible license.
