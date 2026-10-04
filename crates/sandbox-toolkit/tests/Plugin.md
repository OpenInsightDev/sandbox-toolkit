# Plugin 设计

## 概览

从 `<root>/.agents/plugins` 发现 [Agent Plugins](https://agent-plugins.org/specification) 1.0.0 插件包：每个直接子目录是一个包，`plugin.json` 是它的清单，`skills/` 与 `mcp.json` 是它的组件，解析交给 `agent-plugins`。

Plugin 是 Workspace 持有的资源之一（见 [Workspace 设计](./Workspace.md)），只提供清单查询。它不把组件放进自己的应答：构造工作区资源时先加载 Plugin，再把它交给 [Skill 设计](./Skill.md) 与 [MCP 设计](./MCP.md)，由二者在各自的挂载点呈现。与 Skill 相同，它每次请求重新发现，清单反映目录当下的状态。

## 加载

`.agents/plugins` 的直接子目录逐个交给 `agent-plugins` 加载。目录名只是容器：plugin id 取清单的 `name`，`root` 是该目录的规范绝对路径。

`agent-plugins` 报出的失败让整个资源集合失效，该挂载的 `/plugins`、`/mcps`、`/skills` 一律回答 `404`：

| 失败 | 来源 |
| --- | --- |
| `plugin.json` 缺失、不是普通文件、逃逸出包根，或不符合清单规范 | 加载被拒 |
| `mcp.json` 顶层不合法，或 `$schema` 与清单的版本不一致 | MCP 组件被禁用 |
| 同一工作区内两个目录的清单 `name` 相同 | 本设计 |

非致命的问题按规范记录并忽略，不使构造失败：未知顶层字段、非对象的 `extensions`、不可用的组件位置、被跳过的单个 skill 与 server 条目。

### 测试

- `load::discovers`：每个含合法 `plugin.json` 的直接子目录都出现在 `GET /plugins` 中。
- `load::id`：id 取清单的 `name`；子目录名与之不同时 id 仍是 `name`，`root` 仍是该目录。
- `load::rescans`：新增或删除 plugin 目录后，下一次 `GET /plugins` 反映新的集合。
- `load::rejected`：`plugin.json` 缺失、非法 JSON 或 `$schema` 不识别时，`GET /plugins`、`GET /mcps`、`GET /skills` 都返回 `404`。
- `load::disabled_mcp`：`mcp.json` 顶层不合法时 `GET /plugins`、`GET /mcps`、`GET /skills` 同样都返回 `404`。
- `load::duplicate`：两个目录的 `name` 相同时 `GET /plugins`、`GET /mcps`、`GET /skills` 都返回 `404`。
- `load::ignored`：未知顶层字段、非对象 `extensions`、`skills/` 下的坏 skill 与 `mcp.json` 里的坏条目都不影响 `GET /plugins` 的 `200`。

## 查询

`GET /plugins` 返回该挂载点的清单，按 id 升序；`GET /plugins/{plugin_id}` 返回其中一条。

```json
[
  {
    "id": "deploy-kit",
    "root": "/home/u/.agents/plugins/deploy-kit",
    "uri": "/plugins/deploy-kit",
    "manifest": {
      "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
      "name": "deploy-kit",
      "version": "1.0.0",
      "description": "Deployment helpers."
    }
  }
]
```

| 字段 | 来源 |
| --- | --- |
| `id` | 清单的 `name` |
| `root` | plugin 目录的规范绝对路径 |
| `uri` | 该挂载点下本条的地址，即 `/plugins/{id}`，不带 origin；工作区挂载带 workspace 前缀 |
| `manifest` | 校验后的 `plugin.json`：`$schema`、`name`，以及写了的 `version`、`description`、`author`、`homepage`、`repository`、`license`、`keywords`、`extensions` |

`manifest` 只含清单本身，不含组件。

`?offset=` 与 `?limit=` 可选地截取清单，缺省返回全部。

### 端点

| 路径 | 语义 |
| --- | --- |
| `/plugins` | global 工作区 |
| `/workspaces/{workspace_id}/plugins` | 指定工作区，[合并](#合并) 后 |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回清单数组，或 `GET /plugins/{plugin_id}` 的单个条目 |
| 404 | 未找到 | 挂载的 `{workspace_id}` 不存在；`{plugin_id}` 不在 [合并](#合并) 集合中；[加载](#加载) 失败；挂载的工作区与 global 都无资源 |

### 测试

- `query::list`：`GET /plugins` 列出全部 plugin，按 id 升序。
- `query::pagination`：`?offset=` 与 `?limit=` 截取清单，缺省返回全部。
- `query::fields`：条目字段与清单一致，`manifest` 不含组件。
- `query::uri`：`uri` 指向该挂载点下的本条地址，工作区挂载带 workspace 前缀。
- `query::one`：`GET /plugins/{id}` 返回对应的单个条目。
- `query::unknown`：`GET /plugins/missing` 返回 `404`。
- `query::workspace`：`GET /workspaces/{id}/plugins` 返回合并后的清单。
- `query::unknown_workspace`：访问不存在的 workspace 返回 `404`。

## 合并

工作区对外呈现的 plugin 集合是 global 与工作区自身的并集，按 id 合并，同名时工作区条目胜出；`/plugins`（无前缀）解析到 global。

工作区自身资源缺失不影响合并结果，仅当 global 与工作区都无资源时，工作区挂载回答 `404`；资源门控与资源集合的构成见 [Workspace 设计](./Workspace.md)。

### 测试

- `merge::includes_global`：global 与工作区各自的 plugin 都出现在 `/workspaces/{id}/plugins` 中。
- `merge::workspace_wins`：同名 id 经 `/workspaces/{id}/plugins/{plugin_id}` 读到的是工作区自身的 plugin。
- `merge::absent_workspace`：工作区自身无 `.agents` 而 global 有 plugin 时，`/workspaces/{id}/plugins` 返回 global 的条目而非 `404`。

## 变量

plugin 的 `mcp.json` 按[插件变量](https://agent-plugins.org/plugin-authors/mcp-servers#plugin-variables)解析，注入两个保留变量：

| 变量 | 取值 |
| --- | --- |
| `PLUGIN_ROOT` | plugin 目录 |
| `PLUGIN_DATA` | plugin 目录下的 `.data` |

`.data` 在 plugin 包内、不在发现范围里，而 `agent-plugins` 只读 `plugin.json`、`mcp.json` 与 `skills/`，它不影响加载。

§9.1 要求 `PLUGIN_DATA` 的内容跨 plugin 更新保留。本设计只发现 plugin，不安装也不更新，该要求眼下没有落点；它约束的是将来若以整体替换目录的方式更新，那时 `.data` 会随目录一起丢弃，就地更新不受影响。

保留变量只属于 plugin 的 stdio 条目；`.agents/mcp.json` 自身的条目不注入，见 [MCP 设计](./MCP.md)。

### 测试

- `variables::anchors`：plugin 的 stdio 条目拉起子进程时 `PLUGIN_ROOT` 为该 plugin 目录，`PLUGIN_DATA` 为其下的 `.data`。

## 派生工作区

清单 `extensions` 里声明的每个命名空间派生一个工作区，id 为 `plugin.{scope}.{plugin_id}.{extension_id}`，`scope` 是持有该 plugin 的工作区。该 id 与注册工作区一样可直接用于 workspace 端点，但派生工作区不注册：

- 可解析当且仅当 `scope` 可解析、`{plugin_id}` 当前被该 scope 发现、清单此刻声明该命名空间；其他形态回答 `404`；
- `root` 为 plugin 目录下与命名空间同名的顶层目录的路径，清单只声明数据、没有该目录时同样解析，`access` 继承 `scope` 的 `access`；
- 出现在 `GET /workspaces/{workspace_id}`，不出现在 `GET /workspaces`；
- `PATCH` 与 `DELETE` 回答 `403`。

plugin 名与反向域名都允许 `.`，因此这些 id 不按分隔符切分，而是按当前发现结果查表解析。

### 测试

- `extension::resolves`：`GET /workspaces/plugin.{scope}.{id}.{ns}` 返回以该命名空间目录为 `root` 的工作区。
- `extension::not_listed`：`GET /workspaces` 不含派生工作区。
- `extension::mutate_denied`：对派生工作区的 `PATCH` 与 `DELETE` 回答 `403`。
- `extension::unknown`：未被声明或未被发现的 `plugin.{scope}.{id}.{ns}` 回答 `404`。
