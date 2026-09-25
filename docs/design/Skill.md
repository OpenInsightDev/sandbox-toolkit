# Skill 设计

把 Agent Skill 作为从固定目录发现的只读资源，通过 HTTP 暴露给远程 agent。

## 定位

- skill 是一个目录，格式遵循 [Agent Skills 规范](https://agentskills.io/specification)：目录内至少有一个 `SKILL.md`，`SKILL.md` 由 frontmatter 与正文构成；
- 资源来自全局与工作区两处 `.agents/skills` 目录的发现，见“发现”；
- 正文经 `/skills` 暴露；skill 内的其它文件复用 [FileSystem.md](./FileSystem.md) 的只读工作区；
- 错误响应沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封。

skill 与 MCP 是彼此独立的资源；MCP 见 [MCP.md](./MCP.md)。

## 发现

skill 从两处固定的 `.agents/skills` 目录发现，各对应一个挂载点：

| 范围   | 目录                              | 挂载点                              |
| ------ | --------------------------------- | ----------------------------------- |
| 全局   | `~/.agents/skills`                | `/skills`                           |
| 工作区 | `{workspace_root}/.agents/skills` | `/workspaces/{workspace_id}/skills` |

- 扫描目录的直接子目录：含合法 `SKILL.md` 的即一个 skill，`{workspace_root}` 是工作区根目录；
- 每次请求按需扫描，目录变化立即反映到查询结果；
- `SKILL.md` 缺失或不满足规范的子目录被静默跳过，不进入列表。

## 资源标识

- `skill_id` 即 skill 目录名，同时必须等于 `SKILL.md` frontmatter 的 `name`；
- 取值遵循 Agent Skills 规范：`[a-z0-9-]`，不允许首尾 `-` 和连续 `-`，最长 64 字符；
- 该字符集同时排除了 `/`、`.` 等分隔符，使 `skill_id` 可直接作为 URL 路径段；
- `skill_id` 在各自挂载点内唯一。

## 查询

```
GET /skills
GET /skills/{skill_id}
GET /workspaces/{workspace_id}/skills
GET /workspaces/{workspace_id}/skills/{skill_id}
```

- `GET .../skills` 返回该挂载点下所有 skill 的元数据，即 Agent Skills 渐进披露中的 metadata 层，支持分页；
- `GET .../skills/{skill_id}` 返回 SKILL 正文，见下；
- 未知 `skill_id` 或不存在的工作区返回 `404`。

全局挂载点的列表条目示例：

```json
{
  "skills": [
    {
      "id": "deploy",
      "root": "/home/u/.agents/skills/deploy",
      "name": "deploy",
      "description": "Roll out a service.",
      "license": "MIT",
      "compatibility": "Requires kubectl",
      "metadata": { "author": "acme" },
      "uri": "/skills/deploy",
      "workspace_id": "skill-global-deploy"
    }
  ]
}
```

- `name`、`description`、`license`、`compatibility`、`metadata` 取自 `SKILL.md` frontmatter；
- `root` 是发现到的 skill 目录；
- 工作区挂载点下 `uri` 为 `/workspaces/{workspace_id}/skills/{skill_id}`，`workspace_id` 见“附属文件：只读工作区”。

## SKILL 正文

```
GET /skills/{skill_id}
GET /workspaces/{workspace_id}/skills/{skill_id}
```

返回 `SKILL.md` frontmatter 之后的正文 Markdown，`Content-Type: text/markdown; charset=utf-8`。

- 只返回正文；frontmatter 字段经查询端点提供；
- 未知 `skill_id` 返回 `404`。

渐进披露由三层构成：查询端点提供 metadata，本端点提供正文，附属文件按需通过工作区获取。

## 附属文件：只读工作区

skill 内其它文件（`scripts/`、`references/`、`assets/` 等）复用 [FileSystem.md](./FileSystem.md) 的工作区机制：每个发现的 skill 自动对应一个只读工作区。

- 工作区 id 为 `skill-global-{skill_id}`（全局）或 `skill-{workspace_id}-{skill_id}`（工作区），根目录为发现到的 skill 目录；
- 访问方式为 `/workspaces/skill-global-{skill_id}/fs/{relative-path}` 或 `/workspaces/skill-{workspace_id}-{skill_id}/fs/{relative-path}`，复用 fs 的路径规范化、边界校验、目录查询与 ETag；
- 只读由 [Workspace.md](./Workspace.md) 的工作区属性承载：固定 `access=read-only`，变更操作返回 `403 read_only_workspace`；
- 生命周期随发现：工作区随 `skills` 目录变化自动增减。

对工作区 API 的附加要求：

- 直接删除这类工作区返回 `403 managed_workspace`；

## 错误协议

沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封：状态码表达通用语义，`error.code` 提供稳定的机器可读分类。

| 状态  | `error.code`          | 场景                      |
| ----- | --------------------- | ------------------------- |
| `400` | `bad_request`         | `skill_id` 等路径参数非法 |
| `403` | `read_only_workspace` | 对派生工作区发起变更操作  |
| `403` | `managed_workspace`   | 直接删除派生工作区        |
| `404` | `not_found`           | skill 或工作区不存在      |
| `405` | `method_not_allowed`  | 端点不支持该方法          |

## 暂不纳入

- skill 版本管理与历史；
- skill 的注册、注销，以及从归档或远程源拉取：只从既有目录发现；
- `scripts/` 内可执行文件的分发与执行权限策略。
