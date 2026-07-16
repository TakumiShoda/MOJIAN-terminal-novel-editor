# ADR 0001：M0 环境搭建与两项 [VERIFY] 的实测结论

日期：2026-07-16
状态：已接受

## 背景

doc.md 对若干技术判断标了 `[VERIFY]`，要求实现时用真实环境验证，不得直接当事实。本条记录 M0 搭建环境时实际验证到的结论。

验证环境：macOS 15（Darwin 24.6.0）/ aarch64 / Apple clang 17。

## 结论

### 1. MSRV 1.88 —— 成立

doc.md §3 标注「MSRV Rust 1.88（ratatui 0.30 要求）`[VERIFY]`」。

实测 `cargo +1.88.0 check --workspace --all-targets` 通过。本仓库采用 edition 2024（需 ≥1.85），与 MSRV 1.88 不冲突。CI 设独立 `msrv` job 守住这条线；破坏 MSRV 属契约变更，需另写 ADR。

日常开发工具链为 stable（当前 1.97.1），由 `rust-toolchain.toml` 锁定 channel 与组件。

### 2. rusqlite bundled 的 FTS5 + trigram —— 可用

doc.md §5.4 标注「`[VERIFY]` 确认 rusqlite bundled 版本是否启用 FTS5 与 trigram，未启用则回退为『遍历 + 内存匹配』」。

实测 rusqlite 0.32.1（features = ["bundled"]）：
- `CREATE VIRTUAL TABLE ... USING fts5(..., tokenize = 'trigram')` 建表成功；
- 中文子串 `风裹着雪` 经 `MATCH` 命中，无需外挂分词器。

**因此 §5.4 的「遍历 + 内存匹配」回退方案在当前依赖下不需要实现。** 该结论以 `crates/mj-core/tests/verify_fts5.rs` 固化为测试——若将来 rusqlite 换版或改 feature 导致 FTS5 缺失，CI 会直接失败，而不是等到用户搜不到东西才发现。

注意 trigram 分词器要求查询串至少 3 个字符，短于 3 字的中文查询需另行处理（留待 M3 §6.6 实现时定）。

### 3. 依赖版本 —— 全部按 doc.md §3 基线解析成功

ratatui 0.30.2 / ropey 1.6.1 / rusqlite 0.32.1 / jieba-rs 0.7.4 / zstd 0.13.3 /
blake3 1.8.5 / similar 2.7.0 / unicode-width 0.2.2。

crossterm 未单独引入，统一走 ratatui 再导出的 `ratatui-crossterm`（遵 §3 「勿重复引入不同版本」）。

## 尚未验证

- §2.1 终端字体能力（OSC 50 / kitty 远程控制 / WezTerm）——留待 M6，需要在真实的 kitty / alacritty / WezTerm 下逐个试。
- §6.10 truecolor 探测与 256 色降级——同上。
