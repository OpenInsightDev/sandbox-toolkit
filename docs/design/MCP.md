# MCP 设计

把每个 MCP server 作为可独立注册和管理的资源，统一以 Streamable HTTP 暴露。

## 定位

- MCP server 资源 = 一条注册的 MCP server 配置；
- 对外统一为 Streamable HTTP，端点形态与 [Process.md](./Process.md) 中 `/mcp` 一致；
- 按配置声明支持两种传输：本地 stdio 子进程、远程 Streamable HTTP；
- 资源挂在工作区路由下或直接挂载，工作区只在路由前缀中表达，见 [Workspace.md](./Workspace.md)；
- 错误响应沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封。

MCP 与 skill 是彼此独立的资源，MCP 注册和管理，skill 从固定目录发现；skill 见 [Skill.md](./Skill.md)。

## 资源标识

注册时由调用方指定唯一 `id`，字符集限定为 `[a-z0-9-]`，不允许首尾 `-` 和连续 `-`；`id` 要作为 URL 路径段，并在同一挂载点内唯一。

## 注册

```
POST /workspaces/{id}/mcps
POST /mcps
```

`server` 是按 `type` 判别的封闭联合：字段属于哪种传输就只能是哪种。工作区模式与直接模式都接受两种传输。

| `type`            | 必填      | 可选                 |
| ----------------- | --------- | -------------------- |
| `stdio`           | `command` | `args`、`env`、`cwd` |
| `streamable-http` | `url`     | `headers`            |

```json
{
  "id": "validator",
  "server": {
    "type": "stdio",
    "command": "./bin/validator",
    "args": ["--data", "data/validator"],
    "env": { "CONFIG": "config.json" },
    "cwd": "."
  }
}
```

### stdio

- `command` 是单个可执行 token：bare 名按平台搜索规则解析，`./` 开头的路径相对 `cwd` 解析，参数逐项传递，不经过 shell；
- `cwd` 是工作目录：工作区模式下为工作区相对路径，缺省取工作区根目录；直接模式下如果传入，必须是规范化的绝对路径；直接模式下缺省时为该 stdio 会话新建临时目录，并以该目录作为子进程工作目录；
- 工作区模式下 `command` 与 `cwd` 解析后必须留在工作区内，复用 [FileSystem.md](./FileSystem.md) 的路径规范化与边界校验，子进程的文件访问也经该工作区受限；直接模式下传入的 `cwd` 不受工作区边界约束，但必须在启动前完成绝对路径规范化与目录校验；
- 工作区模式下按 [Workspace.md](./Workspace.md) 的环境变量把工作区根目录注入子进程环境；
- `args`、`env` 都是字面值。

### streamable-http

- `url` 必须是绝对 `http`/`https` URL，不得含 userinfo 或 fragment；非 loopback 必须 HTTPS；
- `headers` 是字面、可见的配置数据，只承载非敏感内容。

### 校验与失败

- 配置不满足上述约束返回 `422 invalid_mcp`；
- `type` 为 `sse` 等不支持的传输返回 `422 unsupported_transport`；
- `id` 在同一挂载点已存在返回 `409`；
- 成功返回 `201` 与资源描述符。

注册是控制面操作，不引入 ETag 条件。

## 查询与管理

```
GET    /workspaces/{id}/mcps
GET    /workspaces/{id}/mcps/{mcp_id}
DELETE /workspaces/{id}/mcps/{mcp_id}
GET    /mcps
GET    /mcps/{mcp_id}
DELETE /mcps/{mcp_id}
```

- `GET .../mcps` 返回该挂载点下所有已注册 MCP 的清单；
- `GET .../mcps/{mcp_id}` 返回单个描述符，含 server 配置与对外的 `uri`；
- `DELETE .../mcps/{mcp_id}` 注销资源，终止其子进程与活动会话；注销不存在的 id 返回 `404`。

```json
{
  "id": "validator",
  "server": { "type": "stdio", "command": "./bin/validator" },
  "uri": "/workspaces/docs/mcps/validator/mcp"
}
```

## 导出

```
GET /mcps?format=mcp-json
GET /workspaces/{id}/mcps?format=mcp-json
```

`format=mcp-json` 把该挂载点下可用的 MCP 导出为 [Agent Plugins mcp.json](https://agent-plugins.org/plugin-authors/mcp-servers) 文档，使 Agent Plugins 客户端可直接消费；未指定 `format` 时按“查询与管理”返回清单。

- 文档严格遵循 mcp.json 的封闭格式：顶层只有 `$schema` 与 `mcpServers`，`$schema` 固定为 `https://agent-plugins.org/schemas/1.0.0/mcp.schema.json`；
- `mcpServers` 的键为 MCP `id`，覆盖该挂载点下全部已注册 MCP，无论注册时声明的是 `stdio` 还是 `streamable-http`；
- 每个值都是 `streamable-http` 条目，`url` 指向该注册的规范端点，见“MCP 端点”：注册为 `stdio` 的条目经 stdio 代理、注册为 `streamable-http` 的条目经远程代理，均对外统一为 Streamable HTTP，因此导出文档无需、也不包含注册时声明的 `server` 配置；
- `url` 由请求推导的外部基址（`scheme` + `Host`）拼成绝对地址；
- 直接模式导出全局 MCP，工作区模式导出该工作区条目并合并全局 MCP，同名时工作区条目覆盖全局条目；
- 工作区条目指向 `/workspaces/{id}/mcps/{mcp_id}/mcp`，被合并的全局条目仍指向其全局端点 `/mcps/{mcp_id}/mcp`。

工作区导出示例，`validator` 注册为 `stdio`、经 stdio 代理呈现，`deployment-api` 则来自合并的全局 MCP：

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "validator": {
      "type": "streamable-http",
      "url": "https://sandbox.example.com/workspaces/docs/mcps/validator/mcp"
    },
    "deployment-api": {
      "type": "streamable-http",
      "url": "https://sandbox.example.com/mcps/deployment-api/mcp"
    }
  }
}
```

## 工作区耦合

工作区仍挂有 MCP 资源时不能注销，返回 `409 workspace_in_use`，须先注销这些资源；注销 MCP 只终止自己启动的子进程，不删除、不修改工作区本身。该约束由工作区 API 统一执行。

## MCP 端点

```
/workspaces/{id}/mcps/{mcp_id}/mcp
/mcps/{mcp_id}/mcp
```

- 传输为 Streamable HTTP，占用 `POST`（JSON-RPC 调用）、`GET`（服务端到客户端的流）和 `DELETE`（结束会话）；
- 端点屏蔽配置里声明的传输差异，客户端始终用 Streamable HTTP 访问；
- 单独使用 `/mcp` 子路径，避免与资源管理端点在 `GET`、`DELETE` 上的冲突。

### stdio 代理

声明为 `stdio` 的条目由服务端启动子进程，并桥接为可远程访问的 Streamable HTTP 端点：

- 服务端既作为 MCP client 与子进程完成握手，又作为 MCP server 面向远程客户端；
- 子进程按 `command`、`args`、`env`、`cwd` 逐项启动，不经过 shell；工作区模式下工作目录为 `cwd`（默认工作区根目录）并受该工作区边界约束；
- stdio 是单连接有状态传输，每个下游 Streamable HTTP 会话对应一条独立的上游连接：会话建立时按需启动专属子进程，会话结束（`DELETE`、断连或空闲超时）时终止并回收；直接模式下未配置 `cwd` 时，临时目录随该会话创建，并在会话结束时一并清理；
- 会话与空闲超时同 [Process.md](./Process.md) 中 pty session 的回收动机。

### 远程代理

声明为 `streamable-http` 的条目由服务端反向代理到配置的 `url`：

- 转发 JSON-RPC 与流；
- 配置的 `headers` 只在访问该 origin 时附带，不得随重定向或 SSE endpoint 事件转发到其它 origin；

## 错误协议

沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封：状态码表达通用语义，`error.code` 提供稳定的机器可读分类。

| 状态  | `error.code`            | 场景                      |
| ----- | ----------------------- | ------------------------- |
| `400` | `bad_request`           | `id`、`format` 等参数非法 |
| `404` | `not_found`             | MCP 或工作区不存在        |
| `405` | `method_not_allowed`    | 端点不支持该方法          |
| `409` | `conflict`              | `id` 在同一挂载点已存在   |
| `409` | `workspace_in_use`      | 工作区仍挂有 MCP 而注销   |
| `422` | `invalid_mcp`           | server 配置校验失败       |
| `422` | `unsupported_transport` | 不支持的传输类型          |
