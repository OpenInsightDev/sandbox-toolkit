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

Workspace 监听 `root/.agents` 目录的存在性，并据此决定是否持有资源集合（见 [MCP 设计](./MCP.md)）。

`root/.agents` 存在时构造资源集合，不存在时为 `None`；运行期 `.agents` 出现或消失时同样构造或丢弃。

资源缺失时，其挂载一律回答 `404`：`GET /mcps` 与 `/mcps/{id}` 均如此。

### 测试

- `resources::absent`：无 `.agents` 时注册与启动成功，`GET /mcps` 与 `/mcps/{id}` 返回 `404`。
- `resources::present`：有 `.agents` 时资源集合被构造，`GET /mcps` 返回 `200`。
- `resources::toggles`：运行期创建 `.agents` 后 `GET /mcps` 变为 `200`，删除后回到 `404`。
