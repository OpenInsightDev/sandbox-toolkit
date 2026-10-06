# Workspace 设计

## 概览

Workspace 以 `id` 标识，绑定一个规范化的绝对目录 `root`，并带有一个 `access` 属性。用户家目录默认被建模为 id 为 `global` 的 workspace。

## 注册

`POST /workspaces`，请求体 `{id, root, access?}`，其中 `access` 取 `read-write`（默认）或 `read-only`。

`root` 必须是绝对路径并指向已存在的目录，注册前被规范化为规范绝对路径。注册时由服务进程实地探测该目录的读、写权限，缺任一项即失败。

`id` 是非空字符串，且只由 ASCII 字母、数字与 `-`、`_`、`.`、`~` 组成。

### 端点

`POST /workspaces`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 201 | 已创建 | 注册成功，返回该 workspace 对象 |
| 400 | 请求无效 | `id` 为空或含不允许的字符；`root` 非绝对、不存在或不是目录 |
| 403 | 无权限 | 服务进程对 `root` 不具备读或写权限 |
| 409 | 冲突 | `id` 已被占用（含 `global`） |

### 测试

- `register::creates`：注册后 `GET /workspaces/{id}` 返回该 workspace，`root` 为规范化绝对路径，`access` 默认为 `read-write`。
- `register::conflict`：同一 `id` 注册两次，第二次返回 `409`。
- `register::invalid_root`：`root` 非绝对、不存在或为文件时返回 `400`。
- `register::invalid_id`：`id` 为空或含不允许的字符时返回 `400`。
- `register::denied`：服务进程对 `root` 无写权限时返回 `403`。

## 查询

返回全部 workspace 或指定的单个 workspace。

### 端点

`GET /workspaces` 与 `GET /workspaces/{workspace_id}`。

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | `GET /workspaces` 返回 workspace 对象数组；`GET /workspaces/{workspace_id}` 返回单个 workspace 对象 |
| 404 | 未找到 | `{workspace_id}` 不存在 |

### 测试

- `query::list`：`GET /workspaces` 含 `global` 与已注册的 workspace。
- `query::one`：`GET /workspaces/{id}` 返回对应的单个 workspace。
- `query::unknown`：`GET /workspaces/missing` 返回 `404`。

## 更新

`PATCH /workspaces/{workspace_id}`，请求体 `{access}`，改写 `access` 并返回更新后的 workspace 对象。`access` 是声明性属性，更新不触发权限探测。

### 端点

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回更新后的 workspace 对象 |
| 403 | 无权限 | `{workspace_id}` 为 `global` |
| 404 | 未找到 | `{workspace_id}` 不存在 |

### 测试

- `update::access`：`PATCH` 为 `read-only` 后 `GET` 反映新的 `access`。
- `update::unknown`：对不存在的 `workspace_id` 返回 `404`。

## 注销

`DELETE /workspaces/{workspace_id}` 注销指定 workspace。

### 端点

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 204 | 无内容 | 注销成功 |
| 403 | 无权限 | `{workspace_id}` 为 `global` |
| 404 | 未找到 | `{workspace_id}` 不存在 |

### 测试

- `unregister::removes`：注销后 `GET /workspaces/{id}` 返回 `404`。
- `unregister::unknown`：对不存在的 `workspace_id` 返回 `404`。

## global

用户家目录被预置为 id 为 `global` 的 workspace；用户不能更新或注销 `global`。

### 测试

- `global::default`：`GET /workspaces/global` 的 `root` 为用户家目录。
- `global::immutable`：对 `global` 的 `PATCH` 与 `DELETE` 返回 `403`。

## 资源

Workspace 监听 `root/.agents` 目录的存在性，并据此决定是否持有资源集合（见 [MCP 设计](./MCP.md)、[Skill 设计](./Skill.md)、[Plugin 设计](./Plugin.md)）。

`root/.agents` 存在时构造资源集合，不存在时为 `None`；构造是整体的，任一类资源构造失败（如 `.agents` 存在而 `mcp.json` 损坏，或某个 plugin 的 `plugin.json` 被拒）即整体视为 `None`；`mcp.json` 缺失不算构造失败，该资源类只是没有条目；运行期 `.agents` 出现或消失时同样构造或丢弃。

构造时先加载 Plugin，再把它交给 MCP 与 Skill，见 [Plugin 设计](./Plugin.md)。

资源缺失时，global 的挂载回答 `404`：`GET /mcps`、`/mcps/{id}`、`GET /skills`、`/skills/{skill_id}`、`GET /plugins` 与 `/plugins/{plugin_id}` 均如此。工作区挂载的 `404` 判定另见 [MCP 设计](./MCP.md) 与 [Skill 设计](./Skill.md) 的合并规则。

### 测试

- `resources::absent`：无 `.agents` 时注册与启动成功，`GET /mcps`、`/mcps/{id}`、`GET /skills`、`/skills/{skill_id}`、`GET /plugins` 与 `/plugins/{plugin_id}` 返回 `404`。
- `resources::present`：有 `.agents` 时资源集合被构造，`GET /mcps` 返回 `200`；无 `mcp.json` 时同样构造，`GET /mcps` 与 `GET /skills` 都返回 `200`。
- `resources::toggles`：运行期创建 `.agents` 后 `GET /mcps` 变为 `200`，删除后回到 `404`。
- `resources::broken`：`.agents` 存在但 `mcp.json` 损坏时，资源集合整体视为缺失，`GET /mcps` 与 `GET /skills` 返回 `404`。
- `resources::broken_plugin`：`.agents` 存在但某个 plugin 的 `plugin.json` 被拒时，资源集合整体视为缺失，`GET /plugins`、`GET /mcps` 与 `GET /skills` 都返回 `404`。

## 监听

`GET /workspaces` 带 `Accept: text/event-stream` 时不返回数组，而打开一条 SSE 流，推送 workspace 生命周期事件；不带该头时照常返回 [查询](#查询)。`/workspaces` 是唯一挂载，没有工作区前缀变体。

流与 [查询](#查询) 的集合一致：只覆盖注册的 workspace，不覆盖派生工作区（`skill.*`、`plugin.*`）。`global` 预置且不可变，不会有它的事件。

| 事件 | 触发条件 |
| --- | --- |
| `register` | `POST /workspaces` 注册成功，id 进入集合 |
| `unregister` | `DELETE /workspaces/{workspace_id}` 注销成功，id 离开集合 |
| `update` | `PATCH /workspaces/{workspace_id}` 改写 `access` |

流以 `text/event-stream` 承载，每个事件由一行 `event:`（事件名）、一行 `data:`（`{"id": "<workspace_id>"}`）与一个空行结束：

```
event: register
data: {"id":"work"}

```

流自订阅时刻起只推后续事件，不回放当前集合；调用方断开即停止监听。

### 测试

- `events::register`：`POST /workspaces` 成功注册后流上出现 `register`，`id` 为 workspace id。
- `events::unregister`：`DELETE /workspaces/{id}` 注销后流上出现 `unregister`。
- `events::update`：`PATCH /workspaces/{id}` 改写 `access` 后流上出现 `update`。
- `events::negotiation`：不带 `Accept: text/event-stream` 时该端点返回数组，带该头时以 `text/event-stream` 应答。

## 汇总监听

`GET /workspaces/{workspace_id}` 带 `Accept: text/event-stream` 时不返回 workspace 对象，而打开一条 SSE 流，汇总该工作区自身 scope 的 skill、plugin、mcp 事件；不带该头时照常返回 [查询](#查询) 的对象。`global` 同样适用。

每个事件的 `event:` 与对应独立端点同名（`register`、`unregister`、`update`），`data` 多一个 `resource` 字段标明来源：

```
event: register
data: {"resource":"skill","id":"deploy"}

```

| 字段 | 取值 |
| --- | --- |
| `resource` | `skill`、`plugin` 或 `mcp` |
| `id` | 该资源在该 scope 下的 id |

同一个事件同时出现在汇总流与其对应的独立端点上。汇总流只覆盖该工作区自身的这三类事件，不含 registry 的 `register`/`unregister`/`update`（见 [监听](#监听)）。

流自订阅时刻起只推后续事件，不回放当前集合；调用方断开即停止监听。

### 测试

- `summary::covers`：在 workspace 内新增 skill、plugin、mcp 条目后，汇总流分别出现 `resource` 为 `skill`、`plugin`、`mcp` 的 `register`。
- `summary::shares`：同一个 skill 事件既出现在汇总流、也出现在 `/workspaces/{id}/skills` 上。
- `summary::excludes_registry`：注册另一个 workspace、以及对该 workspace 的 `PATCH` 都不出现在汇总流上。
- `summary::global`：`GET /workspaces/global` 的汇总流报 global 内的事件。
- `summary::negotiation`：不带 `Accept: text/event-stream` 时该端点返回 workspace 对象，带该头时以 `text/event-stream` 应答。
