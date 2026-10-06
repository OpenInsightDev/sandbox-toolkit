# MCP 设计

## 概览

从 `.agents/mcp.json` 与所在工作区持有的 plugin 的 `mcp.json` 自动加载外部 MCP server，统一以 Streamable HTTP 暴露。按条目的 `type` 支持两种 transport：`stdio`、`streamable-http`，并提供查询端点 `GET /mcps`。

MCP 是 Workspace 持有的资源之一，其构造与丢弃由 [Workspace 设计](./Workspace.md) 负责；plugin 一侧的构成见 [Plugin 设计](./Plugin.md)。

## 加载

监听 `.agents/mcp.json` 与 plugin `mcp.json` 的变动并重载；`.agents/mcp.json` 缺失时条目为空，不算加载失败；重载失败时丢弃上一次成功加载的条目，挂载回答 `404`，直到再次解析成功。

条目有两个来源：该 scope 自身的 `.agents/mcp.json`，以及该工作区持有的 plugin 的 `mcp.json`。plugin 一侧的加载失败由资源集合整体承担，不在这里单独处理。

### 测试

- `load::discovers`：`.agents/mcp.json` 中的每个条目都出现在 `GET /mcps` 中。
- `load::plugin`：工作区持有的 plugin 的 `mcp.json` 条目也出现在 `GET /mcps` 中。
- `load::reloads`：改写 `.agents/mcp.json` 后，`GET /mcps` 随之反映新的条目集合。
- `load::broken`：运行期把 `.agents/mcp.json` 改坏后 `GET /mcps` 与 `/mcps/{mcp_id}` 返回 `404`，`GET /skills` 仍为 `200`；改回有效后恢复 `200`。

## 合并

工作区对外呈现的 MCP 集合是 global 与工作区自身的并集，按 id 合并，同名时工作区条目胜出；`/mcps`（无前缀）解析到 global。plugin 提供的条目 id 带 `{plugin_id}.` 前缀，因此只可能在工作区与 global 持有同名 plugin 时相撞。

工作区自身资源缺失不影响合并结果，仅当 global 与工作区都无资源时，工作区挂载回答 `404`；被合并的任一 scope 处于重载失败状态时，该挂载回答 `404`。

### 测试

- `merge::includes_global`：global 与工作区各自的条目都出现在 `/workspaces/{id}/mcps` 文档中。
- `merge::workspace_wins`：同名 id 经 `/workspaces/{id}/mcps/{mcp_id}` 接入到工作区自身的上游。
- `merge::plugin_prefix`：plugin 提供的条目以 `{plugin_id}.` 为前缀出现。
- `merge::absent_workspace`：工作区自身无 `.agents` 而 global 有资源时，`/workspaces/{id}/mcps` 返回 global 的条目而非 `404`。
- `merge::broken`：工作区自身的 `mcp.json` 运行期损坏后，`/workspaces/{id}/mcps` 返回 `404`，global 的条目不再呈现。

## 代理

按条目的 `type` 运行对应的 transport：

- stdio 条目经 stdio 代理；
  - plugin 的 stdio 条目注入保留变量 `PLUGIN_ROOT`、`PLUGIN_DATA`，取值见 [Plugin 设计](./Plugin.md)；`.agents/mcp.json` 自身的条目不注入；
- streamable-http 条目经远程代理；

对外统一为 Streamable HTTP，接入路径为 `/mcps/{mcp_id}`；其应答遵循 MCP 规范，本项目只定义进入代理前的边界错误。`{mcp_id}` 按 [合并](#合并) 规则解析。

### 端点

`/mcps/{mcp_id}` 与带 workspace 前缀的 `/workspaces/{workspace_id}/mcps/{mcp_id}`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 404 | 未找到 | `{mcp_id}` 不在合并集合中；或 workspace 挂载下 `{workspace_id}` 不存在；或该挂载合并了 [加载](#加载) 失败的 scope |

### 测试

- `proxy::not_found`：访问不存在的 `mcp_id` 返回 `404`。
- `proxy::remote`：`streamable-http` 条目经代理与上游完成 MCP 握手并转发 `list_tools`。
- `proxy::stdio`：`.agents/mcp.json` 的 stdio 条目拉起子进程时不注入 `PLUGIN_ROOT`、`PLUGIN_DATA`。

## 查询

`GET /mcps` 返回全部条目均为 `streamable-http` 的 mcp.json 文档；工作区挂载返回 [合并](#合并) 后的文档。

### 端点

`GET /mcps` 与 `GET /workspaces/{workspace_id}/mcps`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回 mcp.json 文档 |
| 404 | 未找到 | workspace 挂载下 `{workspace_id}` 不存在；或该挂载合并了 [加载](#加载) 失败的 scope |

### 测试

- `query::manifest`：`GET /mcps` 返回的文档中每个条目都被改写为 `streamable-http`，指向 `/mcps/{mcp_id}`。
- `query::workspace`：`GET /workspaces/global/mcps` 的条目地址带 workspace 前缀。
- `query::unknown_workspace`：访问不存在的 workspace 返回 `404`。

## 监听

`GET /mcps`（工作区挂载为 `GET /workspaces/{workspace_id}/mcps`）带 `Accept: text/event-stream` 时不返回文档，而打开一条 SSE 流，推送该挂载点 scope 的 mcp 条目生命周期事件；不带该头时照常返回 [查询](#查询)。

scope 的 `.agents/mcp.json` 与它全部 plugin 的 `mcp.json` 由同一观察者监听，即 [加载](#加载) 的重载范围，事件合并在同一条流上。

| 事件 | 触发条件 |
| --- | --- |
| `register` | 一个 mcp id 新进入该 scope 的发现集合 |
| `unregister` | 一个 id 离开发现集合 |
| `update` | 一个仍在集合中的 id，其来源 `mcp.json` 发生变动 |

`id` 与 [查询](#查询) 一致：scope 自身取 server 名，plugin 提供者取 `{plugin_id}.{server_name}`。条目没有目录那样的稳定身份，改掉 server 名即旧 id `unregister`、新 id `register`。只有 `mcp.json` 触发 `update`，其他文件（如 `plugin.json`）自身的变动不发事件。

[加载](#加载) 失败使集合视为空，原有 id 逐个 `unregister`；恢复后逐个 `register`。plugin 一侧的失败由资源集合整体承担，同样落到空集合。

流以 `text/event-stream` 承载，每个事件由一行 `event:`（事件名）、一行 `data:`（`{"id": "<mcp_id>"}`）与一个空行结束：

```
event: register
data: {"id":"alpha"}

```

流自订阅时刻起只推后续事件，不回放当前集合；调用方断开即停止监听。

### 测试

- `events::register`：`.agents/mcp.json` 新增一个条目后流上出现 `register`，`id` 为 server 名，且不出现 `update`。
- `events::unregister`：删除一个条目后流上出现 `unregister`。
- `events::update`：改动仍在集合中的条目的配置后流上出现 `update`。
- `events::rename`：改掉 server 名后旧 id `unregister`、新 id `register`，且不出现 `update`。
- `events::broken`：`.agents/mcp.json` 变为非法使集合为空，流上对原有 id 出现 `unregister`，且不出现 `update`。
- `events::plugin`：plugin 的 `mcp.json` 变动后流上对其条目出现 `update`，`id` 带 `{plugin_id}.` 前缀。
- `events::ignores_other_files`：改动 plugin 目录内的其他文件不产生事件。
- `events::workspace`：`GET /workspaces/{id}/mcps` 的流报该 scope 的事件。
- `events::negotiation`：不带 `Accept: text/event-stream` 时该端点返回文档，带该头时以 `text/event-stream` 应答。
