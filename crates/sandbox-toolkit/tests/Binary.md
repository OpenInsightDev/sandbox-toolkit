# Binary 设计

## 概览

构建期把常用命令行工具的预编译产物压缩内嵌进 sbxtkt 二进制；启动时物化成真实文件，并把所在目录挂进 PATH，容器内无需安装、无需联网即可按名字调用。

内嵌四个工具，各取其上游 release 的预编译产物：

| 工具 | 上游 |
| --- | --- |
| `fd` | sharkdp/fd |
| `rg` | BurntSushi/ripgrep |
| `uv` | astral-sh/uv |
| `deno` | denoland/deno |

## 内嵌

build.rs 在构建期从各上游 release 取与目标平台匹配的预编译产物，读出其中需要的可执行文件，压缩后内嵌进服务。

构建脚本同时内嵌一份清单，逐工具记录解压后字节的 blake3 摘要，即构建期预计算的签名。

### 测试

- `embed::tools`：四个工具的数据都内嵌在服务二进制里，物化出的文件可直接执行。

## 物化

运行期把内嵌的工具释放成真实文件，物化在 `$CACHE/sandbox-toolkit/bin`；`$CACHE` 取自 dirs crate 读到的系统缓存目录。

服务启动时执行一次物化，并行展开每个工具，阻塞至全部完成再继续启动 server。

每次启动都对每个已落盘文件与内嵌清单比对：一致即跳过重写，不一致或缺失则重新物化。

### 测试

- `materialize::layout`：四个工具都落成 `$CACHE/sandbox-toolkit/bin` 下的真实可执行文件。
- `materialize::skip`：已落盘文件与清单一致时启动不重写。
- `materialize::repair`：文件被改坏或删除后启动重新物化，内容回到与清单一致。

## PATH

exec 模块部署 subprocess 时必须把物化路径添加到 PATH 中，确保 `fd`、`rg`、`uv`、`deno` 以及 `node` 均直接可用。`node` 不单独内嵌，由 `deno` 承担。

### 测试

- `path::tools`：`/exec` 中 `fd`、`rg`、`uv`、`deno` 都按名字直接调用成功。
- `path::keeps`：物化路径加进 PATH，原有条目保留。
