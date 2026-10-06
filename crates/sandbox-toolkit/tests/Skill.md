# Skill 设计

## 概览

从 `.agents/skills` 与所在工作区持有的 plugin 的 `skills/` 发现 [Agent Skills](https://agentskills.io/specification)，格式严格遵循该规范：skill 目录的 `SKILL.md` 携带 frontmatter 与正文，正文按需读取，附带的文件经派生工作区读取。

Skill 是 Workspace 持有的资源之一，其门控与合并沿用 [Workspace 设计](./Workspace.md) 与 [MCP 设计](./MCP.md) 的规则；plugin 一侧的构成见 [Plugin 设计](./Plugin.md)。

## 加载

在 `<root>/.agents/skills` 的直接子目录中，`SKILL.md` 通过规范校验的即为一个 skill。规范要求 frontmatter 的 `name` 与目录名一致，因此目录名即 skill id。更深的 `SKILL.md` 不计入。未通过校验的子目录被跳过，不影响其他 skill。

plugin 的 `skills/` 下每个直接子目录同样是一个 skill，id 为 `{plugin_id}.{skill_id}`，其余规则相同；plugin 一侧的加载失败由资源集合整体承担。

每次请求重新发现，目录的变化立即反映在应答中。

### 测试

- `load::discovers`：`.agents/skills` 下每个合法子目录都出现在 `GET /skills` 中，id 为目录名。
- `load::plugin`：plugin 的 `skills/` 下每个合法子目录也出现在 `GET /skills` 中，id 为 `{plugin_id}.{skill_id}`。
- `load::skips`：缺 `SKILL.md`、frontmatter 不合法、`name` 与目录名不一致的子目录，以及嵌套目录下的 `SKILL.md`，都不出现。
- `load::rescans`：新增或删除 skill 目录后，下一次 `GET /skills` 反映新的集合。

## 合并

工作区对外呈现的 skill 集合是 global 与工作区自身的并集，按 id 合并，同名时工作区条目胜出；`/skills`（无前缀）解析到 global。

plugin 提供的 skill id 带 `{plugin_id}.` 前缀，而工作区自身的 skill id 取目录名、不含 `.`，两者不会同名；前缀相同的两份只在工作区与 global 持有同名 plugin 时出现，此时工作区条目胜出。

工作区自身资源缺失不影响合并结果，仅当 global 与工作区都无资源时，工作区挂载回答 `404`；资源门控与资源集合的构成见 [Workspace 设计](./Workspace.md)。

### 测试

- `merge::includes_global`：global 与工作区各自的 skill 都出现在 `/workspaces/{id}/skills` 文档中。
- `merge::workspace_wins`：同名 id 经 `/workspaces/{id}/skills/{skill_id}` 读到的正文来自工作区自身。
- `merge::plugin_prefix`：plugin 提供的 skill 以 `{plugin_id}.` 为前缀出现。
- `merge::absent_workspace`：工作区自身无 `.agents` 而 global 有 skill 时，`/workspaces/{id}/skills` 返回 global 的条目而非 `404`。

## 列表

`GET /skills` 返回该挂载点的 skill 文档；工作区挂载返回 [合并](#合并) 后的文档。条目按 id 升序排列。

```json
{
  "skills": [
    {
      "id": "deploy",
      "root": "/home/u/.agents/skills/deploy",
      "name": "deploy",
      "description": "Roll out a service.",
      "license": "MIT",
      "metadata": { "author": "acme" },
      "uri": "http://127.0.0.1:3000/skills/deploy",
      "workspace_id": "skill.global.deploy"
    }
  ]
}
```

| 字段 | 来源 |
| --- | --- |
| `id` | skill 目录名；plugin 提供的 skill 为 `{plugin_id}.{skill_id}` |
| `root` | skill 目录的规范绝对路径 |
| `name`、`description` | frontmatter 的必填字段 |
| `license`、`compatibility`、`metadata` | frontmatter 写了的才出现 |
| `uri` | 该挂载点下正文的地址 |
| `workspace_id` | 派生工作区 id，见 [派生工作区](#派生工作区) |

规范中的 `allowed-tools` 不进入应答。

`uri` 按 [MCP 设计](./MCP.md) 的地址规格，由请求 origin 与挂载路径加 `/{skill_id}` 拼成；`workspace_id` 命名发现该 skill 的工作区。合并视图下二者指向的 scope 可以不同。

### 端点

| 路径 | 语义 |
| --- | --- |
| `/skills` | global 工作区 |
| `/workspaces/{workspace_id}/skills` | 指定工作区，合并后 |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回 skill 文档 |
| 404 | 未找到 | workspace 挂载下 `{workspace_id}` 不存在；挂载的工作区与 global 都无资源 |

### 测试

- `list::skills`：`GET /skills` 列出全部合法 skill，条目按 id 升序。
- `list::fields`：条目字段与 frontmatter 一致，未写的可选字段不出现。
- `list::uri`：`uri` 指向该挂载点下的正文地址，工作区挂载带 workspace 前缀。
- `list::workspace`：`GET /workspaces/{id}/skills` 返回合并后的文档。
- `list::unknown_workspace`：访问不存在的 workspace 返回 `404`。

## 正文

`GET /skills/{skill_id}` 返回 `SKILL.md` 去掉 frontmatter 之后的正文原文，不做重排，`Content-Type` 为 `text/markdown`。

### 端点

| 路径 | 语义 |
| --- | --- |
| `/skills/{skill_id}` | global 工作区 |
| `/workspaces/{workspace_id}/skills/{skill_id}` | 指定工作区 |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 返回正文 |
| 404 | 未找到 | workspace 挂载下 `{workspace_id}` 不存在；`{skill_id}` 不在 [合并](#合并) 集合中 |

### 测试

- `read::body`：`GET /skills/{id}` 返回 frontmatter 之后的原文，`Content-Type` 为 `text/markdown`。
- `read::merged`：工作区挂载下读取只在 global 出现的 skill 同样返回其正文。
- `read::not_found`：读取未被发现的 `{skill_id}` 返回 `404`。

## 派生工作区

每个发现的 skill 派生一个工作区，id 为 `skill.{scope}.{skill_id}`，`scope` 是发现该 skill 的工作区。该 id 与注册工作区一样可直接用于 workspace 端点，但派生工作区不注册：

- 可解析当且仅当 `{skill_id}` 当前被 `scope` 发现；`scope` 是 `global` 或已注册工作区，其他形态不解析，回答 `404`；
- `root` 为该 skill 目录的规范绝对路径，`access` 继承 `scope` 的 `access`；
- 出现在 `GET /workspaces/{workspace_id}`，不出现在 `GET /workspaces`；
- `PATCH` 与 `DELETE` 回答 `403`。

plugin 提供的 skill 其 id 含 `.`，因此这些 id 不按分隔符切分，而是按已知 `scope` 与当前发现结果查表解析。

### 测试

- `workspace::resolves`：`GET /workspaces/skill.{scope}.{id}` 返回以该 skill 目录为 `root` 的工作区。
- `workspace::usable`：该 id 可直接用于 workspace 端点，命令的缺省 `cwd` 为该 skill 目录。
- `workspace::access`：派生工作区的 `access` 与来源 `scope` 一致。
- `workspace::not_listed`：`GET /workspaces` 不含派生工作区。
- `workspace::unknown`：未被发现的 `skill.{scope}.{id}` 与未注册的 `scope` 都回答 `404`。
- `workspace::mutate_denied`：对派生工作区的 `PATCH` 与 `DELETE` 回答 `403`。
- `workspace::plugin_skill`：plugin 提供的 skill 其 id 含 `.`，派生工作区 id 仍解析到该 skill 目录。

## 监听

`GET /skills`（工作区挂载为 `GET /workspaces/{workspace_id}/skills`）带 `Accept: text/event-stream` 时不返回文档，而打开一条 SSE 流，推送该挂载点 scope 的 skill 生命周期事件；不带该头时照常返回 [列表](#列表)。

scope 的 `.agents/skills` 与它全部 plugin 的 `skills/` 由同一观察者监听，事件合并在同一条流上。

| 事件 | 触发条件 |
| --- | --- |
| `register` | 一个通过 [加载](#加载) 校验的 skill id 新进入该 scope 的发现集合 |
| `unregister` | 一个 id 离开发现集合 |
| `update` | 一个已注册的 id 的 `SKILL.md` 发生变动 |

`id` 与 [列表](#列表) 一致：scope 自身取目录名，plugin 提供者取 `{plugin_id}.{skill_id}`。skill 目录内其他文件的变动不产生事件。

流以 `text/event-stream` 承载，每个事件由一行 `event:`（事件名）、一行 `data:`（`{"id": "<skill_id>"}`）与一个空行结束：

```
event: register
data: {"id":"deploy"}

```

流自订阅时刻起只推后续事件，不回放当前集合；调用方断开即停止监听。

### 测试

- `events::register`：新建合法 skill 目录后流上出现 `register`，`id` 为目录名，且不出现 `update`。
- `events::unregister`：删除已发现的 skill 目录后流上出现 `unregister`。
- `events::update`：改动已发现 skill 的 `SKILL.md` 后流上出现 `update`。
- `events::skips`：无合法 `SKILL.md` 的目录出现或消失都不产生事件。
- `events::ignores_other_files`：改动 skill 目录内的其他文件不产生事件。
- `events::plugin`：plugin 的 `skills/` 下 skill 变动同样出现在流上，`id` 带 `{plugin_id}.` 前缀。
- `events::workspace`：`GET /workspaces/{id}/skills` 的流报该 scope 的事件。
- `events::negotiation`：不带 `Accept: text/event-stream` 时该端点返回列表文档，带该头时以 `text/event-stream` 应答。
