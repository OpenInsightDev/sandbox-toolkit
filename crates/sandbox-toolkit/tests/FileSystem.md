# FileSystem 设计

## 概览

FileSystem 挂载在 `/fs`，在工作区内提供文件与目录操作：读取元数据、内容、目录与通配匹配，写入文件与创建目录、符号链接，修改元数据、应用补丁、截断，复制、移动与删除。

端点固定，操作由方法与查询串里的 `type` 共同决定；参数通过 JSON 请求体传入。文件上传一律走 `/tus`（见 [Tus 设计](./Tus.md)）。写操作成功只报结果：回 `204`，不带响应体。

各 `type` 的字段与行为见下文，状态码见「请求」一节。

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
| `QUERY` | `content` | 读取 utf-8 文本内容 | `path` |
| `QUERY` | `stream` | 流式下载 | `path` |
| `QUERY` | `metadata` | 查询单个资源 | `path` |
| `QUERY` | `list` | 列目录 | `path`、`depth`、`offset`、`limit` |
| `QUERY` | `glob` | 通配匹配 | `path`、`pattern`、`exclude`、`offset`、`limit` |
| `QUERY` | `realpath` | 解析真实路径 | `path` |
| `QUERY` | `access` | 探测访问权限 | `path` |
| `QUERY` | `lines` | 按行读取文件 | `path`、`offset`、`limit` |
| `QUERY` | `watch` | 监听变更 | `path`、`recursive` |
| `PUT` | `content` | 写入文件 | `path`、`content` |
| `PUT` | `directory` | 创建目录 | `path`、`recursive` |
| `PUT` | `symlink` | 创建符号链接 | `path`、`target` |
| `PATCH` | `metadata` | 修改元数据 | `path`、可变字段 |
| `PATCH` | `patch` | 应用文本补丁 | `path`、`format`、`patch` |
| `PATCH` | `truncate` | 截断文件 | `path`、`length` |
| `POST` | `copy` | 复制 | `path`、`destination` |
| `POST` | `move` | 移动 / 重命名 | `path`、`destination` |
| `DELETE` | — | 删除 | `path`、`recursive`、`force` |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 200 | 成功 | 读操作返回结果 |
| 204 | 无内容 | 写操作成功 |
| 400 | 请求错误 | `type` 未知，或字段非法 |
| 403 | 拒绝 | 权限不足 |
| 404 | 未找到 | 目标路径不存在 |
| 409 | 冲突 | 目标已存在，或目录非空且未 `recursive` |
| 422 | 无法处理 | 内容不是合法 UTF-8 |

### 测试

- `request::body`：参数在 JSON 请求体中给出；URL 只承载 `type`。

## `QUERY ?type=content`

读取目标文本文件内容，以 JSON 承载。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |

输出：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 目标路径 |
| `content` | string | UTF-8 解码后的文件内容 |
| `size` | integer | 原始字节数 |

- 只读取小规模文本文件：内容不是合法 UTF-8 返回 `422`；
- 读取二进制文件与大文件走 `?type=stream`。

### 测试

- `content::text`：返回 `path`、`content` 与 `size`。
- `content::not_utf8`：内容不是合法 UTF-8 时返回 `422`。

## `QUERY ?type=stream`

以响应体承载文件原始字节。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |

输出：响应体即文件原始字节，无 JSON 包装。

- 不支持区间读取：请求无法表达 `offset` 与长度。

### 测试

- `stream::read`：按 body 里的 `path` 返回文件原始字节。

## `QUERY ?type=metadata`

查询单个资源的元数据。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标路径 |

输出：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `kind` | string | `file`、`directory` 或 `symlink` |
| `size` | integer | 字节数 |
| `modified_at` | string | 修改时间 |
| `accessed_at` | string \| null | 访问时间 |
| `birthtime` | string \| null | 创建时间 |
| `mode` | string \| null | 权限位，八进制 |
| `device` | integer \| null | 所在设备号 |
| `device_type` | integer \| null | 设备类型 |
| `inode` | integer \| null | inode 号 |
| `links` | integer \| null | 硬链接数 |
| `uid` | integer \| null | 属主 |
| `gid` | integer \| null | 属组 |
| `block_size` | integer \| null | 块大小 |
| `blocks` | integer \| null | 块数 |

- `kind` 与 `modified_at`、`size` 总是有值；
- 平台无法提供的字段为 `null`，不用 `0` 顶替。

### 测试

- `metadata::file`：文件的 `kind` 为 `file`，`size` 为字节数。
- `metadata::directory`：目录的 `kind` 为 `directory`。

## `QUERY ?type=list`

列出目录条目。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标目录，空串表示工作区根 |
| `depth` | integer \| `"infinity"` \| null | 遍历深度，`"infinity"` 不限层，缺省 1 |
| `offset` | integer \| null | 分页起点，缺省 0 |
| `limit` | integer \| null | 每页条目数，缺省不限 |

输出：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `entries` | object[] | 条目数组 |
| `entries[].path` | string | 条目路径，按寻址模式给出 |

- 条目路径带被列出目录的前缀，递归时子目录条目形如 `sub/deep.txt`。

### 测试

- `list::children`：列出工作区根的条目，路径为工作区相对路径。
- `list::recursive`：`depth` 为 `"infinity"` 时含子树条目，路径带前缀。

## `QUERY ?type=glob`

按通配模式匹配路径。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，匹配根，空串表示工作区根 |
| `pattern` | string | 必填，相对匹配根的模式 |
| `exclude` | string[] | 可选，排除匹配的子树 |
| `offset` | integer \| null | 分页起点，缺省 0 |
| `limit` | integer \| null | 每页条目数，缺省不限 |

输出：与 `QUERY ?type=list` 同形。

- `*` 不跨目录分隔符，`**` 跨层；
- 条目路径按寻址模式给出。

### 测试

- `glob::recursive`：`**/*.txt` 匹配到子目录里的文件。
- `glob::single_component`：`*.md` 只匹配匹配根下的条目。
- `glob::exclude`：`exclude` 里的子树不出现在结果里。

## `QUERY ?type=realpath`

解析路径的规范绝对路径。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标路径 |

输出：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 规范绝对路径 |

- 解析 `.`、`..` 与路径中的符号链接，两种寻址模式都回绝对路径；
- 目标不存在时报错。

### 测试

- `realpath::resolves`：解析符号链接后回绝对路径。

## `QUERY ?type=access`

探测目标是否满足请求的访问属性。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标路径 |
| `ok` | boolean \| null | 可选，只要求目标存在 |
| `readable` | boolean \| null | 可选，要求可读 |
| `writable` | boolean \| null | 可选，要求可写 |

输出：无内容（`204`）。

- 只检查被请求的属性，任一不满足即报错；
- 三个都不给时只要求目标存在。

### 测试

- `access::probes`：存在且可读写的目标在 `ok`、`readable`、`writable` 下都回 `204`。

## `QUERY ?type=lines`

按行读取文本文件。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |
| `offset` | integer \| null | 起始行号，从 0 开始，缺省 0 |
| `limit` | integer \| null | 每页行数，缺省 1000 |

输出：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `lines` | string[] | 本页的行，不含行尾符 |
| `truncated` | boolean | 是否还有后续行 |

- 文件末尾没有换行的最后一行照样返回；
- 还有后续行时 `truncated` 为真，下一页用 `offset + lines.length` 续读。

### 测试

- `lines::reads`：行不含行尾符。
- `lines::trailing`：末尾没有换行的最后一行照样返回。
- `lines::pages`：超过一页的文件按 `truncated` 与 `offset` 分页读完。

## `QUERY ?type=watch`

监听路径变更，以 NDJSON 承载事件。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，监听目标，空串表示工作区根 |
| `recursive` | boolean \| null | 是否上报子树的变更，缺省否 |

输出：每行一个事件对象。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `event` | string | `create`、`update` 或 `remove` |
| `path` | string | 变更路径，按寻址模式给出 |

- 响应保持打开，调用方断开即停止监听；
- 监听子目录时 `path` 带该目录前缀。

### 测试

- `watch::create`：新建上报 `create`。
- `watch::update`：改动上报 `update`。
- `watch::remove`：删除上报 `remove`。
- `watch::recursive`：`recursive` 为真时上报子树里的变更。
- `watch::subdirectory`：监听子目录时 `path` 带目录前缀。

## `PUT ?type=content`

写入文件内容，以 JSON 承载。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |
| `content` | string | 必填，写入的内容 |

输出：无内容（`204`）。

- 文本写入留在 `/fs`，字节上传走 `/tus`；
- 覆盖已存在的文件。

### 测试

- `content::writes`：写入并覆盖已存在的文件，成功回 `204`。

## `PUT ?type=directory`

创建目录。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目录路径 |
| `recursive` | boolean \| null | 可选，是否创建缺失的父目录，缺省否 |

输出：无内容（`204`）。

- 缺父目录且未 `recursive` 时报错。

### 测试

- `directory::creates`：创建目录，成功回 `204`。
- `directory::recursive`：`recursive` 为真时创建缺失的父目录。

## `PUT ?type=symlink`

创建符号链接。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，链接自身的路径 |
| `target` | string | 必填，链接指向的路径 |

输出：无内容（`204`）。

- `path` 已存在时报 `409`；`target` 不必存在。

### 测试

- `symlink::creates`：创建链接后 `QUERY ?type=metadata` 报 `kind` 为 `symlink`。
- `symlink::exists`：`path` 已存在时返回 `409`。

## `PATCH ?type=metadata`

改写目标的元数据。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标路径 |
| `mode` | string \| null | 可选，权限位，八进制 |
| `uid` | integer \| null | 可选，属主 |
| `gid` | integer \| null | 可选，属组 |
| `atime` | string \| null | 可选，访问时间 |
| `mtime` | string \| null | 可选，修改时间 |

输出：无内容（`204`）。

- 只改写给出的字段，其余不动；
- 改写的是 `path` 指向的文件：符号链接被跟随。

### 测试

- `metadata::patched`：改写 `mode` 后 `QUERY ?type=metadata` 反映新值。
- `metadata::follows_symlink`：改写符号链接的元数据，改的是它指向的文件。

## `PATCH ?type=patch`

对文件应用文本补丁。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |
| `format` | string | 必填，补丁格式，目前只取 `unified` |
| `patch` | string | 必填，补丁文本 |

输出：无内容（`204`）。

### 测试

- `patch::applies`：应用 unified 补丁后文件内容更新。

## `PATCH ?type=truncate`

截断文件。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标文件路径 |
| `length` | integer \| null | 可选，目标字节数，缺省 0 |

输出：无内容（`204`）。

- `length` 大于当前长度时同样静默成功：文件被扩展，新增部分为零字节。

### 测试

- `truncate::extends`：`length` 大于当前长度时静默成功，`size` 变为 `length`。

## `POST ?type=copy`

复制。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，源路径 |
| `destination` | string | 必填，目标路径 |

输出：无内容（`204`）。

- 目标已存在时覆盖；
- 源是目录时整棵子树被复制，合并进已存在的目标目录，符号链接按目标复制。

### 测试

- `copy::copies`：复制到新目标，目标已存在时覆盖。
- `copy::directory`：复制目录后整棵子树出现在目标下。

## `POST ?type=move`

移动或重命名。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，源路径 |
| `destination` | string | 必填，目标路径 |

输出：无内容（`204`）。

- 目标已存在时覆盖；同一目录内即重命名；
- 跨设备时不做复制兜底，失败。

### 测试

- `move::renames`：移动后源路径不存在，目标可取。

## `DELETE`

删除目标。

输入（JSON body）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 必填，目标路径 |
| `recursive` | boolean \| null | 可选，是否删除整棵子树，缺省否 |
| `force` | boolean \| null | 可选，目标不存在时是否也算成功，缺省否 |

输出：无内容（`204`）。

- 目录非空且未 `recursive` 时报 `409`。

### 测试

- `delete::removes`：删除文件后回 `204`。
- `delete::recursive`：`recursive` 为真时删除整棵子树。
- `delete::non_empty`：目录非空且未 `recursive` 时返回 `409`。
