# Toolkit 设计

本项目把每项功能尽可能同时以 HTTP 端点和 MCP 工具两种调用面暴露：HTTP 端点面向 SDK 与程序化客户端，MCP 工具面向 agent。两个调用面是同一套功能逻辑与同一套参数模型之上的薄适配层，本文定义这两个共用点；各功能自身的端点、参数与语义归各自文档，见[功能与文档](#功能与文档)。

## 两个调用面

| 调用面    | 面向               | 形态                                           |
| --------- | ------------------ | ---------------------------------------------- |
| HTTP 端点 | SDK / 程序化客户端 | REST：路由 + 请求头 + JSON 或原始 body         |
| MCP 工具  | agent              | `/mcp` 上的 Streamable HTTP，JSON-RPC 工具调用 |

MCP 工具面由 rmcp 提供，所有工具注册在同一个 `/mcp` server 上；从 `.agents/mcp.json` 发现并代理外部 MCP server 是另一项资源，见 [MCP.md](./MCP.md)。

## 分层

每项功能分三层：两个调用面依赖功能逻辑与参数模型，功能逻辑与参数模型互不依赖。

| 层       | 职责                                                 | 不负责             |
| -------- | ---------------------------------------------------- | ------------------ |
| 调用面   | 把传输形态翻译成一次逻辑调用，再把结果或错误编码回去 | 业务规则           |
| 功能逻辑 | 执行操作、校验，返回领域结果或领域错误               | 传输、路由、状态码 |
| 参数模型 | 描述输入输出                                         | 行为               |

- 两个调用面对同一操作调用同一函数、传同一模型，差别只在入参如何构造、错误如何映射。

## 参数模型

每项功能的输入输出都是 Rust 结构体或枚举，同一类型同时承担三种职责：

| 职责       | 依赖       | 用途                                             |
| ---------- | ---------- | ------------------------------------------------ |
| serde 模型 | `serde`    | HTTP body 与 MCP `arguments`、结果的 JSON 编解码 |
| MCP 模型   | `schemars` | MCP 工具的 `inputSchema` 与 `outputSchema`       |
| TS 导出    | `ts-rs`    | SDK 的类型定义                                   |

统一写法：输入类型 derive `Deserialize`，输出类型 derive `Serialize`，两者都 derive `JsonSchema` 与 `TS` 并加 `#[ts(export)]`。

```rust
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CreateWorkspaceRequest {
    pub(crate) id: String,
    pub(crate) root: String,
}
```

TS 绑定由 `#[ts(export)]` 生成的测试在 `cargo test` 时写出；落盘目录、整型映射与导入扩展名由 `.cargo/config.toml` 的 `TS_RS_EXPORT_DIR`、`TS_RS_LARGE_INT`、`TS_RS_IMPORT_EXTENSION` 固定。产物落在 `packages/sandbox-toolkit/src/generated`，只由重新生成更新，不手工编辑。

### 三面兼容约束

三种表示必须对同一类型给出同一语义，因此模型只使用三者都支持且一致的形式：

| 约束       | 规则                                                                                                                                |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| 命名       | JSON 名、JSON Schema 属性名与 TS 属性名由同一套 serde 命名规则派生，一律 `snake_case`；枚举标签同理。                               |
| MCP 输入   | 输入模型的根 schema 必须是 `type: object`，故输入用结构体；无参操作取空对象。                                                       |
| MCP 输出   | 输出用 `Json<T>` 承载才能生成 `outputSchema`；`T` 本身不受对象限制。                                                                |
| 可空       | `Option<T>` 的可空性由 JSON Schema 的联合类型表达，序列化为显式 `null`，不依赖 `nullable` 扩展。                                    |
| 大整数     | `i64`/`u64` 映射为 TS `number`，而 JSON 数与 JS 数都是 IEEE-754 双精度：64 位整数只用于可预期落在 `2^53-1` 以内的量，否则用字符串。 |
| 映射       | 键值容器用 `HashMap<String, T>`，导出为 TS `Record`；键必须是字符串。                                                               |
| 枚举       | 用 serde 的带标签表示（`tag`/`content`/`untagged`），三者均支持。                                                                   |
| 展平       | `#[serde(flatten)]` 三者均支持（TS 生成交叉类型），但与内部标签枚举组合时受限，慎用。                                               |
| 缺省与省略 | `skip_serializing_if` 只在同时有 `#[serde(default)]` 时才影响 TS，否则字段存在性在三面不一致。                                      |

约束的根据是三个库各自的规则：schemars 与 ts-rs 都从 serde 派生的属性读取命名与结构，而 MCP schema 另按 JSON Schema 2020-12 生成。

## 错误映射

功能逻辑返回领域错误类型，两个调用面各自把它映射到本传输的表示，分类不变：

- HTTP：映射为状态码 + `error.code` 信封，见 [FileSystem.md](./FileSystem.md) 的错误协议；
- MCP：映射为 JSON-RPC 错误，`data` 中携带与 HTTP 相同的稳定 `error.code`，使 agent 与 SDK 得到同一分类。

领域错误的种类与稳定码由各功能的错误表定义一次，两个调用面只做翻译。

## 调用面对应

同一操作在两个调用面上是：

| 维度       | HTTP 端点             | MCP 工具    |
| ---------- | --------------------- | ----------- |
| 定位       | 路由路径              | 工具名      |
| 寻址       | 路径参数              | 模型字段    |
| 结构化参数 | JSON body 或原始 body | 模型字段    |
| 结果       | 响应 body             | 工具 result |

### 规则

- 工具名与端点一一对应，命名 `<verb>_<noun>`，如 `create_workspace` 对应 `POST /workspaces`；
- HTTP 从路由与 body 之外取得的一切输入都必须是模型字段：MCP 没有路由，寻址（`workspace_id`、`path`）只能作为参数对象的字段；调用面负责把路径参数折叠进模型，MCP 面直接反序列化参数对象；
- HTTP 独有的协议输入留在调用面：条件请求头属 HTTP 语义，作为功能逻辑的显式、可选入参，由 HTTP 端点从头部构造；MCP 面没有请求头，是把它提升为模型字段还是使用免条件语义，由各功能文档规定；
- 数据面（原始 body 的读取与写入）属 HTTP 独有；MCP 面需要内容时，由控制面模型内联承载，如 `WriteResourceRequest.content`。

### 示例

注册工作区在两个调用面上的等价写法：

```
POST /workspaces
{ "id": "docs", "root": "/srv/project" }
```

```
create_workspace { "id": "docs", "root": "/srv/project" }
```

两者都反序列化为 `CreateWorkspaceRequest`，调用同一段注册逻辑。

## 单面功能

MCP 工具是单次请求-响应，无请求头与长连接，因此下列功能只在 HTTP 面提供，其余短操作尽可能双面提供：

| 功能                         | HTTP | MCP                  |
| ---------------------------- | ---- | -------------------- |
| 文件数据面（原始 body 读写） | 有   | 无，内容走控制面模型 |
| exec 升级流                  | 有   | 无                   |
| pty session（WebSocket）     | 有   | 无                   |
| 文件变更监听（事件流）       | 有   | 无                   |

## 模块布局

- 每个功能一个模块，内含共享模型 `model`、HTTP 调用面，以及功能逻辑，如工作区的 `registry`；
- MCP 调用面作为功能模块内与功能同名的子模块，如 `workspace/mcp.rs`、`fs/mcp.rs`，复用功能的模型与逻辑；
- 所有 MCP 工具注册到 `server` 中同一个会话级 server，并挂载为 `/mcp` 端点。
