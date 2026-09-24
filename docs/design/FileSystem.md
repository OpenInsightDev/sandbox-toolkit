# FileSystem 设计

借鉴 WebDAV 的资源寻址与条件操作，但不采用其扩展方法（`PROPFIND`、`PROPPATCH`、`MKCOL`、`COPY`、`MOVE`、`LOCK`）。

## HTTP/2

协议基于 HTTP/2，利用多路复用降低 agent 连续访问文件时的连接开销。

## 控制面和数据面分离

- 数据面用标准方法承载原始 body：`GET` 下载、`PUT` 写入、`HEAD` 探测，元数据经 `Content-Type`、`Content-Length`、`ETag`、`Last-Modified` 等响应头携带；
- 控制面用 JSON 承载目录查询、元数据与资源改动；
- 控制面读取一律用 `QUERY`（安全、幂等、可带请求体）加 `?type=<type>`；
- 改动中文件内容用裸 `PUT` 写入，其余改动用 `POST`、`PATCH` 或带 `type` 的 `PUT`；
- 每个 `type` 的输入输出 schema 各自独立。

## HTTP 端点设计

端点资源导向：`{base}` 之后是目标资源路径，操作由方法与 `type` 共同决定。

| 方法     | 语义                                          |
| -------- | --------------------------------------------- |
| `GET`    | 数据面读取文件内容（原始 body）               |
| `HEAD`   | 只返回响应头                                  |
| `QUERY`  | 控制面读取，必须带 `type`                     |
| `PUT`    | 在目标路径创建或替换资源，`type` 区分资源种类 |
| `PATCH`  | 修改已有资源的元数据，必须带 `type`           |
| `POST`   | 作用于目标资源的动作，必须带 `type`           |
| `DELETE` | 删除资源，目录递归删除                        |

- `type` 放在 query；简单标量参数（如 `depth`）也在 query；结构化参数放 JSON body；
- 带分页的读取统一用 `offset` + `limit` 组合；
- 条件仍用标准头 `If-Match`、`If-None-Match`，见“ETag 版本机制”；
- WebDAV 的 `Depth`、`Destination`、`Overwrite` 等头不再使用，改由 query 或该 `type` 的 body schema 承载。

### 端点总表

`{base}` 取 `/workspaces/{id}/fs` 或 `/fs`，见“两种寻址模式”。

| 端点            | 方法     | `type`      | 操作          |
| --------------- | -------- | ----------- | ------------- |
| `{base}/{path}` | `GET`    | —           | 下载文件      |
| `{base}/{path}` | `HEAD`   | —           | 探测资源      |
| `{base}/{path}` | `QUERY`  | `metadata`  | 查询单个资源  |
| `{base}/{path}` | `QUERY`  | `list`      | 列目录        |
| `{base}/{path}` | `PUT`    | —           | 写入文件      |
| `{base}/{path}` | `PUT`    | `directory` | 创建目录      |
| `{base}/{path}` | `PATCH`  | `metadata`  | 修改元数据    |
| `{base}/{path}` | `POST`   | `copy`      | 复制          |
| `{base}/{path}` | `POST`   | `move`      | 移动 / 重命名 |
| `{base}/{path}` | `DELETE` | —           | 删除          |

`QUERY`、`PATCH`、`POST` 缺少 `type` 返回 `422 unsupported_type`；`type` 与所用方法不匹配（如 `PUT ?type=metadata`）返回 `405 method_not_allowed`；操作要求的资源类型与目标不符（对目录 `GET`、对文件 `QUERY ?type=list`）返回 `400 not_a_file` / `400 not_a_directory`。

### 读取类型

#### `QUERY ?type=metadata`

返回目标资源自身元数据；目标为工作区根目录时路径为空，即直接请求 `{base}`。

输入：无，目标由路径寻址。

输出 `ResourceMetadata`：

| 字段          | 类型                             | 说明                                               |
| ------------- | -------------------------------- | -------------------------------------------------- |
| `name`        | string                           | 资源名称                                           |
| `path`        | string                           | 当前寻址模式下的路径：工作区相对路径或远程绝对路径 |
| `kind`        | `file` / `directory` / `symlink` | 资源类型                                           |
| `size`        | integer                          | 字节数，目录为 `0`                                 |
| `etag`        | string                           | 版本标识                                           |
| `modified_at` | string（RFC 3339）               | 修改时间                                           |
| `target`      | string                           | 仅 `kind=symlink` 出现，链接目标                   |

资源不存在返回 `404`。

#### `QUERY ?type=list`

列出目录成员。

输入（query）：

| 字段     | 类型       | 说明                                           |
| -------- | ---------- | ---------------------------------------------- |
| `depth`  | `infinity` | 不带时只列目标目录的直接子项，带该值时递归子项 |
| `offset` | integer    | 起始条目序号，不带时从首条开始                 |
| `limit`  | integer    | 单次返回的条目上限，不带时用服务端上限         |

输出：

| 字段        | 类型                 | 说明                                        |
| ----------- | -------------------- | ------------------------------------------- |
| `entries`   | `ResourceMetadata[]` | 目录条目，字段同 `metadata`，按稳定顺序切分 |
| `truncated` | boolean              | 还有未返回的条目                            |

- 返回顺序稳定，`offset` 按该顺序切分；
- 递归查询还受服务端总条目数或响应大小上限约束，达到即截断并置 `truncated`；
- 目标是文件（包括指向文件的符号链接）时返回 `400 not_a_directory`。

### 变更类型

#### `PUT`

写入文件内容，body 为原始字节，不带 `type`，见“两种文件写入模式”。

#### `PUT ?type=directory`

创建目录，对应 WebDAV `MKCOL`。

```json
{ "parents": true }
```

- body 可选；不带 `parents` 时只创建末级目录，`parents=true` 时递归创建缺失的父目录；
- 目标已存在返回 `412`。

#### `PATCH ?type=metadata`

修改已有资源的可变元数据。

```json
{ "mode": "0644", "modified_at": "2026-01-01T00:00:00Z" }
```

- body 只接受可变字段，如 `mode`（权限）与 `modified_at`（修改时间）；
- 出现不可变字段（如 `kind`、`size`、`etag`）返回 `422 invalid_request`；
- 必须带 `If-Match`，成功返回更新后的 `metadata`。

#### `POST ?type=copy` 与 `POST ?type=move`

```json
{ "destination": "src/b.txt" }
```

- `destination` 与源同属一种寻址模式：工作区相对路径或远程绝对路径；
- 源必须提供 `If-Match`；不带 `overwrite` 时目标必须不存在，否则 `412`；`overwrite=true` 时覆盖已有目标；
- 移动先复制再删除源；`destination` 越界或位于源目录子树内返回 `400`；
- 成功创建目标返回 `201`，覆盖已有目标返回 `204`，响应带结果的 `ETag`。

#### `DELETE`

删除文件或目录，目录递归删除；必须提供 `If-Match`，成功返回 `204`。

## WebDAV 覆盖对照

| WebDAV            | 本设计                                     |
| ----------------- | ------------------------------------------ |
| `GET` / `HEAD`    | `GET` / `HEAD`                             |
| `PUT`             | `PUT`                                      |
| `MKCOL`           | `PUT ?type=directory`                      |
| `PROPFIND`        | `QUERY ?type=metadata`、`QUERY ?type=list` |
| `PROPPATCH`       | `PATCH ?type=metadata`                     |
| `COPY` / `MOVE`   | `POST ?type=copy` / `POST ?type=move`      |
| `DELETE`          | `DELETE`                                   |
| `LOCK` / `UNLOCK` | 不采用，并发控制由 ETag 条件请求承担       |
| `OPTIONS`         | 不采用，能力由端点与 `type` 定义           |

## 两种寻址模式

文件资源支持两种寻址模式，它们共享同一套资源操作、条件请求和错误语义，差异只在寻址方式与边界：

| 模式         | 端点                             | 路径参数         | 边界         |
| ------------ | -------------------------------- | ---------------- | ------------ |
| 工作区模式   | `/workspaces/{id}/fs/{rel-path}` | 工作区内相对路径 | 工作区根目录 |
| 绝对路径模式 | `/fs/{abs-path}`                 | 远程绝对路径     | 无           |

### 工作区模式

访问文件前，先把一个远程绝对路径通过工作区端点注册为工作区，并指定唯一的 `id`（见 [Workspace.md](./Workspace.md)）；该注册是协议中唯一接受远程绝对路径的控制面边界。

后续请求使用工作区 ID + 工作区内相对路径。工作区是路径访问的边界，服务端必须校验工作区存在、具备相应权限，并且目标始终位于工作区根目录内；变更操作还受该工作区属性约束，见 [Workspace.md](./Workspace.md) 的工作区属性。

### 绝对路径模式

请求直接以远程绝对路径寻址，用于尚未或不需要注册为工作区的目录。

工作区级别的权限与版本管理仅在工作区模式下提供。

## 简化并严格的路径模型

两种模式都只接受规范化路径，路径错误不自动修正，返回稳定错误码，且都只进行一次明确的路径解码。

工作区相对路径：

- 使用 `/` 作为路径分隔符；
- 不允许以 `/` 开头；
- 不允许空路径作为文件操作目标（目录查询除外，空路径表示工作区根目录）；
- 不允许 `.`、`..`、NUL 字节及歧义编码；
- 读写操作都必须检查路径是否越过工作区边界；
- 对符号链接等可能越界的对象执行边界校验。

绝对路径：

- 必须是远程绝对路径，规范化后无 `.`、`..`、NUL 字节及歧义编码；
- URL 中 `/fs/` 之后的部分是绝对路径去掉前导 `/` 的写法，服务端补回前导 `/` 后再解析，例如 `/fs/srv/project/a.txt` 对应 `/srv/project/a.txt`。

## ETag 版本机制

ETag 是不透明的资源版本标识，文件和目录都有。涉及已有资源的覆盖、删除、移动和复制必须使用条件请求。

| 操作           | 条件语义                                       |
| -------------- | ---------------------------------------------- |
| 创建文件或目录 | 使用 `If-None-Match: *`，确保目标不存在        |
| 覆盖文件       | 必须提供匹配当前版本的 `If-Match`              |
| 修改元数据     | 必须提供匹配当前版本的 `If-Match`              |
| 删除文件或目录 | 必须提供匹配当前版本的 `If-Match`              |
| 移动           | 源必须提供 `If-Match`，目标由 `overwrite` 决定 |
| 复制           | 可校验源版本，目标由 `overwrite` 决定          |

此外：

- `If-Match: *` 要求目标存在；
- ETag 不匹配返回 `412 Precondition Failed`；
- 缺少必要的 `If-Match` 返回 `428 Precondition Required`；
- 服务端必须在同一并发控制范围内完成版本检查和提交；
- 文件写入使用临时文件和原子替换，成功后返回新的 ETag。

## 两种文件写入模式

### 小文件直接写入

小文件通过一次 `PUT` 提交，body 为原始字节。服务端写入临时文件，完成条件检查后原子替换目标；请求失败或取消时清理临时文件。

### 大文件上传

保留大文件上传模式，具体形式（上传会话、分块上传或断点续传）待实际需求明确后设计；新增操作同样以 `QUERY`/`POST`/`PUT`/`PATCH` + `type` 表达。

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

| 状态  | `error.code`         | 场景                                  |
| ----- | -------------------- | ------------------------------------- |
| `400` | `bad_request`        | 路径、query 参数或 `destination` 非法 |
| `400` | `not_a_file`         | 操作要求文件，目标是目录              |
| `400` | `not_a_directory`    | 操作要求目录，目标是文件              |
| `404` | `not_found`          | 资源不存在                            |
| `405` | `method_not_allowed` | 方法与 `type` 组合不适用              |
| `412` | `etag_mismatch`      | 前置条件不满足                        |
| `422` | `invalid_request`    | body 不符合该 `type` 的 schema        |
| `422` | `unsupported_type`   | `type` 缺失或不支持                   |

工作区相关错误码见 [Workspace.md](./Workspace.md)，如只读工作区返回 `403 read_only_workspace`。
