# FileSystem 设计

## 概览

FileSystem 挂载在 `/fs`，在工作区内提供文件与目录操作：读取元数据、内容、目录与通配匹配，写入文件与创建目录、符号链接，修改元数据、应用补丁、截断，复制、移动与删除。

端点固定，操作由方法与查询串里的 `type` 共同决定；参数通过 JSON 请求体传入，唯一的例外是[字节通道](#字节通道)。

各 `type` 的字段语义、状态码与错误码另行定义。

## 挂载

fs 与 exec、pty 同一挂载规则（见 [Exec 设计](./Exec.md)），两种寻址模式：

| 路径 | 语义 | 寻址 |
| --- | --- | --- |
| `/fs` | global 工作区 | 绝对路径 |
| `/workspaces/{workspace_id}/fs` | 指定工作区 | 工作区相对路径 |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 404 | 未找到 | `{workspace_id}` 不存在 |

### 测试

- `mount::direct`：`/fs` 以绝对路径寻址。
- `mount::workspace`：`/workspaces/{id}/fs` 以工作区相对路径寻址。
- `mount::unknown_workspace`：访问不存在的 workspace 返回 `404`。

## 请求

`type` 在查询串里给出，其余参数一律在 JSON 请求体中给出，`path` 是其中一个字段。

| 方法 | `type` | 操作 | body（关键字段） |
| --- | --- | --- | --- |
| `QUERY` | `content` | 读取文件内容 | `path` |
| `QUERY` | `stream` | 流式下载 | `path` |
| `QUERY` | `metadata` | 查询单个资源 | `path` |
| `QUERY` | `list` | 列目录 | `path`、`depth`、`offset`、`limit` |
| `QUERY` | `glob` | 通配匹配 | `path`、`pattern`、`exclude`、`offset`、`limit` |
| `QUERY` | `realpath` | 解析真实路径 | `path` |
| `QUERY` | `access` | 探测访问权限 | `path` |
| `QUERY` | `lines` | 按行读取文件 | `path`、`offset`、`limit` |
| `QUERY` | `watch` | 监听变更 | `path`、`recursive` |
| `PUT` | `file` | 写入文件 | `path`、`content` |
| `PUT` | `directory` | 创建目录 | `path`、`recursive` |
| `PUT` | `symlink` | 创建符号链接 | `path`、`target` |
| `PUT` | `stream` | 写入原始字节 | body 即内容，`path` 在查询串 |
| `PATCH` | `metadata` | 修改元数据 | `path`、可变字段 |
| `PATCH` | `patch` | 应用文本补丁 | `path`、`format`、`patch` |
| `PATCH` | `truncate` | 截断文件 | `path`、`length` |
| `POST` | `copy` | 复制 | `path`、`destination` |
| `POST` | `move` | 移动 / 重命名 | `path`、`destination` |
| `DELETE` | — | 删除 | `path`、`recursive`、`force` |

### 测试

- `request::body`：参数在 JSON 请求体中给出；URL 只承载 `type`。

## 字节通道

`type=stream` 是内容本身进出的通道。`QUERY type=stream` 按 body 里的 `path` 流式返回文件字节；`PUT type=stream` 的 body 就是文件内容，因此 `path` 是查询串里唯一的参数。

查询串里的 `path` 是 RFC 3986 的查询值：UTF-8 百分号编码，`+` 是字面量而非空格；服务端只做百分号解码。

文本与小内容走 `PUT type=file` 的 JSON `content`；二进制与大文件走 `type=stream`。

### 测试

- `stream::write`：`PUT type=stream` 把 body 字节原样写入查询串给出的 `path`。
- `stream::read`：`QUERY type=stream` 按 body 里的 `path` 返回文件原始字节。
- `stream::path_encoding`：`path` 含 `+`、`%`、空格与非 ASCII 字符时按字节精确寻址。
