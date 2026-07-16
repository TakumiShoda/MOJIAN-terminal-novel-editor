# ADR 0002：M0 实现中对 doc.md 的三处偏离

日期：2026-07-16
状态：已接受

doc.md §0 要求：偏离文档需写 ADR 说明理由。M0 有三处。

## 1. workspace 默认路径按平台取值，而非固定 `~/.local/share/mojian`

doc.md §5.1 写死了 `~/.local/share/mojian`。实现改为走 `directories` crate：
Linux 仍是 `~/.local/share/mojian`，macOS 则是 `~/Library/Application Support/mojian`。

理由：§9 把 macOS 列为一等支持。在 macOS 上创建 `~/.local/share` 不符合平台惯例。
文档里的路径应理解为「Linux 下的取值示例」，而非跨平台的硬性规定。

`--workspace` 与 `MOJIAN_WORKSPACE` 可覆盖（后者为实现新增，测试与脚本需要）。

## 2. panic hook 中的 stderr 输出

§0 禁止事项 2 禁止向 stdout/stderr 打印。panic hook 里保留了一处 `eprintln!`
（已就地 `#[allow(clippy::print_stderr)]` 并注明理由）。

理由：该禁令的目的是「不撕裂 TUI 界面」。panic hook 执行时 TUI 已经死了、
终端已恢复，不存在撕裂问题。而「未保存的正文 dump 到哪了」这条信息只能靠
stderr 告诉用户——写进日志他看不见，而他此刻正盯着一屏 backtrace 以为稿子没了。

其余所有代码路径的禁令由 workspace lint `print_stdout`/`print_stderr` 强制。

## 3. crash dump 不走原子写

§0 禁止事项 1 要求所有写盘走「tmp + fsync + rename」。crash dump 用了直接的
`std::fs::write`。

理由：原子写解决的是「覆盖已有文件时中途失败会毁掉原内容」。crash dump 每次
写的是带时间戳的新文件，不覆盖任何东西，没有可毁的原内容。而 panic 路径上
进程状态已不可信，步骤越少越可能成功——多一次 fsync 和 rename 就多两次失败机会。

正文的正常保存路径（M1 的 `Store::save_body`）必须走 `mj_core::atomic::write`，
此处不构成先例。

## 附：M0 期间发现并修复的一个真实缺陷

`panic::install` 最初用 `take_hook()` + 链式调用，重复调用会让 hook 层层叠加，
导致一次 panic 恢复多遍终端、打印多遍 backtrace。改为用 `OnceLock` 只捕获一次
原始 hook，`install` 幂等。回归测试见 `tests/panic_recovery.rs::repeated_install_does_not_stack_hooks`。
