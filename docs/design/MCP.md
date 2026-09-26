# MCP 设计

从 `.agents/mcp.json` 自动加载外部 MCP server，统一以 Streamable HTTP 暴露。

## 定位

- MCP server = `.agents/mcp.json` 中 `mcpServers` 的一个条目；
- 对外统一为 Streamable HTTP，端点形态与 [Process.md](./Process.md) 中 `/mcp` 一致；
- 按条目声明的 `type` 支持两种传输：本地 stdio 子进程、远程 Streamable HTTP；
- 资源挂在工作区路由下或直接挂载，工作区只在路由前缀中表达，见 [Workspace.md](./Workspace.md)；
- 错误响应沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封。

MCP 与 skill 是彼此独立的资源，都从 `.agents` 目录发现：MCP 读 `.agents/mcp.json` 文件，skill 扫 `.agents/skills` 目录；skill 见 [Skill.md](./Skill.md)。

## 发现

MCP server 从两处固定的 `.agents/mcp.json` 发现，各对应一个挂载点：

| 范围   | 文件                                | 挂载点                            | 加载时机         |
| ------ | ----------------------------------- | --------------------------------- | ---------------- |
| 全局   | `~/.agents/mcp.json`                | `/mcps`                           | 服务启动后立即   |
| 工作区 | `{workspace_root}/.agents/mcp.json` | `/workspaces/{workspace_id}/mcps` | 工作区注册后立即 |

- 文件格式为 [Agent Plugins mcp.json](https://agent-plugins.org/plugin-authors/mcp-servers)：顶层只有 `$schema` 与 `mcpServers`，`$schema` 固定为 `https://agent-plugins.org/schemas/1.0.0/mcp.schema.json`；
- `mcpServers` 的键即 `mcp_id`，值为按 `type` 判别的封闭联合，见“配置”；
- `{workspace_root}` 是工作区根目录；
- 每个挂载点只在上述时机读取一次；文件缺失按空清单处理，之后文件变化不自动生效，须重启服务或重新注册工作区；
- 单个条目不满足“资源标识”或“配置”约束时被跳过，不进入清单，也不影响其它条目。

## 生命周期

- 服务启动后加载全局挂载点的 MCP；注册工作区后加载该工作区挂载点的 MCP；
- 注销工作区时卸载其挂载点，终止名下所有 stdio 子进程与活动会话，不删除、不修改 `.agents/mcp.json` 或工作区本身；
- 加载只建立可用清单与配置，不常驻子进程；stdio 子进程在会话建立时按需启动，见“stdio 代理”。

## 资源标识

- `mcp_id` 即 `mcpServers` 的键；
- 取值限定为 `[a-z0-9-]`，不允许首尾 `-` 和连续 `-`；
- 该字符集排除 `/`、`.` 等分隔符，使 `mcp_id` 可直接作为 URL 路径段；
- `mcp_id` 在同一挂载点内唯一；JSON 对象键天然唯一。

## 配置

`mcpServers` 的值是按 `type` 判别的封闭联合：字段属于哪种传输就只能是哪种。全局与工作区两处都接受两种传输。

| `type`            | 必填      | 可选                 |
| ----------------- | --------- | -------------------- |
| `stdio`           | `command` | `args`、`env`、`cwd` |
| `streamable-http` | `url`     | `headers`            |

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "validator": {
      "type": "stdio",
      "command": "./bin/validator",
      "args": ["--data", "data/validator"],
      "env": { "CONFIG": "config.json" }
    }
  }
}
```

### stdio

- `command` 是单个可执行 token：bare 名按平台搜索规则解析，`./` 开头的路径相对 `${PLUGIN_ROOT}` 解析，参数逐项传递，不经过 shell；
- `cwd` 是工作目录，只接受 `${PLUGIN_ROOT}`、`${PLUGIN_DATA}` 前缀，缺省取 `${PLUGIN_ROOT}`；占位符展开见“插件变量”；
- 工作区挂载点下，`command` 与 `cwd` 解析后必须留在工作区内，复用 [FileSystem.md](./FileSystem.md) 的路径规范化与边界校验，子进程的文件访问也经该工作区受限；全局挂载点下不受工作区边界约束，但必须在启动前完成绝对路径规范化与目录校验；
- 按 [Workspace.md](./Workspace.md) 的环境变量把已注册工作区根目录注入子进程环境。

### streamable-http

- `url` 必须是绝对 `http`/`https` URL，不得含 userinfo 或 fragment；非 loopback 必须 HTTPS；
- `headers` 是字面、可见的配置数据，只承载非敏感内容。

## 插件变量

服务端扮演 Agent Plugins 客户端，按 [Plugin variables](https://agent-plugins.org/plugin-authors/mcp-servers#plugin-variables) 为 stdio 子进程提供两个保留变量：

| 变量          | 取值                                                                        |
| ------------- | --------------------------------------------------------------------------- |
| `PLUGIN_ROOT` | 发现到该条目的目录，全局为 `~/.agents`、工作区为 `{workspace_root}/.agents` |
| `PLUGIN_DATA` | 服务端为该 MCP 分配的专有可写目录，位于 `${PLUGIN_ROOT}/.data/{mcp_id}`     |

- 两个变量都注入子进程环境，并作为 `args`、`env` 值、`cwd` 中同名占位符的展开值；作为保留变量不可被插件覆盖；
- 展开是文本、单次、非递归的，不作用于 `env` 的键、`command`、远程 `url` 与 `headers`；
- `PLUGIN_DATA` 按需创建，随 `.agents` 目录持久化，不随 MCP 卸载删除。

## 查询

```
GET /mcps
GET /mcps/{mcp_id}
GET /workspaces/{id}/mcps
GET /workspaces/{id}/mcps/{mcp_id}
```

- `GET .../mcps` 返回该挂载点下已加载 MCP 的清单；
- `GET .../mcps/{mcp_id}` 返回单个描述符，含 server 配置与对外的 `uri`；
- 未知 `mcp_id` 或不存在的工作区返回 `404`。

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

`format=mcp-json` 把该挂载点下可用的 MCP 导出为 [Agent Plugins mcp.json](https://agent-plugins.org/plugin-authors/mcp-servers) 文档，使 Agent Plugins 客户端可直接消费；未指定 `format` 时按“查询”返回清单。

- 文档严格遵循 mcp.json 的封闭格式：顶层只有 `$schema` 与 `mcpServers`，`$schema` 固定为 `https://agent-plugins.org/schemas/1.0.0/mcp.schema.json`；
- `mcpServers` 的键为 `mcp_id`，覆盖该挂载点下全部已加载 MCP，无论加载时声明的是 `stdio` 还是 `streamable-http`；
- 每个值都是 `streamable-http` 条目，`url` 指向该条目的规范端点，见“MCP 端点”：`stdio` 条目经 stdio 代理、`streamable-http` 条目经远程代理，均对外统一为 Streamable HTTP，因此导出文档无需、也不包含加载时声明的 `server` 配置；
- `url` 由请求推导的外部基址（`scheme` + `Host`）拼成绝对地址；
- 全局挂载点导出全局 MCP，工作区挂载点导出该工作区条目并合并全局 MCP，同名时工作区条目覆盖全局条目；
- 工作区条目指向 `/workspaces/{id}/mcps/{mcp_id}/mcp`，被合并的全局条目仍指向其全局端点 `/mcps/{mcp_id}/mcp`。

工作区导出示例，`validator` 声明为 `stdio`、经 stdio 代理呈现，`deployment-api` 则来自合并的全局 MCP：

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

## MCP 端点

```
/workspaces/{id}/mcps/{mcp_id}/mcp
/mcps/{mcp_id}/mcp
```

- 传输为 Streamable HTTP，占用 `POST`（JSON-RPC 调用）、`GET`（服务端到客户端的流）和 `DELETE`（结束会话）；
- 端点屏蔽配置里声明的传输差异，客户端始终用 Streamable HTTP 访问；
- 单独使用 `/mcp` 子路径，避免与查询端点在 `GET` 上的冲突。

### stdio 代理

声明为 `stdio` 的条目由服务端启动子进程，并桥接为可远程访问的 Streamable HTTP 端点：

- 服务端既作为 MCP client 与子进程完成握手，又作为 MCP server 面向远程客户端；
- 子进程按 `command`、`args`、`env`、`cwd` 逐项启动，不经过 shell；工作区挂载点下工作目录为 `cwd`（默认 `${PLUGIN_ROOT}`）并受该工作区边界约束；
- stdio 是单连接有状态传输，每个下游 Streamable HTTP 会话对应一条独立的上游连接：会话建立时按需启动专属子进程，会话结束（`DELETE`、断连或空闲超时）时终止并回收；
- 会话与空闲超时同 [Process.md](./Process.md) 中 pty session 的回收动机。

### 远程代理

声明为 `streamable-http` 的条目由服务端反向代理到配置的 `url`：

- 转发 JSON-RPC 与流；
- 配置的 `headers` 只在访问该 origin 时附带，不得随重定向或 SSE endpoint 事件转发到其它 origin。

## 错误协议

沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封：状态码表达通用语义，`error.code` 提供稳定的机器可读分类。

| 状态  | `error.code`         | 场景                         |
| ----- | -------------------- | ---------------------------- |
| `400` | `bad_request`        | `mcp_id`、`format` 等参数非法 |
| `404` | `not_found`          | MCP 或工作区不存在           |
| `405` | `method_not_allowed` | 端点不支持该方法             |
