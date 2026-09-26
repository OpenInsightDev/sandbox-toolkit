# Process 设计

区分 exec 与 pty：

- exec 模仿 MCP 的 streamable-http：请求在阈值 `wait` 内结束则直接返回 JSON 结果，否则在同一个 `200` 响应上升级为帧流，多路返回 stdout、stderr 与终态；
- pty 提供独立的 session API，每个 session 通过一条 WebSocket 双向通信。

## 端点与寻址

exec 与 pty 各占一个子路由：

| 模式       | exec                    | pty                    | `cwd` 语义     |
| ---------- | ----------------------- | ---------------------- | -------------- |
| 工作区模式 | `/workspaces/{id}/exec` | `/workspaces/{id}/pty` | 工作区相对路径 |
| 直接模式   | `/exec`                 | `/pty`                 | 远程绝对路径   |

- 无论哪种寻址模式，都按 [Workspace.md](./Workspace.md) 的环境变量注入全部已注册工作区的变量；
- 工作区模式下 `cwd` 必须位于工作区根目录内，越界返回 `400 bad_request`；`command` 不受工作区边界约束。

## exec

exec 以 JSON body 执行一个可执行文件或一段脚本，`format` 区分两种载荷；两种载荷共用同一组参数：

| 字段     | 说明                                                                           |
| -------- | ------------------------------------------------------------------------------ |
| `format` | 载荷种类，`exec`（缺省）或 `shell`                                             |
| `cwd`    | 可选，工作目录，缺省为工作区根目录（直接模式为服务进程工作目录）               |
| `env`    | 可选，环境变量，覆盖继承环境与注入的工作区变量                                 |
| `wait`   | 可选，等待命令结束以直接返回 JSON 的上限（毫秒）；缺省 `0` 表示不等待、直接流式 |

`format=exec` 的载荷：

| 字段      | 说明                                                                                                                       |
| --------- | -------------------------------------------------------------------------------------------------------------------------- |
| `command` | 必填，可执行 token：bare 名按 PATH（物化目录前置，见 [Binary.md](./Binary.md)）解析，含 `/` 时按路径解析，相对路径相对 `cwd` |
| `args`    | 可选，参数数组，逐项组成 argv                                                                                              |

`format=shell` 的载荷：

| 字段     | 说明                                         |
| -------- | -------------------------------------------- |
| `script` | 必填，脚本内容                               |
| `shell`  | 可选，解释器名或路径，缺省 `sh`；按 PATH 解析 |

- `env` 在继承的服务进程环境之上叠加，并覆盖 [Workspace.md](./Workspace.md) 注入的工作区变量；
- 要确定性拿到流式响应令 `wait=0`，要 JSON 传入 `wait>0`；
- body 不符合所选载荷 schema 返回 `422 invalid_request`，`format` 取不支持的值返回 `422 unsupported_type`；
- 启动失败（`command`/`script` 无法解析为可执行文件、`cwd` 非法或越界）走 [FileSystem.md](./FileSystem.md) 的统一错误信封，不进入 `status`；
- stdin 仅由 pty 承载。

### 响应

服务端按 `wait` 与输出量在两种形态间选择，两种都承载在普通 `200` 响应上、靠 `Content-Type` 区分：

- `wait>0`，命令在 `wait` 内结束，且 stdout、stderr 累计未超缓冲上限（4 MiB）：返回 JSON（`Content-Type: application/json`），stdout、stderr 按 UTF-8 解码，非法序列替换；
- 否则：升级为帧流（`Content-Type: application/vnd.sandbox-toolkit.exec-stream`），在流中多路返回 stdout、stderr 与终态；输出累计超过缓冲上限时提前升级。

```json
{ "status": "exited", "exit_code": 1, "stdout": "out", "stderr": "err" }
```

两种形态的 `status` 相同：

| `status` | 附加字段            | 场景                                 |
| -------- | ------------------- | ------------------------------------ |
| `exited` | `exit_code`（整数） | 进程退出并给出退出码，含 `0`         |
| `failed` | `message`           | 进程被信号终止等未产生退出码的情况   |

### 帧格式

帧流是长度前缀的帧序列，单向（服务端 → 客户端），信道编号与 pty 统一：

| 值  | 信道   | payload                    |
| --- | ------ | -------------------------- |
| `0` | stdin  | 保留，exec 不使用          |
| `1` | stdout | 进程标准输出，原始字节     |
| `2` | stderr | 进程标准错误，原始字节     |
| `3` | error  | 终态 `status` 的 JSON 编码 |

- 每帧为 1 字节信道标识加 4 字节大端长度，再加 payload；
- 帧间无分隔符，按长度切分，payload 可含任意字节；
- 发送端按 4 MiB 上限切分帧，接收端遇 payload 超过 4 MiB 的帧报协议错误；
- stdout 与 stderr 分属不同信道，不合并；
- 流以恰好一个 error 帧结束，缺少该帧表示流被截断。

## pty session

`POST .../pty` 创建 session：

| 字段      | 说明                                                          |
| --------- | ------------------------------------------------------------- |
| `command` | 必填，要执行的命令（通常是 shell），解析规则同 exec            |
| `args`    | 可选，参数数组                                                |
| `cwd`     | 可选，初始工作目录，缺省同 exec                               |
| `env`     | 可选，环境变量，同 exec                                       |
| `size`    | 可选，初始终端尺寸 `{ "width": …, "height": … }`；缺省由服务端定 |

成功返回 `201`：

```json
{ "session_id": "…", "url": "/workspaces/{id}/pty/{session_id}" }
```

- `url` 是连接该 session 的 WebSocket 端点，由请求推导的外部基址拼成绝对地址，客户端用 HTTP/2 CONNECT（RFC 8441）建立；
- 一个 session 对应一条 WebSocket，生命周期归服务端持有；
- 创建后 30s 内未建立连接即回收；连接后按 WebSocket 活动（含 ping/pong）计时，空闲超过 5min 回收；
- 客户端断开、发送 WebSocket close 或连接异常中断时，服务端终止整个 process group 并回收 session，避免容器内进程逃逸。

### 帧格式

每条 WebSocket 二进制消息首字节为信道标识，其余为 payload；信道集合固定：

| 值  | 信道   | 方向            |
| --- | ------ | --------------- |
| `0` | stdin  | 客户端 → 服务端 |
| `1` | stdout | 服务端 → 客户端 |
| `3` | error  | 服务端 → 客户端 |
| `4` | resize | 客户端 → 服务端 |

- pty 把 stderr 合并进 stdout（信道 `2` 不使用）；
- stdout 为原始字节；error 为终态 `status` 的 JSON，字段同 exec；
- resize 的 payload 是 2 字节大端宽度加 2 字节大端高度，共 4 字节；
- 关闭使用 WebSocket close 帧与状态码，不设独立信道；
- 单条消息上限 4 MiB。

### 双向交互

- 客户端 → 服务端只发送 stdin 和 resize 两类消息，顺序即终端输入顺序；
- 服务端 → 客户端发送 stdout 数据流，进程结束后追加一条 error 消息，随后关闭连接；
- 任一端发送 WebSocket close 后连接关闭，session 进入回收流程。

## 错误协议

沿用 [FileSystem.md](./FileSystem.md) 的统一 JSON 信封；exec 与 pty 创建共用：

| 状态  | `error.code`       | 场景                                             |
| ----- | ------------------ | ------------------------------------------------ |
| `400` | `bad_request`      | `cwd` 非法或越界；`command` 无法解析为可执行文件 |
| `404` | `not_found`        | 工作区不存在                                     |
| `422` | `invalid_request`  | body 不符合所选载荷 schema                       |
| `422` | `unsupported_type` | `format` 取不支持的值                            |

运行期结果不在此表，由响应 `status` 表达，见“响应”。

## 参考：k8s 与 Docker 的命令执行端点设计

### k8s api server

- **端点**：exec 作为 pod 的子资源，`POST /api/v1/namespaces/{ns}/pods/{name}/exec`；命令与流开关放在 query 参数：`command`（可重复出现，逐项组成 argv）、`stdin`、`stdout`、`stderr`、`tty`、`container`。
- **升级方式**：先发普通 HTTP 请求，服务端返回 `101 Switching Protocols` 升级为流。1.31 起由 SPDY 迁移到 WebSocket，协商靠子协议头：WebSocket 用 `Sec-WebSocket-Protocol: v5.channel.k8s.io`。
- **多路复用帧**：每条消息前缀 1 字节 channel ID 区分流：`0`=stdin、`1`=stdout、`2`=stderr、`3`=error、`4`=resize。退出状态不占独立信道，由流的 error channel / 连接结束语义携带。

### Docker remote api

- **两段式端点**：先 `POST /containers/{id}/exec` 创建 exec 实例（返回 `Id`），再 `POST /exec/{id}/start` 启动；tty 尺寸用 `POST /exec/{id}/resize?h=&w=` 另行调整；退出码通过 `GET /exec/{id}/inspect` 带外查询（`ExitCode`），不在流内传递。
- **hijack 升级**：`start` 请求带 `Connection: Upgrade` / `Upgrade` 头以提示代理做连接劫持；服务端有升级头则返回 `101`，否则降级返回 `200` 的裸流——同一端点可同时服务“升级”与“不升级”两种响应形态。
- **多路复用帧**：非 TTY 时 stdout/stderr 复用一条流，每帧为 8 字节头（1 字节流类型 `0`=stdin/`1`=stdout/`2`=stderr + 3 字节填充 + 4 字节大端长度）加 payload；Content-Type 区分 `application/vnd.docker.multiplexed-stream` 与 `raw-stream`（TTY 场景为原始 PTY 字节流）。exec 的 `start` 请求体用 `Detach`、`Tty` 控制行为。

### k8s TTY 会话

- **开启条件**：exec 请求同时带 `stdin=true` 与 `tty=true` 才建立交互式终端；`tty=true` 会把 stderr 合并进 stdout。
- **尺寸变更**：`v3.channel.k8s.io` 起支持终端 resize，客户端监听本地 `SIGWINCH` 后把新尺寸发到独立信道，服务端再转交容器运行时调整 pty。
- **信道复用与版本**：全部输入输出复用同一条连接，首字节标识目标信道：`0`=stdin、`1`=stdout、`2`=stderr、`3`=error、`4`=resize、`255`=close。其中 `v3` 加 resize、`v4` 加退出码（JSON error 走 error 信道）、`v5` 加 close 信号；resize 载荷为 `TerminalSize{Width, Height}`。
- **升级与协商**：SPDY 用 `X-Stream-Protocol-Version: v4.channel.k8s.io`（POST 升级），WebSocket 用 `Sec-WebSocket-Protocol: v5.channel.k8s.io`（GET 升级）。
- **会话生命周期**：会话从连接升级建立，到连接结束或收到 close 信号为止；服务端用连接跟踪缓存管理活动会话，并设有流创建超时（默认 30s）。连接中断时容器内进程仍继续运行，容易留下孤儿进程。
