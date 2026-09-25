# FileSystem 设计

借鉴 WebDAV 的资源操作与条件请求，但不采用其扩展方法（`PROPFIND`、`PROPPATCH`、`MKCOL`、`COPY`、`MOVE`、`LOCK`）。

## HTTP/2

协议基于 HTTP/2，利用多路复用降低 agent 连续访问文件时的连接开销。

## 控制面和数据面分离

- 端点为固定地址，目标路径由请求 body 的 `path` 字段给出，不出现在 URL 中；
- 控制面用 JSON 承载目录查询、元数据与资源改动；
- 控制面读取一律用 `QUERY`（安全、幂等、可带请求体）加 `?type=<type>`；
- 数据面用裸字节承载文件内容：`QUERY ?type=stream` 流式下载，`PUT ?type=sink` 流式上传；控制面 `QUERY ?type=content` 只读取文本文件，二进制走 `QUERY ?type=stream`；
- 文件内容写入用 `PUT ?type=file`，`content` 为 UTF-8 文本，按原样写入；其余改动用 `POST`、`PATCH` 或带 `type` 的 `PUT`；
- 每个 `type` 的输入输出 schema 各自独立。

## HTTP 端点设计

端点固定，操作由方法与 `type` 共同决定；所有参数都必须通过 body 来传递与 `type` 对应 schema 的 JSON 对象来传入。

| 方法     | 语义                                          |
| -------- | --------------------------------------------- |
| `QUERY`  | 控制面读取，必须带 `type`                     |
| `PUT`    | 创建或替换资源，`type` 区分资源种类           |
| `PATCH`  | 修改已有资源（元数据或内容），必须带 `type`   |
| `POST`   | 作用于资源的动作，必须带 `type`               |
| `DELETE` | 删除资源，`recursive` 控制递归、`force` 控制忽略不存在 |

- `type` 放在 query；body 统一为 JSON 对象且至少含 `path`，其余字段为该 `type` 的 schema；
- 带分页的读取统一用 `offset` + `limit` 组合；
- 条件用标准头 `If-Match`、`If-None-Match`；复制/移动的目标条件另用扩展头 `Destination-If-Match`、`Destination-If-None-Match`，见“ETag 版本机制”；
- WebDAV 的 `Depth`、`Destination`、`Overwrite` 等头不再使用；`depth`、`destination` 等改由该 `type` 的 body schema 承载，目标版本条件见“复制/移动的双资源条件”。

### 端点总表

`{base}` 取 `/workspaces/{id}/fs` 或 `/fs`，见“两种寻址模式”。

| 方法     | `type`      | 操作          | body（关键字段）                                  |
| -------- | ----------- | ------------- | ------------------------------------------------- |
| `QUERY`  | `content`   | 读取文本文件  | `path`                                            |
| `QUERY`  | `stream`    | 流式下载      | `path`                                            |
| `QUERY`  | `metadata`  | 查询单个资源  | `path`                                            |
| `QUERY`  | `list`      | 列目录        | `path`、`depth`、`offset`、`limit`                |
| `QUERY`  | `glob`      | 通配匹配      | `path`、`pattern`、`exclude`、`offset`、`limit`   |
| `QUERY`  | `realpath`  | 解析真实路径  | `path`                                            |
| `QUERY`  | `access`    | 探测访问权限  | `path`                                            |
| `QUERY`  | `lines`     | 按行读取文件  | `path`、`offset`、`limit`                         |
| `QUERY`  | `watch`     | 监听变更      | `path`、`recursive`                               |
| `PUT`    | `file`      | 写入文件      | `path`、`content`                                 |
| `PUT`    | `sink`      | 流式上传文件  | 待定                                              |
| `PUT`    | `directory` | 创建目录      | `path`、`recursive`                               |
| `PUT`    | `symlink`   | 创建符号链接  | `path`、`target`                                  |
| `PATCH`  | `metadata`  | 修改元数据    | `path`、可变字段                                  |
| `PATCH`  | `patch`     | 应用文本补丁  | `path`、`format`、`patch`                         |
| `PATCH`  | `truncate`  | 截断文件      | `path`、`length`                                  |
| `POST`   | `copy`      | 复制          | `path`、`destination`                             |
| `POST`   | `move`      | 移动 / 重命名 | `path`、`destination`                             |
| `DELETE` | —           | 删除          | `path`、`recursive`、`force`                      |

`type` 缺失或不受支持返回 `422 unsupported_type`；`type` 与所用方法不匹配（如 `PUT ?type=metadata`）返回 `405 method_not_allowed`；操作要求的资源类型与目标不符（对目录 `QUERY ?type=content`、`QUERY ?type=stream`、`QUERY ?type=lines`，对文件 `QUERY ?type=list`、`QUERY ?type=glob`）返回 `400 not_a_file` / `400 not_a_directory`；`path` 缺失或类型不符返回 `422 invalid_request`。

### 读取类型

#### `QUERY ?type=content`

读取目标文本文件内容，以 JSON 承载。

输入（JSON body）：

| 字段   | 类型   | 说明               |
| ------ | ------ | ------------------ |
| `path` | string | 必填，目标文件路径 |

输出：

| 字段      | 类型    | 说明                       |
| --------- | ------- | -------------------------- |
| `path`    | string  | 目标路径                   |
| `content` | string  | UTF-8 解码后的文件内容     |
| `size`    | integer | 原始字节数                 |
| `etag`    | string  | 当前版本                   |

- 只读取文本文件：内容不是合法 UTF-8 返回 `422 invalid_request`，二进制文件改用 `QUERY ?type=stream`；
- 整份内容读入内存，大文件用 `QUERY ?type=stream`；
- 目标为目录返回 `400 not_a_file`；资源不存在返回 `404`；
- 成功响应在 `ETag` 头返回当前版本，与 JSON 的 `etag` 相同。

#### `QUERY ?type=stream`

以裸字节响应流式发送目标文件内容。

输入（JSON body）：`path`。

响应头：

| 头部             | 说明                                              |
| ---------------- | ------------------------------------------------- |
| `Content-Type`   | 文件类型；无法检测时为 `application/octet-stream` |
| `Content-Length` | 文件字节数                                        |
| `ETag`           | 当前文件版本                                      |
| `Last-Modified`  | 文件系统修改时间                                  |
| `Accept-Ranges`  | `none`                                            |

- 在开始发送响应体前完成路径、权限、文件类型、ETag 与大小校验；
- 使用固定大小缓冲区读取，客户端断开后关闭文件；I/O 错误终止响应；
- 目标为目录返回 `400 not_a_file`；资源不存在返回 `404`。

#### `QUERY ?type=metadata`

返回目标资源自身元数据；目标为工作区根目录时 `path` 为空字符串。

输入（JSON body）：`path`。

输出 `ResourceMetadata`：

| 字段          | 类型                             | 说明                                               |
| ------------- | -------------------------------- | -------------------------------------------------- |
| `name`        | string                           | 资源名称                                           |
| `path`        | string                           | 当前寻址模式下的路径：工作区相对路径或远程绝对路径 |
| `kind`        | `file` / `directory` / `symlink` | 资源类型                                           |
| `size`        | integer                          | 字节数，目录为 `0`                                 |
| `etag`        | string                           | 版本标识                                           |
| `mode`        | string                           | 权限位，八进制，如 `"0644"`                        |
| `uid`         | integer                          | 所有者用户 ID                                      |
| `gid`         | integer                          | 所有者组 ID                                        |
| `inode`       | integer                          | inode 编号                                         |
| `links`       | integer                          | 硬链接数                                           |
| `device`      | integer                          | 资源所在设备的编号                                 |
| `device_type` | integer                          | 特殊文件指向的设备编号                             |
| `block_size`  | integer                          | 文件系统块大小                                     |
| `blocks`      | integer                          | 占用的块数                                         |
| `modified_at` | string（RFC 3339）               | 修改时间                                           |
| `accessed_at` | string（RFC 3339）               | 访问时间                                           |
| `birthtime`   | string（RFC 3339）               | 创建时间                                           |
| `target`      | string                           | 仅 `kind=symlink` 出现，链接目标                   |

平台无法提供的属性省略，如不支持创建时间的文件系统不返回 `birthtime`。

资源不存在返回 `404`。

#### `QUERY ?type=list`

列出目录成员。

输入（JSON body）：

| 字段     | 类型       | 说明                                           |
| -------- | ---------- | ---------------------------------------------- |
| `path`   | string     | 必填，目标目录路径，空字符串表示工作区根目录   |
| `depth`  | `infinity` | 不带时只列目标目录的直接子项，带该值时递归子项 |
| `offset` | integer    | 起始条目序号，不带时从首条开始                 |
| `limit`  | integer    | 单次返回的条目上限，不带时用服务端上限         |

输出：

| 字段        | 类型              | 说明                     |
| ----------- | ----------------- | ------------------------ |
| `entries`   | `ResourceEntry[]` | 目录条目，按遍历顺序切分 |
| `truncated` | boolean           | 还有未返回的条目         |

`ResourceEntry` 只含 `name`、`path`、`kind`、`size`、`etag`、`modified_at`，字段含义同 `metadata`。

- 条目按目录遍历顺序返回，`offset` 按该顺序切分；
- 递归查询还受服务端总条目数或响应大小上限约束，达到即截断并置 `truncated`；
- `depth` 只接受 `infinity`，其它值返回 `422 invalid_request`；`limit` 为 `0` 返回 `400 bad_request`；
- 目标是文件（包括指向文件的符号链接）时返回 `400 not_a_directory`；
- 未来可加入按名称稳定排序的有序集合，参考 [RFC 3648](https://datatracker.ietf.org/doc/html/rfc3648)。

#### `QUERY ?type=lines`

按行读取文本文件的指定范围。

输入（JSON body）：

| 字段     | 类型    | 说明                                      |
| -------- | ------- | ----------------------------------------- |
| `path`   | string  | 必填，目标文件路径                        |
| `offset` | integer | 起始行序号，从 `0` 开始；不带时从首行开始 |
| `limit`  | integer | 最多返回的行数；不带时使用服务端上限      |

输出：

```json
{
  "lines": ["first line", "second line"],
  "offset": 10,
  "truncated": true
}
```

- `lines` 按文件中的逻辑行顺序返回，不包含行尾的 `\\n` 或 `\\r\\n`；空行作为空字符串返回；
- `offset` 表示首个返回行的零基序号；请求超出文件末尾时返回空数组，不视为错误；
- 文件末尾没有换行符时，末尾内容仍作为一行返回；
- `truncated` 表示服务端因 `limit` 或响应大小上限未返回全部后续行；没有后续行时为 `false`；
- 默认按 UTF-8 解码；内容不是合法 UTF-8 时返回 `422 invalid_request`；
- `offset` 必须为非负整数，否则返回 `422 invalid_request`；`limit` 必须为正整数，为 `0` 返回 `400 bad_request`；
- 目标是目录时返回 `400 not_a_file`。

#### `QUERY ?type=glob`

在目标目录子树内按模式定位资源。

输入（JSON body）：

| 字段      | 类型     | 说明                                   |
| --------- | -------- | -------------------------------------- |
| `path`    | string   | 必填，搜索根目录路径                   |
| `pattern` | string   | 必填，相对搜索根目录的匹配模式         |
| `exclude` | string[] | 可选，命中即排除的附加模式，可重复出现 |
| `offset`  | integer  | 起始条目序号，不带时从首条开始         |
| `limit`   | integer  | 单次返回的条目上限，不带时用服务端上限 |

`pattern` 与 `exclude` 用标准 glob 语法，相对 `path`、整串匹配。

输出与 `QUERY ?type=list` 相同。

- 目标目录本身不作为命中条目；文件、目录与符号链接都可命中，但遍历不跟随目录符号链接；
- 命中 `exclude` 的目录连同其子树跳过，其它条目剔除；
- 结果按遍历顺序返回，`offset` 按该顺序切分，达到服务端上限即截断并置 `truncated`；
- `pattern` 缺失返回 `422 invalid_request`，非法返回 `400 bad_request`；目标是文件返回 `400 not_a_directory`。

#### `QUERY ?type=realpath`

把路径解析为最终真实路径，展开符号链接并归一化 `.`、`..`。

输入（JSON body）：`path`。

输出：

| 字段   | 类型   | 说明                     |
| ------ | ------ | ------------------------ |
| `path` | string | 解析后的真实绝对路径     |

- 逐级展开路径中的全部符号链接，返回规范化后的真实绝对路径，与寻址模式无关；
- 目标不存在（含中间路径不存在或悬空链接）返回 `404`。

#### `QUERY ?type=access`

探测服务进程对目标的可访问性。

输入（JSON body）：`path`。

输出：

| 字段         | 类型    | 说明                                   |
| ------------ | ------- | -------------------------------------- |
| `readable`   | boolean | 服务进程是否可读目标                   |
| `writable`   | boolean | 服务进程是否可写目标                   |
| `executable` | boolean | 服务进程是否可执行目标（目录即可遍历） |

- 跟随符号链接探测其目标；目标不存在（含悬空链接）返回 `404`；
- 工作区模式下 `writable` 还反映工作区 `read-only` 属性，只读工作区返回 `false`（见 [Workspace.md](./Workspace.md)）。

#### `QUERY ?type=watch`

监听目标资源的变更并持续推送事件，目标可为文件或目录。

输入（JSON body）：

| 字段        | 类型    | 说明                                             |
| ----------- | ------- | ------------------------------------------------ |
| `path`      | string  | 必填，监听目标路径                               |
| `recursive` | boolean | 为 `true` 时递归监听子目录；不带时只监听直接子项 |

输出为换行分隔的 JSON（`Content-Type: application/x-ndjson`），每行一个 `WatchEvent`，随变更逐条产生：

| 字段    | 类型                           | 说明                                                 |
| ------- | ------------------------------ | ---------------------------------------------------- |
| `event` | `create` / `update` / `remove` | 变更种类                                             |
| `path`  | string                         | 变更资源的路径，寻址模式与 `metadata` 的 `path` 一致 |

- `create` 表示新资源出现，`update` 表示已有资源的内容或元数据变化，`remove` 表示资源消失；重命名表现为旧路径 `remove` 与目标路径 `create`；
- 只报告监听建立之后的变更，不含初始快照；初始状态先用 `QUERY ?type=list` 或 `QUERY ?type=metadata` 获取；
- 事件只携带路径，消费方按路径重新读取当前状态即可，重复事件按同一路径幂等处理；
- 目标为符号链接时解析到其真实目标，递归监听不跟随目录符号链接（同 `QUERY ?type=glob`）；
- 事件按发生顺序推送；单个连接的发送缓冲达到上限时终止该流，消费方重连后重新同步；
- 目标不存在返回 `404`；`recursive` 非布尔返回 `422 invalid_request`。

### 变更类型

#### `PUT ?type=file`

写入文件内容。

```json
{ "path": "src/a.txt", "content": "hello\n" }
```

| 字段      | 类型   | 说明                 |
| --------- | ------ | -------------------- |
| `path`    | string | 必填，目标文件路径   |
| `content` | string | 必填，UTF-8 文本内容 |

- `content` 为 UTF-8 文本，按原样作为文件字节写入；缺失返回 `422 invalid_request`；二进制内容走数据面 `PUT ?type=sink`；
- 目标不存在时创建，必须带 `If-None-Match: *`；目标已存在时替换，必须带匹配当前版本的 `If-Match`；
- 成功创建返回 `201`，替换返回 `200`，都返回更新后的 `metadata`，并在 `ETag` 头返回新版本；
- 服务端先写入临时文件，再在写闸门内提交，见“两种文件写入模式”；目标为目录返回 `400 not_a_file`。

#### `PUT ?type=directory`

创建目录，对应 WebDAV `MKCOL`。

```json
{ "path": "src", "recursive": true }
```

- 默认只创建末级目录；`recursive=true` 时递归创建缺失的父目录；
- 创建必须带 `If-None-Match: *`；目标已存在返回 `412`；
- 成功返回 `201` 与创建的 `metadata`。

#### `PUT ?type=symlink`

在 `path` 位置创建符号链接，链接目标由 body 给出。

```json
{ "path": "link", "target": "../b.txt" }
```

| 字段     | 类型   | 说明                     |
| -------- | ------ | ------------------------ |
| `path`   | string | 必填，链接所在路径       |
| `target` | string | 必填，链接目标，允许悬空 |

- 创建 `kind=symlink` 资源，`target` 原样保存，不要求目标存在；
- 相对 `target` 相对链接所在目录解析，绝对 `target` 按自身解析，工作区模式下解析结果必须位于工作区根目录内（见“工作区模式”），越界返回 `400 bad_request`；
- 必须带 `If-None-Match: *`；`path` 已存在返回 `412`；
- 成功返回 `201` 与创建的 `metadata`，并在 `ETag` 头返回新版本。

#### `PATCH ?type=metadata`

修改已有资源的可变元数据。

```json
{ "path": "src/a.txt", "mode": "0644", "modified_at": "2026-01-01T00:00:00Z" }
```

- body 在 `path` 之外只接受可变字段，如 `mode`（权限）与 `modified_at`（修改时间）；
- 出现不可变字段（如 `kind`、`size`、`etag`）返回 `422 invalid_request`；
- 必须带 `If-Match`，成功返回更新后的 `metadata`。

#### `PATCH ?type=patch`

对已有文本文件应用补丁，避免整文件重传，只需提交差异部分。

```json
{
  "path": "a.txt",
  "format": "unified",
  "patch": "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
}
```

| 字段     | 类型   | 说明                           |
| -------- | ------ | ------------------------------ |
| `path`   | string | 必填，目标文件路径             |
| `format` | string | 补丁种类，目前仅支持 `unified` |
| `patch`  | string | 按该种类编码的补丁文本         |

- 补丁种类由 body 的 `format` 声明，目前只接受 `unified`，即标准 unified diff format；`format` 缺失或不受支持返回 `422 unsupported_type`，`patch` 缺失或类型不符返回 `422 invalid_request`；
- 补丁中的文件头路径仅作说明，不参与寻址；目标始终由 `path` 决定，只有 hunk 内容应用到目标文件；
- 目标必须为文本文件：目录返回 `400 not_a_file`，无法按文本解码的文件返回 `422 invalid_request`；
- 补丁必须干净地应用到当前内容，上下文不匹配等无法应用的情况返回 `409 patch_conflict`，且不做任何修改；应用是原子的，要么整体成功，要么目标保持原样；
- 版本条件与其它变更一致：必须带匹配当前版本的 `If-Match`，成功返回更新后的 `metadata`，并在 `ETag` 头返回新版本。

#### `PATCH ?type=truncate`

把已有文件内容调整到指定长度。

```json
{ "path": "a.txt", "length": 1024 }
```

| 字段     | 类型    | 说明               |
| -------- | ------- | ------------------ |
| `path`   | string  | 必填，目标文件路径 |
| `length` | integer | 必填，非负目标长度 |

- 短于当前长度则截断，长于当前长度以零字节补齐，等于当前长度不改变内容；
- 目标必须为文件：目录返回 `400 not_a_file`；
- 必须带匹配当前版本的 `If-Match`，成功返回 `200` 与更新后的 `metadata`，并在 `ETag` 头返回新版本。

#### `POST ?type=copy`

把资源复制到 `destination`，源保持不变。

```json
{ "path": "src/a.txt", "destination": "src/b.txt" }
```

- `path` 为源路径，`destination` 为目标路径，二者同属一种寻址模式：工作区相对路径或远程绝对路径；
- 源必须提供匹配当前版本的 `If-Match`；目标条件用扩展头承载，见“复制/移动的双资源条件”；
- `destination` 越界或位于源目录子树内返回 `400`；
- 成功创建目标返回 `201`，覆盖已有目标返回 `204`，响应带结果的 `ETag`。

#### `POST ?type=move`

移动资源到 `destination`，成功后源路径不再存在（先复制到目标再删除源）；其余同 `POST ?type=copy`。

#### `DELETE`

```json
{ "path": "src", "recursive": true, "force": false }
```

| 字段        | 类型    | 说明                                                 |
| ----------- | ------- | ---------------------------------------------------- |
| `path`      | string  | 必填，目标路径                                       |
| `recursive` | boolean | 为 `true` 时递归删除目录及其整个子树；缺省为 `false` |
| `force`     | boolean | 为 `true` 时目标不存在按成功处理；缺省为 `false`     |

- 目标为文件或符号链接时直接删除，不跟随符号链接的目标，`recursive` 不适用；
- 目标为目录且 `recursive` 不为 `true` 返回 `400 not_a_file`；为 `true` 时删除目录及其整个子树；
- 必须提供匹配当前版本的 `If-Match`；`force=true` 且目标不存在时无当前版本可比，省略 `If-Match` 直接返回 `204`；
- 目标不存在且 `force` 不为 `true` 返回 `404`；`recursive`、`force` 非布尔返回 `422 invalid_request`；
- 成功返回 `204`，无响应体。

## WebDAV 覆盖对照

| WebDAV            | 本设计                                                |
| ----------------- | ----------------------------------------------------- |
| `GET` / `HEAD`    | `QUERY ?type=content`、`QUERY ?type=stream`、`QUERY ?type=metadata` |
| `PUT`             | `PUT ?type=file`                                      |
| `MKCOL`           | `PUT ?type=directory`                                 |
| `PROPFIND`        | `QUERY ?type=metadata`、`QUERY ?type=list`            |
| `PROPPATCH`       | `PATCH ?type=metadata`                                |
| `COPY`            | `POST ?type=copy`                                     |
| `MOVE`            | `POST ?type=move`                                     |
| `DELETE`          | `DELETE`                                              |
| `LOCK` / `UNLOCK` | 不采用，并发控制由 ETag 条件请求承担                  |
| `OPTIONS`         | 仅上传端点使用                                        |

## 两种寻址模式

文件资源支持两种寻址模式，它们共享同一套资源操作、条件请求和错误语义，差异只在端点和 `path` 语义：

| 模式         | 端点                    | `path` 语义      | 边界         |
| ------------ | ----------------------- | ---------------- | ------------ |
| 工作区模式   | `/workspaces/{id}/fs`   | 工作区内相对路径 | 工作区根目录 |
| 绝对路径模式 | `/fs`                   | 远程绝对路径     | 无           |

### 工作区模式

访问文件前，先把一个远程绝对路径通过工作区端点注册为工作区，并指定唯一的 `id`（见 [Workspace.md](./Workspace.md)）；该注册是协议中唯一接受远程绝对路径的控制面边界。

后续请求在 URL 中指定工作区 ID；`path` 参数则必须为工作区内相对路径。
传入的相对路径先经过规范化，再校验其位于工作区根目录内，越界返回 `400 bad_request`。变更操作的边界只针对请求给出的 `path` 地址，不追踪其上的符号链接去校验最终落点：只要被操作的地址在工作区内即可，该地址经符号链接指向根目录外时写入按其目标进行，不视为越界。
工作区是路径访问的边界，服务端还必须校验工作区存在、具备相应权限；变更操作还受该工作区属性约束，见 [Workspace.md](./Workspace.md) 的工作区属性。

### 绝对路径模式

请求在 `path` 字段中直接携带远程绝对路径，用于尚未或不需要注册为工作区的目录。传入的绝对路径同样先经过 canonicalization。

工作区级别的权限与版本管理仅在工作区模式下提供。

## ETag 版本机制

ETag 是服务端生成的不透明**强验证器**。文件、目录和符号链接都有 ETag；客户端保存并原样回传服务端返回的值。

并发前提：受保护资源由本服务的文件 API 统一写入。`exec`、`shell`、`pty` 或其它进程需要与文件 API 共享写入时，由调用方负责串行化，并在写入后重新查询 ETag。

### 版本的管理

- 服务进程维护进程内资源版本表，键为路径校验后的服务端绝对路径；工作区路径和绝对路径共享同一版本。
- 版本表由文件 API 的读写闸门统一访问。首次观察资源时分配版本；每次成功变更分配新版本。ETag 使用进程随机前缀和单调序号生成，按 HTTP entity-tag 语法带双引号返回。
- 进程启动时创建新的随机前缀，资源首次观察时生成新 ETag；因此版本表无需持久化。
- 创建、替换、修改元数据、删除、复制或移动资源时，递增受影响资源及其所有祖先目录的版本。目录 ETag 表示该目录及其子树的服务端变更版本，可保护递归列表和递归删除。
- 每次成功写操作都递增版本，包括新旧内容相同的写操作。

### 条件请求

涉及已有资源的变更必须使用强 `If-Match`。创建使用 `If-None-Match: *`；`If-None-Match` 在本 API 中只支持这个创建语义。

| 操作                     | 条件语义                                            |
| ------------------------ | --------------------------------------------------- |
| 创建文件、目录或符号链接 | 必须提供 `If-None-Match: *`，确保目标不存在         |
| 覆盖文件                 | 必须提供匹配当前版本的 `If-Match`                   |
| 修改元数据               | 必须提供匹配当前版本的 `If-Match`                   |
| 应用补丁                 | 必须提供匹配当前版本的 `If-Match`                   |
| 截断文件                 | 必须提供匹配当前版本的 `If-Match`                   |
| 删除文件或目录           | 必须提供匹配当前版本的 `If-Match`，`force=true` 且目标不存在时除外 |
| 移动                     | 源必须提供匹配当前版本的 `If-Match`                 |
| 复制                     | 源必须提供匹配当前版本的 `If-Match`                 |
| 复制/移动的目标          | 目标条件由扩展头承载，见“复制/移动的双资源条件”     |

- `If-Match` 按强比较解析标准 entity-tag 列表；列表中任一值匹配当前 ETag 即满足条件。变更请求使用具体 ETag，`If-Match: *` 保留给需要“资源存在”判断的通用 HTTP 语义。
- 缺少必要条件返回 `428 Precondition Required`；条件格式非法返回 `400 bad_request`；资源不存在返回 `404`；条件不满足返回 `412 etag_mismatch`。
- `If-Match` 与 `If-None-Match` 同时出现时返回 `400 invalid_request`。创建请求使用 `If-None-Match: *`，覆盖请求使用 `If-Match`。
- 服务端在同一个文件 API 写闸门内完成资源解析、条件检查、文件系统提交和版本递增；请求体可先写入临时文件，最终提交时重新检查目标条件。
- 成功响应在 HTTP `ETag` 头返回新版本；`QUERY ?type=content`、`QUERY ?type=stream`、`QUERY ?type=metadata` 和 `QUERY ?type=list` 返回当前资源或列表根目录的 ETag 头。JSON 中的 `etag` 与响应头使用同一值。
- ETag 用于并发控制；`Last-Modified` 表示文件系统修改时间。

### 复制/移动的双资源条件

一个复制/移动请求同时涉及源和目标，单个 `If-Match` 头只表示源条件。目标条件用扩展头表达：

| 头部                        | 语义                                               |
| --------------------------- | -------------------------------------------------- |
| `Destination-If-None-Match` | 只接受 `*`，目标必须不存在，已存在返回 `412`       |
| `Destination-If-Match`      | 目标已存在时须精确匹配其当前 ETag，不存在时条件成立 |

```http
POST /workspaces/{id}/fs?type=copy
If-Match: "source-tag"
Destination-If-Match: "destination-tag"
```

- 两个目标条件头必选其一：都不带返回 `428 precondition_required`，同时出现返回 `400 invalid_request`；
- `Destination-If-Match` 匹配失败返回 `412 etag_mismatch`；
- 目标检查、源检查和实际复制/移动在同一写闸门内进行。成功响应的 `ETag` 是目标的新 ETag；移动成功后源路径进入不存在状态。
- 复制或移动目录时，目标子树中的路径分配新 ETag；源子树的旧路径版本失效，相关祖先目录版本递增。

### 乐观锁流程

ETag 代表一次提交的乐观锁版本。客户端读取资源并保存 ETag，提交变更时回传该值；服务端在写闸门内校验并提交，成功后生成新 ETag。多个客户端可以并行读取，同一版本只有首个提交成功；后续客户端收到 `412` 后重新读取资源、重新计算修改并用新 ETag 重试。所有文件 API 写入均遵循该流程即可避免丢失更新。

## 两种文件写入模式

### 小文件直接写入

小文件通过一次 `PUT ?type=file` 提交。服务端把 `content` 写入临时文件；请求体接收完成后，在写闸门内重新检查目标条件，再以原子替换提交并递增版本。请求失败或取消时清理临时文件。

### 大文件上传

大文件走 [tus 1.0.0](https://tus.io/protocols/resumable-upload) 核心协议，并启用 `creation`、`termination`、`concatenation` 扩展。tus 的方法、请求头、状态码与 `OPTIONS` 能力集合全部按该协议；上传端点不适用本文的 `type` 与错误码。

上传是 FileSystem 的子路由，与 `/fs` 同级：

| 端点                        | 方法      |
| --------------------------- | --------- |
| `{upload-base}`             | `OPTIONS` |
| `{upload-base}`             | `POST`    |
| `{upload-base}/{upload_id}` | `HEAD`    |
| `{upload-base}/{upload_id}` | `PATCH`   |
| `{upload-base}/{upload_id}` | `DELETE`  |

`{upload-base}` 在工作区模式为 `/workspaces/{id}/upload`，绝对路径模式为 `/upload`。

上传只负责把字节汇聚成完整文件；完整文件通过与小文件写入相同的写闸门提交到目标 FileSystem 路径：

- 目标 `path` 与提交条件随创建请求给出，承载方式见“待定”；
- 提交在写闸门内完成条件检查、原子替换与版本递增，成功即目标路径获得新 ETag；
- `partial` 上传只是暂存分块，不构成 FileSystem 资源；`final` 拼装完成时按 `Upload-Concat` 的顺序合成整份文件并一次性提交；
- 提交目标的条件语义与其它变更相同（见“条件请求”），并在提交时于写闸门内校验，以覆盖分块上传过程中目标被其它写者改变的情况；
- 目标路径、工作区属性约束沿用既有规则（见“工作区模式”、[Workspace.md](./Workspace.md) 的工作区属性）。

#### 待定

- 目标 `path` 与提交条件在创建请求中的承载方式（`Upload-Metadata`）；
- 上传会话的状态存放、暂存位置与清理策略（`Upload-Expires` 或 `termination`）；
- `Tus-Max-Size` 取值与单次 `PATCH` 分块大小约束；
- 是否启用 `creation-with-upload`、`checksum`。

## 错误协议

错误协议首先参考 HTTP、WebDAV 等成熟协议，只增加必要的特定错误。

错误响应使用统一 JSON 结构：

```json
{
  "error": {
    "code": "etag_mismatch",
    "message": "resource has changed",
    "request_id": "req_123"
  }
}
```

HTTP 状态码表达通用语义，`error.code` 提供稳定的机器可读分类；`message` 仅供阅读和日志。

| 状态  | `error.code`            | 场景                                       |
| ----- | ----------------------- | ------------------------------------------ |
| `400` | `bad_request`           | `path`、参数或 `destination` 非法          |
| `400` | `not_a_file`            | 操作要求文件，目标是目录                   |
| `400` | `not_a_directory`       | 操作要求目录，目标是文件                   |
| `404` | `not_found`             | 资源不存在                                 |
| `405` | `method_not_allowed`    | 方法与 `type` 组合不适用                   |
| `409` | `patch_conflict`        | 补丁无法应用到当前内容                     |
| `412` | `etag_mismatch`         | ETag 条件不满足                            |
| `428` | `precondition_required` | 缺少操作要求的条件请求头                   |
| `422` | `invalid_request`       | body 不符合该 `type` 的 schema，如缺 `path` |
| `422` | `unsupported_type`      | `type` 缺失或不支持                        |

工作区相关错误码见 [Workspace.md](./Workspace.md)，如只读工作区返回 `403 read_only_workspace`。
