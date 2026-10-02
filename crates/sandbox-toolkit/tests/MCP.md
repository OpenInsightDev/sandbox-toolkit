# MCP 设计

## 概览

从 `.agents/mcp.json` 自动加载外部 MCP server，统一以 Streamable HTTP 暴露。按条目的 `type` 支持两种 transport：`stdio`、`streamable-http`，并提供查询端点 `GET /mcps`。

MCP 是 Workspace 持有的资源之一，其构造与丢弃由 [Workspace 设计](./Workspace.md) 负责。

## 加载

从 `.agents/mcp.json` 自动加载外部 MCP server，并监听 `.agents/mcp.json` 与 plugin `mcp.json` 的变动并重载。

### 测试

- `load::discovers`：`.agents/mcp.json` 中的每个条目都出现在 `GET /mcps` 中。
- `load::reloads`：改写 `.agents/mcp.json` 后，`GET /mcps` 随之反映新的条目集合。

## 代理

按条目的 `type` 运行对应的 transport：

- stdio 条目经 stdio 代理；
  - stdio 子进程提供保留变量 `PLUGIN_ROOT`、`PLUGIN_DATA`；
- streamable-http 条目经远程代理；

对外统一为 Streamable HTTP，接入路径为 `/mcps/{mcp_id}`；其应答遵循 MCP 规范，本项目只定义进入代理前的边界错误。

### 端点

`/mcps/{mcp_id}` 与带 workspace 前缀的 `/workspaces/{workspace_id}/mcps/{mcp_id}`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 404 | 未找到 | `{mcp_id}` 未注册；或 workspace 挂载下 `{workspace_id}` 不存在 |

### 测试

- `proxy::not_found`：访问不存在的 `mcp_id` 返回 `404`。
- `proxy::remote`：`streamable-http` 条目经代理与上游完成 MCP 握手并转发 `list_tools`。
- `proxy::stdio`：`stdio` 条目拉起子进程时注入 `PLUGIN_ROOT`、`PLUGIN_DATA`。

## 查询

`GET /mcps` 返回全部条目均为 `streamable-http` 的 mcp.json 文档。

### 端点

`GET /mcps` 与 `GET /workspaces/{workspace_id}/mcps`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回 mcp.json 文档 |
| 404 | 未找到 | workspace 挂载下 `{workspace_id}` 不存在；`/mcps` 固定解析到 global，因而不会返回 `404` |

### 测试

- `query::manifest`：`GET /mcps` 返回的文档中每个条目都被改写为 `streamable-http`，指向 `/mcps/{mcp_id}`。
- `query::workspace`：`GET /workspaces/global/mcps` 的条目地址带 workspace 前缀。
- `query::unknown_workspace`：访问不存在的 workspace 返回 `404`。
