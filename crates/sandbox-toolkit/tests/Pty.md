# Pty 设计

## 概览

pty 提供独立的 session API：先创建 session，再用返回的 endpoint 建立一条 WebSocket 完成双向通信。

session 的挂载规则、`cwd`/`env` 缺省与终态 JSON 沿用 [Exec 设计](./Exec.md)。

## 创建

`POST /pty`（workspace 模式为 `/workspaces/{workspace_id}/pty`）。

| 字段 | 说明 |
| --- | --- |
| `command` | 必填，要执行的命令（通常是 shell），解析规则同 exec |
| `args` | 可选，参数数组 |
| `cwd` | 可选，初始工作目录，缺省同 exec |
| `env` | 可选，环境变量，同 exec |
| `size` | 可选，初始终端尺寸 `{ "rows": …, "cols": … }`；缺省 24 行 × 80 列 |

成功返回 `201`：

```json
{ "id": "…", "endpoint": "/workspaces/{id}/pty/{session_id}" }
```

### 测试

- `pty::create`：`POST /pty` 返回 `201`，含 `id` 与指向该 session 的 `endpoint`。

## 附接

`endpoint` 是该 session 的 WebSocket 端点，为服务器相对路径，直接模式下形如 `/pty/{session_id}`；客户端在 HTTP 基址上把 scheme 换成 `ws`/`wss`。服务端只接受 HTTP/2 extended CONNECT（RFC 8441）。

一个 session 对应一条 WebSocket，生命周期归服务端持有。创建后 30s 内未建立连接即回收；连接后按 WebSocket 活动（含 ping/pong）计时，空闲超过 5min 回收。

一个 session 只能附着一次；`{session_id}` 不存在或已被附着时返回 `404 not_found`。

客户端断开、发送 close 或连接异常中断时，服务端终止整个 process group 并回收 session，避免容器内进程逃逸。

### 测试

- `pty::unknown_session`：连接不存在的 `{session_id}` 返回 `404`。
- `pty::attach_once`：同一 session 第二次连接返回 `404`。
- `pty::attach`：连接 `endpoint` 后 stdout 经信道 `1` 返回，进程结束经信道 `3` 返回终态 JSON 并关闭。
- `pty::close`：客户端发送 close 后 session 结束并回收。
- `pty::reclaims`：客户端断开后 session 被回收。

## 信道

每条 WebSocket 二进制消息首字节为信道标识，其余为 payload，信道集合固定。

| 值 | 信道 | 方向 |
| --- | --- | --- |
| `0` | stdin | 客户端 → 服务端 |
| `1` | stdout | 服务端 → 客户端 |
| `3` | error | 服务端 → 客户端 |
| `4` | resize | 客户端 → 服务端 |

- 客户端 → 服务端只发送 stdin 与 resize，顺序即终端输入顺序；
- 服务端 → 客户端发送 stdout 数据流，进程结束后追加一条 error 消息，随后关闭连接；
- 关闭使用 WebSocket close 帧，不设独立信道；任一端的 close 或连接中断都结束会话；
- pty 把 stderr 合并进 stdout（信道 `2` 不使用）；
- stdout 为原始字节；error 为终态 JSON，字段同 exec；
- resize 的 payload 是 2 字节大端行数加 2 字节大端列数，共 4 字节；
- 单条消息上限 4 MiB；
- 信道编号沿用 k8s v5 的 stdin/stdout/error/resize（不设 stderr 与 close 信道）；resize 与 error 的 payload 编码为本协议自定义，与 k8s 的 `TerminalSize`/`metav1.Status` 不同。

### 测试

- `pty::exit`：进程退出时以 error 帧发送终态 JSON。
- `pty::channels`：stdin/stdout/error/resize 信道固定，stderr 并入 stdout（信道 `2` 不使用）。
- `pty::resize`：resize 的 payload 为 2 字节大端行数加 2 字节大端列数，共 4 字节。
- `pty::message_limit`：单条消息上限 4 MiB。
