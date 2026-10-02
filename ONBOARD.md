# ONBOARD

- [ ] P1：把 sdk 中所有 layerForWorkspace 取消掉，改为通过 Workspace 向外提供 workspace scope 的各类服务，从而保证生命周期 integrity
- [ ] P1：进行派生工作区（skill、plugin 命名空间）工作机制的完整设计
  - 现状 id：注册工作区 id 限定 `[a-z0-9-]`；派生 id 额外允许 `.`（[Plugin.md](./docs/design/Plugin.md) 声明为唯一扩展），本次 skill 已改为命名空间段统一用 `.` 连接。
  - 现状拼法有两套：skill 为 `skill.[workspace_id].[plugin_id]?.[skill_id]`，全局段是 `global`；plugin 命名空间为 `plugin-[workspace_id-]{plugin_id}-{namespace}`，scope 与 plugin/namespace 之间用 `-`，点只来自反向域名。
  - 现状解析：派生工作区不注册，一律 `Workspace::managed`（只读、不可直接删除）；`AppState::resolve_workspace` 先查注册表，再 skill、再 plugin，按需重扫发现目录把 id 解码回 scope 与资源目录。
  - 现状生命周期：随 `.agents` 发现目录变化自动增减，不订阅 `workspace_registered`/`workspace_removed`，也不进注册表的目录 watch。
  - 现状客户端：`validateWorkspaceId`（create）保持严格 `[a-z0-9-]`，`validateWorkspaceRef`（get/remove）允许 `.`。
  - 待定：`global` 段是否保留、注册名为 `global` 时的冲突；两套分隔符是否统一；派生 id 目前不能作为 skill/plugin/mcp 挂载前缀（这些端点先做严格 `validate_id`），与 `agents_root` 注释「scope 可命名派生工作区」矛盾；`Workspace::env()` 未覆盖 `.`；按需重扫的代价与缓存策略。
- [ ] P1：把 tus crate 对接到 fs 上
  - 阻塞：tus 的 `Config.base_path` 是单个静态挂载前缀，同时用于注册路由、从 `request.uri()` 反推 upload id、生成 `Location`/`Upload-Concat` URL，一份 handler 无法同时服务 `/upload` 与动态的 `/workspaces/{id}/upload`。根因是 `Handler` 声称「independent of routing」（`handler.rs:40`）却自己解析路径；应让路由层解析 `{upload_id}`、把 `base` 显式传给 handler。
  - 规避：取消 workspace 路由，只保留静态 `/upload`，tus crate 零改动，但偏离 [FileSystem.md](./docs/design/FileSystem.md) 的两种寻址模式、丢掉工作区边界/只读校验，并使 `TUSClient.layerForWorkspace` 失效；且工作区身份最终仍需由 `Upload-Metadata` 承载（即 FileSystem.md 的「待定」项）。
- [ ] P2：探索 h2 only 的可行性
- [ ] P2：支持 Programmable Tool Calling / Code Mode
- [ ] P2：支持 Tool Search
- [ ] P2：把 /skill 端点提供的能力与 [ext-skill](https://github.com/modelcontextprotocol/ext-skills) 扩展规范对齐
