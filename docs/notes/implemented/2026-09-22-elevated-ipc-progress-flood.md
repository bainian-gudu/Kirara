# 提权管道进度洪水不再中断整次操作

Status: implemented

## Problem

`ipc/manager.rs::managed_operation` 用一条 `tokio::sync::broadcast` 同时承载进度与结果，
容量 100。接收侧写的是：

```rust
while let Ok(v) = rx.recv().await { … }
```

`broadcast::Receiver::recv()` 在接收方落后于发送方超过容量时返回
`Err(RecvError::Lagged(n))`，而 `while let Ok` 会把 `Err` 当成「通道关闭」直接跳出循环，
紧接着走「提权进程没了」的分支：清掉进程句柄并返回 `IPC_ERR`。

触发条件是常规的：文件多（每个文件若干条进度）、界面线程慢、提权进程一次性把进度推满
通道。此时提权进程其实还在正常干活，安装会继续写文件，主进程却已经报错并复位句柄——
用户看到安装失败，磁盘上却出现了一部分新文件；再点一次又会拉起第二个提权进程。

## Decision

- 接收侧显式区分两种错误：`Lagged` 只丢进度，记一条 warn 后 `continue` 继续收；
  只有 `Closed` 才跳出循环并按「提权进程退出」处理。
- 通道容量 100 → 256，给进度留出冗余，让 `Lagged` 在正常情况下不发生（`Lagged`
  的处理仍然必须正确，因为容量永远可能被瞬间打满）。

## Alternatives considered

- 把进度与结果拆成两条通道：结构上更干净，但要改提权侧发送点、`ManagedElevate`
  的订阅关系与所有调用方，收益只是少一次 `continue`。
- 只在发送侧做节流（合并进度）：治标，且节流参数要按文件大小/网速猜。
- 让 `Lagged` 直接重订阅：会丢掉 `Lagged` 之后到重订阅之间的消息，包括结果。

## Verification

| 判据 | 结果 |
|---|---|
| `Lagged` 不再终止循环 | PASS：`Err(RecvError::Lagged(skipped)) => { warn; continue }`；`Closed` 单独分支 |
| 容量提升 | PASS：`broadcast::channel(256)` |
| 断连仍按错误处理 | PASS：`Closed` 分支与既有的 `PipeErr` 分支都保持「清句柄 + `IPC_ERR`」 |
| 编译与行为回归 | 见 CI（Build / unit-test 两个 job） |

## Consequences

- 极端情况下主进程会丢掉一部分进度回调，界面进度条可能跳一段——但结果消息不会被跳过，
  安装的成败判定不受影响。
- 内存占用随容量增加：每条广播消息是 `serde_json::Value`，256 条相比 100 条多出的量在
  几十 KB 级。
