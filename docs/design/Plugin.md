# Plugin 设计

把 Agent Plugin 作为从固定目录发现的包，展开为其组件与 client extension 文件，经既有端点暴露给远程 agent。

## 定位

- plugin 是一个目录，格式遵循 [Agent Plugins 规范](https://agent-plugins.org/specification)：根目录含 `plugin.json`，可选含 `skills/` 目录、`mcp.json` 与按反向域名命名的 client extension 顶层目录；
- 资源来自全局与工作区两处 `.agents/plugins` 目录的发现，见“发现”；
- 服务端只做发现与展开：plugin 内的 skill 与 MCP server 以命名空间 id 并入既有资源，经 [Skill.md](./Skill.md)、[MCP.md](./MCP.md) 的端点查询和使用；manifest 经 `/plugins` 暴露；client extension 文件复用 [FileSystem.md](./FileSystem.md) 的只读工作区；
- 错误响应沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封。

## 发现

plugin 从两处固定的 `.agents/plugins` 目录发现，各对应一个挂载点：

| 范围   | 目录                               | 挂载点                                                            |
| ------ | ---------------------------------- | ----------------------------------------------------------------- |
| 全局   | `~/.agents/plugins`                | `/plugins`，组件并入 `/skills`、`/mcps`                           |
| 工作区 | `{workspace_root}/.agents/plugins` | `/workspaces/{workspace_id}/plugins`，组件并入该工作区的 `/skills`、`/mcps` |

- 扫描目录的直接子目录：根目录含合法 `plugin.json` 的即一个 plugin，`{workspace_root}` 是工作区根目录；
- 每次请求按需扫描，目录变化及其组件增减立即反映到查询结果；
- `plugin.json` 按规范“Manifest”校验：缺失、`$schema` 不支持或其它模式违规的子目录被跳过，不进入清单，也不影响其它 plugin；未知顶层字段与非对象 `extensions` 按规范报告后忽略；
- `skills/`、`mcp.json` 存在但类型不符时，只令对应组件类型失效，不影响其它组件；
- plugins 目录下的 `.data` 目录不是 plugin，见“插件变量”。

## 资源标识

- `plugin_id` 即 plugin 目录名，与 manifest 的 `name` 相互独立；
- 取值与规范“Plugin name constraints”一致：`[a-z0-9.-]`，首尾为字母数字，不含 `--` 与 `..`，长度 1-64；
- 该字符集排除 `/`，使 `plugin_id` 可直接作为 URL 路径段；`plugin_id` 在各自挂载点内唯一。

Plugin 贡献的组件以命名空间 id 并入所属挂载点：

| 组件       | id                       |
| ---------- | ------------------------ |
| skill      | `<plugin_id>.<skill_id>` |
| MCP server | `<plugin_id>.<mcp_id>`   |

- `skill_id`、`mcp_id` 遵循各自规范，均不含 `.`，故命名空间 id 可在最后一个 `.` 处无歧义拆分；
- 命名空间 id 含 `.`，挂载点直接发现的 id 不含 `.`，两组 id 不重名；命名空间 id 在并入后的挂载点内唯一。

## 查询

```
GET /plugins
GET /plugins/{plugin_id}
GET /workspaces/{workspace_id}/plugins
GET /workspaces/{workspace_id}/plugins/{plugin_id}
```

- `GET .../plugins` 返回该挂载点下所有 plugin 的清单，条目为描述符，支持分页；
- `GET .../plugins/{plugin_id}` 返回单个 plugin 描述符；
- 未知 `plugin_id` 或不存在的工作区返回 `404`。

全局挂载点的列表条目示例：

```json
{
  "plugins": [
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
}
```

- `root` 是发现到的 plugin 根目录；
- `manifest` 是 `plugin.json` 解析后的对象，字段与约束见规范“Manifest”；
- 工作区挂载点下 `uri` 为 `/workspaces/{workspace_id}/plugins/{plugin_id}`。

## 组件

plugin 的组件不新增端点，直接并入所属挂载点的既有资源。

### skills

- 从 `<plugin_root>/skills` 的直接子目录发现含合法 `SKILL.md` 的目录，不递归，规则同 [Skill.md](./Skill.md) 的“发现”；
- 不满足规范的子目录静默跳过；
- 发现的 skill 以 `<plugin_id>.<skill_id>` 并入该挂载点的 skill 清单与正文端点；其附属文件工作区沿用 [Skill.md](./Skill.md) 的命名规则，代入命名空间后的 `skill_id`。

### mcp.json

- 从 `<plugin_root>/mcp.json` 读取，格式与加载规则同 [MCP.md](./MCP.md) 的“配置”，缺失即该 plugin 无 MCP；
- 顶层无效，或 `$schema` 与 `plugin.json` 不一致、指向不支持的版本时，禁用该 plugin 的 MCP，不影响其它组件；
- 单个条目不满足约束，或声明的 `type` 不受支持（如 `sse`）时，跳过该条目；
- 有效条目以 `<plugin_id>.<mcp_id>` 并入该挂载点的 MCP 清单、[MCP.md](./MCP.md) 的 MCP 端点与 `format=mcp-json` 导出。

### 插件变量

plugin 条目沿用 [MCP.md](./MCP.md) 的“插件变量”，差别只在取值基准为 plugin 根目录：

| 变量          | 取值                                                   |
| ------------- | ------------------------------------------------------ |
| `PLUGIN_ROOT` | plugin 根目录                                          |
| `PLUGIN_DATA` | 分配给该 plugin 的专有可写目录 `${plugins_dir}/.data/{plugin_id}` |

- 两个变量注入 stdio 子进程环境并展开 `args`、`env` 值、`cwd`，展开规则同 [MCP.md](./MCP.md)；
- `plugins_dir` 为该挂载点的 plugins 目录；`PLUGIN_DATA` 按 plugin 而非按条目分配，随 `.agents` 目录持久化，不随 plugin 卸载删除；
- `./` 开头的 `command`、`cwd` 相对 plugin 根目录解析，解析后必须留在 plugin 根目录内；
- 工作区挂载点下同时受该工作区边界约束，并按 [Workspace.md](./Workspace.md) 的环境变量注入工作区根目录。

## 附属文件：只读工作区

plugin 的 client extension 目录复用 [FileSystem.md](./FileSystem.md) 的工作区机制：每个命名空间目录自动对应一个只读工作区。

- 命名空间目录是 plugin 根目录下名称符合反向域名形式（含至少一个 `.`，各标签为小写字母数字与 `-`）的顶层目录，与 manifest 的 `extensions` 条目相互独立；
- 工作区 id 全局为 `plugin-{plugin_id}-{namespace}`、工作区 plugin 为 `plugin-{workspace_id}-{plugin_id}-{namespace}`，如 `plugin-deploy-kit-com.example.client`；根目录为发现到的命名空间目录；
- 访问方式为 `/workspaces/{workspace_id}/fs/{relative-path}`，复用 fs 的路径规范化、边界校验、目录查询与 ETag；
- 只读由 [Workspace.md](./Workspace.md) 的工作区属性承载：固定 `access=read-only`，变更操作返回 `403 read_only_workspace`；
- 生命周期随发现：工作区随 plugins 目录变化自动增减。

对工作区 API 的附加要求：

- 直接删除这类工作区返回 `403 managed_workspace`；
- 派生工作区 id 允许 `.`（来自反向域名、`plugin_id` 与命名空间后的组件 id），是 [Workspace.md](./Workspace.md) 字符集 `[a-z0-9-]` 之外的唯一扩展。

## 错误协议

沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封：状态码表达通用语义，`error.code` 提供稳定的机器可读分类。

| 状态  | `error.code`          | 场景                        |
| ----- | --------------------- | --------------------------- |
| `400` | `bad_request`         | `plugin_id` 等路径参数非法  |
| `403` | `read_only_workspace` | 对派生工作区发起变更操作    |
| `403` | `managed_workspace`   | 直接删除派生工作区          |
| `404` | `not_found`           | plugin 或工作区不存在       |
| `405` | `method_not_allowed`  | 端点不支持该方法            |

## 暂不纳入

- plugin 的注册、注销，以及从归档或远程源拉取：只从既有目录发现；
- 安装、更新与版本管理：`version` 只随 manifest 返回，不驱动拉取或缓存失效；
- `extensions` 中按 namespace 定义的行为语义：只暴露对应目录的文件，不解释内容。
