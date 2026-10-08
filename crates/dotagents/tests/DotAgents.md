# DotAgents 设计

## 概览

`DotAgents` 把一棵 `.agents/` 目录当作一个整体来读：一次取回它持有的全部资源，并订阅此后每一次变化。

它承诺四件事：

- **整体性**：任一类资源加载失败，整棵目录都视为不持有资源，并交出失败原因。
- **快照**：一次读取得到一份自洽的集合，读不到半新半旧的状态。
- **增量**：变化按资源逐个报出，并带上变化之后的内容。
- **出现与消失**：目录本身的出现与消失，与目录内容的变化同等对待。

下文按命名空间引用类型：`plugin::`、`skill::`、`mcp::`、`watch::` 开头的来自对应的公开模块，其余（`DotAgents`、`Resources`、`LoadError`、`Diagnostic`）是 crate 根上的名字。

公开的记录与枚举都标了 `#[non_exhaustive]`，并都实现了 `Clone`、`PartialEq`、`Eq` 与 `Hash`：调用方可以比较、去重与键值化它们，但不能自行构造。

## 目录结构

```text
.agents/
├── mcp.json                      目录自身的 server 声明
├── skills/<skill>/SKILL.md       目录自身的 skill
└── plugins/<plugin>/
    ├── plugin.json               插件清单，有它才是插件
    ├── mcp.json                  该插件的 server 声明
    └── skills/<skill>/SKILL.md   该插件的 skill
```

一棵 `.agents/` 目录同时住着两类东西：目录自身的 `mcp.json` 与 `skills/`，以及 `plugins/` 下的若干插件。插件的各部分跟着插件走，目录自身的那份单独算。

## 资源集合

`Resources` 是一次加载的结果：目录自身的那份资源，加上 `plugins/` 下的全部插件。

| 字段 | 类型 | 内容 | 顺序 |
| --- | --- | --- | --- |
| `plugins` | `Vec<plugin::Plugin>` | `plugins/` 的直接子目录，每个是一个插件 | 目录名升序 |
| `skills` | `Vec<skill::Skill>` | `skills/` 的直接子目录中通过校验的那些 | 目录名升序 |
| `mcps` | `Vec<mcp::ServerEntry>` | `mcp.json` 的 `mcpServers` | 声明顺序 |
| `diagnostics` | `Vec<Diagnostic>` | 目录自身的组件被跳过或降级的原因 | 加载顺序 |

规则：

- `plugins/` 只认直接子目录。文件、以及更深一层的目录都不是插件。
- 插件的目录名就是它的 id；目录名不是文本时，这个目录没有可用的 id，不算插件，跳过。
- 插件自己的 skill 与 server 留在该 `plugin::Plugin` 里，不进 `skills` 与 `mcps`。这两个字段只表示目录自身的那份。
- skill 目录没有 `SKILL.md`，或 `SKILL.md` 不是普通文件时，它不算 skill，静默跳过。
- `SKILL.md` 存在但通不过校验时，跳过它并写一条 `diagnostics`；单个 skill 的失败不牵动其他资源。

### 元素形状

`plugin::Plugin` 就是一插件：身份与内容都在它身上。

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `id` | `String` | 插件根的目录名；规范规定它就是插件的标识 |
| `root` | `PathBuf` | 插件根，规范化的绝对路径 |
| `manifest` | `plugin::Manifest` | 校验通过的清单 |
| `skills` | `Vec<skill::Skill>` | 该插件的 skill |
| `mcp` | `plugin::McpOutcome` | 该插件的 MCP 组件的去向 |
| `diagnostics` | `Vec<Diagnostic>` | 加载这个插件时记下的非致命问题 |

`skill::Skill` 是 `skills/<目录名>` 的解析结果，由 `directory`、`path`、`meta` 与 `body` 组成；`directory` 恒等于 `meta.name`，规范要求二者一致。`mcp::ServerEntry` 是 `mcp.json` 里的一条 server，由 `name` 与 `server` 组成。

## 加载规则

### 整体性

一次加载要么完整成功，要么整棵目录都不持有资源。只要有一项致命问题，`state()` 就交出 `Err`，而不是交出剩下的那部分。

### 失败原因

`LoadError` 的每个变体对应一种致命问题。

| 变体 | 触发条件 | 附带信息 |
| --- | --- | --- |
| `Absent` | 路径不存在，或存在但不是目录 | — |
| `Unavailable { detail }` | 读目录、读条目或规范化路径时 IO 失败 | IO 层的原文 |
| `PluginRejected { directory, rejection }` | `plugins/` 下某个目录不成一个可用的插件 | 被拒的目录与拒绝原因 |
| `PluginMcpDisabled { id, reason }` | 某插件的 `mcp.json` 存在但不可用 | 插件 id 与禁用原因 |
| `Mcp(reason)` | 目录自身的 `mcp.json` 存在但不可用 | 禁用原因 |

`plugin::Rejection` 覆盖插件被拒的各种情形：`plugin.json` 缺失、不是普通文件、逃出插件根，或清单校验失败。`mcp::DisabledReason` 覆盖 `mcp.json` 不可用的各种情形：不是普通文件、逃出插件根、读不到、不是合法 JSON、不是对象、缺 `$schema`、`$schema` 的版本不受支持。

目录自身没有清单，所以它的 `mcp.json` 直接声明规范版本；插件的 `mcp.json` 的 `$schema` 则要与该插件的 `plugin.json` 一致。

不算失败的情形：

- `plugins/` 或 `skills/` 不存在：对应的资源类为空，仅此而已。
- `mcp.json` 不存在：目录没有 server 声明，仅此而已。
- `skills/` 存在却用不了（不是目录、逃出插件根、读不到）：跳过并写一条 `diagnostics`。
- `mcp.json` 里个别 server 条目无效：跳过并写一条 `diagnostics`，整个文件仍算配置成功。
- 清单里非致命的问题：记进该 `plugin::Plugin` 的 `diagnostics`。

`LoadError` 实现 `Display` 与 `Error`，文案为：

```text
there is no `.agents` directory
`.agents` could not be read: {detail}
the plugin at `{directory}` was rejected: {rejection}
plugin `{id}` has its MCP component disabled: {reason}
`mcp.json` is disabled: {reason}
```

## 打开与快照

```rust
pub async fn open(root: impl AsRef<Path>) -> Result<DotAgents, watch::Error>
pub fn state(&self) -> Result<Arc<Resources>, LoadError>
pub fn subscribe(&self) -> watch::Subscription
```

`open` 建立监听，并立刻加载一次作为基线。

- 目标已经是目录时直接监听它；不存在或不是目录时监听它的父目录，等它出现。因此 `.agents/` 不在时 `open` 照样成功。
- 先挂上监听再加载基线，中间不留缝；此后的每次加载都由监听驱动。
- 基线是无声的：它只决定 `state()` 的初值，不产生变化事件。
- `open` 只会因建立监听失败而失败，错误来自系统监视后端。加载失败不是 `open` 的失败，它出现在 `state()` 里。

`state()` 返回目录当下的资源。

- 返回值是 `Arc<Resources>`，与订阅者共享同一份，克隆廉价。
- 一次调用只对应一个时刻：调用者读到的所有字段来自同一次加载。
- 返回 `Err(LoadError)` 表示目录此刻不持有任何资源。

## 变化事件

### 事件形状

`watch::Event` 的每个变体描述一次变化。

| 变体 | 含义 |
| --- | --- |
| `Invalid(LoadError)` | 目录整体不再持有资源：它消失了，或它加载失败了 |
| `Plugin { id, change }` | `plugins/<id>/` 这个插件被加入、改动或移除 |
| `Skill { id, change }` | `skills/<id>/` 的一个 skill 被加入、改动或移除 |
| `Mcp { added, removed }` | 目录自身的 server 集合变了 |

`watch::PluginChange` 与 `watch::SkillChange` 的形状相同：`Added(值)` 表示刚出现，`Modified(值)` 表示还在但内容变了，两者都带上变化之后的值；`Removed` 不带值，表示不在了。

插件的 skill 与 server 不单独报事件，它们随插件一起变，归入 `watch::PluginChange`。`Skill` 与 `Mcp` 两个变体只表示目录自身的那份。

### 判定规则

两次相邻加载的内容之差就是这次的变化。判定不看文件系统事件的种类，所以重命名、原子保存、写到一半再写完，都归入同样的变化。

| 前后两次加载 | 报出的变化 |
| --- | --- |
| 内容相同 | 无事件 |
| 都成功 | 按 id 逐个比较插件与 skill，再比较 server 集合 |
| 后一次失败 | 一条 `Invalid`，带上新的 `LoadError` |
| 前一次失败、后一次成功 | 全部资源各报一条 `Added` |

逐项比较：

- 插件以 `plugin::Plugin::id` 为名，skill 以 `skill::Skill::directory` 为名。
- 只有后一次有：`Added`，带上后一次的值。
- 只有前一次有：`Removed`。
- 两次都有而内容不同：`Modified`，带上后一次的值。
- 内容相同：不报。

server 集合按条目整体比较：只有后一次有的进 `added`，只有前一次有的进 `removed`；两边都空时不报事件。改了配置的 server 会同时出现在 `added` 与 `removed` 里。

一批变化里，事件按插件、skill、server 的顺序给出；插件与 skill 各自按名字升序。

失败的两条特例：

- 后一次失败时只报一条 `Invalid`，不逐项报 `Removed`。目录的持有是整体的，失败时没有剩下的资源可报。
- 两次都失败而原因不同，报一条新的 `Invalid`；原因相同则不报。

## 订阅

`subscribe()` 从调用那一刻起接收变化。可以开多个订阅者，每个都收到同一份变化序列。

```rust
pub async fn recv(&mut self) -> Result<watch::Event, watch::SubscriptionError>
```

### 落后与停止

| 变体 | 含义 |
| --- | --- |
| `Lagged { events }` | 订阅者落后太多，`events` 条变化被丢弃 |
| `Stopped` | `DotAgents` 已释放，不会再有变化 |

队列容量是 256 条。收到 `Lagged` 或 `Stopped` 都说明订阅者的增量视图不再完整，应当重新读一次 `state()` 对齐；这时的错误只反映订阅者的处境，不表示目录本身有问题。

释放 `DotAgents` 即停止监听，订阅者随即收到 `Stopped`。

### 时序

```mermaid
flowchart LR
    A["文件系统变化"] --> B["去抖成一批"]
    B --> C["重新加载整棵目录"]
    C --> D["更新 state"]
    D --> E["广播事件"]
    E --> F["订阅者 recv，再读 state()"]
```

先更新 `state` 再广播事件，因此订阅者收到一条事件后立刻读 `state()`，读到的状态已经包含这条变化。

监视后端把去抖窗口内的变化合成一批，窗口是 500 ms；重新加载本身会打开文件，后端把这些打开也报成事件，读取类事件被忽略，循环不会自己触发自己。

## 范围之外

- 单个资源的解析与校验规则——`plugin.json` 的清单规范、`SKILL.md` 的 frontmatter、`mcp.json` 的 schema——见 [Plugin 设计](../../sandbox-toolkit/tests/Plugin.md)、[Skill 设计](../../sandbox-toolkit/tests/Skill.md) 与 [MCP 设计](../../sandbox-toolkit/tests/MCP.md)。
- 服务端如何挂载这些资源、global 与工作区如何合并，见 [Workspace 设计](../../sandbox-toolkit/tests/Workspace.md)。

## 待定

- 把 `DotAgents` 接进服务：当前还没有消费者。
- `crates/dotagents/tests/` 下只有本文档，还没有配对的 `dot_agents.rs`。
