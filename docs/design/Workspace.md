# Workspace 需求

Workspace 是对远程文件系统中可操作目录的抽象，解决两类问题：

- **寻址**：把一个远程目录注册为指定 `id` 的工作区，后续操作以 `/workspaces/{id}` 前缀 + 相对路径定位文件；
- **整体管理**：以目录为粒度管理工作区，例如确保用户在该目录的权限，以及对该目录进行版本管理、git 操作等。

工作区是路径访问的边界，复用 [FileSystem.md](./FileSystem.md) 的严格路径模型与边界校验；MCP、skill 等资源复用工作区作为运行和访问边界，见 [MCP.md](./MCP.md)、[Skill.md](./Skill.md)。

## 路由与寻址

工作区是路由前缀；功能既可以挂在某个工作区下（工作区模式），也可以直接使用（直接模式）。

| 模式       | 路由前缀           | 路径           |
| ---------- | ------------------ | -------------- |
| 工作区模式 | `/workspaces/{id}` | 工作区相对路径 |
| 直接模式   | 无                 | 远程绝对路径   |

工作区模式下，功能各占一个子路由：`/workspaces/{id}/fs` 与 `/workspaces/{id}/upload`（[FileSystem.md](./FileSystem.md)）、`/workspaces/{id}/exec`、`/workspaces/{id}/shell` 与 `/workspaces/{id}/pty`（[Process.md](./Process.md)）、`/workspaces/{id}/mcps`（[MCP.md](./MCP.md)）、`/workspaces/{id}/skills`（[Skill.md](./Skill.md)）。两种模式不得混用；路径参数的缺省值由各功能自行规定。

## 路径规则

两种模式都遵循 [FileSystem.md](./FileSystem.md) 的严格路径模型：规范化、只解码一次、路径错误不自动修正；工作区模式下解析结果必须位于工作区根目录内，直接模式下为规范化绝对路径。

## 注册与管理

```
POST   /workspaces
GET    /workspaces
GET    /workspaces/{workspace_id}
DELETE /workspaces/{workspace_id}
```

`/workspaces/{workspace_id}` 既是工作区句柄，也是挂载各功能与资源子路由的前缀，见“路由与寻址”。

### 资源标识

注册时由调用方指定唯一 `id`，字符集限定为 `[a-z0-9-]`，不允许首尾 `-` 和连续 `-`；`id` 作为 URL 路径段，并构成其它资源派生工作区 id 的前缀。

`skill-` 前缀保留给 [Skill.md](./Skill.md) 派生的只读工作区，用户不能创建、删除或改名以该前缀开头的工作区；`global` 亦保留，用于全局 skill 的派生工作区命名空间。

### 注册请求

```json
{ "id": "docs", "root": "/srv/project", "properties": { "access": "read-only" } }
```

- `root` 是远程绝对路径，只在注册这个控制面边界接受；
- 注册时校验 `root` 已存在且是目录，并做规范化，使后续边界校验有稳定基准；
- `properties` 可选，属性与取值见“工作区属性”；
- 注册是控制面操作，不引入 ETag 条件；
- 成功后返回 `201` 与工作区句柄；句柄回显 `id` 与生效属性，不回显 `root`。

### 管理

- `GET /workspaces` 返回已注册工作区清单，按 id 排序；
- `GET /workspaces/{workspace_id}` 返回单个工作区句柄；
- `DELETE /workspaces/{workspace_id}` 注销工作区，成功返回 `204`；注销只解除注册，不删除、不修改远程目录本身。

## 工作区属性

工作区属性是注册时确定的约束，用于收窄该工作区内各功能的行为，注册后不可修改；缺少某个属性时取默认值。

| 属性         | 键       | 取值                      | 缺省         |
| ------------ | -------- | ------------------------- | ------------ |
| 文件读写模式 | `access` | `read-write`、`read-only` | `read-write` |

文件读写模式约束 [FileSystem.md](./FileSystem.md) 的变更操作：

- `read-write`：读写不受限；
- `read-only`：拒绝写、创建、删除、移动等变更操作，返回 `403 read_only_workspace`；读取、目录查询与元数据不受影响。

## 环境变量

每个工作区在进程环境中对应一个环境变量，把工作区根目录提供给在其内运行的命令，使命令无需硬编码远程绝对路径即可引用工作区：

| 项   | 规则                                                                       |
| ---- | -------------------------------------------------------------------------- |
| 名称 | `WORKSPACE_` 前缀加 `id`，转大写、`-` 转 `_`，如 `docs` → `WORKSPACE_DOCS` |
| 取值 | 工作区根目录的规范化绝对路径                                               |

- 名称由 `id` 唯一决定：`id` 不含 `_`，`-` 与 `_` 的映射可逆，不同 `id` 不会得到同名变量；
- 工作区模式下运行命令时注入当前工作区的变量，exec、shell 与 pty session 见 [Process.md](./Process.md)，MCP stdio 子进程见 [MCP.md](./MCP.md)；直接模式没有工作区，不注入；
- 请求的 `env` 与注入变量同名时，`env` 覆盖生效。

## 解析与解析失败

服务端把功能路由前缀中的 `{workspace_id}` 解析为工作区根目录后再执行操作，解析失败按稳定错误码返回，不落回绝对路径或宿主默认目录：

| 场景                                 | 状态  | `error.code`            |
| ------------------------------------ | ----- | ----------------------- |
| `id` 非法                            | `400` | `bad_request`           |
| `root` 非法（非绝对/不存在/非目录）  | `400` | `bad_request`           |
| 工作区不存在                         | `404` | `not_found`             |
| `id` 已存在                          | `409` | `conflict`              |
| 创建 `skill-` 前缀或 `global` 工作区 | `409` | `workspace_id_reserved` |

## 权限与生命周期

- 注册时必须确认沙箱进程对目标目录具备所需权限；不具备时注册失败；
- 每次操作都必须校验工作区存在、具备权限，且解析后的目标位于工作区根目录内，并遵循该工作区属性，见“工作区属性”；
- 被其它资源引用的工作区不能直接注销，返回 `409 workspace_in_use`，须先注销引用方；由上层资源派生并托管的工作区不能直接删除，返回 `403 managed_workspace`。

上述引用约束见 [MCP.md](./MCP.md) 的“工作区耦合”与 [Skill.md](./Skill.md) 的“附属文件：只读工作区”，由工作区 API 统一执行。
