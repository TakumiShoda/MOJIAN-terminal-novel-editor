# ADR 0004：章节 front matter 用 TOML 而非 YAML

日期：2026-07-16
状态：已接受

## 背景

doc.md §5.2 写「章节文件是**带 YAML front matter 的纯文本**」，并给出 `---` 包裹的示例。

但 §3 的技术栈列表里**没有任何 YAML crate**——有 `toml`、`serde_json`，没有 yaml。
文档自身在此处不自洽：要求了一种格式，却没给解析它的工具。

## 决定

改用 `+++` 包裹的 **TOML** front matter：

```toml
+++
id = "ch_7Q2M4KZA"
title = "第一章 雪夜"
status = "draft"
words = 3128
tags = ["伏笔"]
+++
　　雪落了一夜。
```

（`+++` 是 Hugo 等工具用于 TOML front matter 的既有约定，非本项目独创。）

## 理由

**决定性的一条是 §5.2 的 `[MUST]`「字段多余 → 保留原样回写，不得丢弃未知字段」。**

- Rust 生态的 YAML 首选 `serde_yaml` **已于 2024 年由作者正式归档、停止维护**。
  把一个无人维护的解析器放在「每一份手稿的每一次读写」这条必经之路上，
  是不可接受的风险——出了 bug 无处可修，而代价是用户的稿子。
- `toml` 已是项目依赖（config.toml / book.toml / volume.toml 都用它），
  且 `toml::Table` 的 flatten 透传能力**已在 M0 的 config 上验证过**
  （`preserves_unknown_fields_on_roundtrip` 测试）。同一套机制复用到 front matter，
  风险已知、行为已验证。
- 全项目元数据统一为 TOML，用户只需理解一种格式。§1 说「用户用记事本也能打开、也能救」——
  那么少一种格式就少一分理解成本。

## 代价（已知且接受）

1. **偏离文档字面**。故有此 ADR。
2. **`.md` 文件的 front matter 在部分 Markdown 工具里显示为普通文本**。
   但 §5.2 的核心诉求是「正文部分绝不包含任何私有标记——用户拿去别处必须能直接用」，
   指的是**正文**；front matter 本就是元数据区。且多数写作/静态站点工具（Hugo、Zola）
   原生支持 TOML front matter。
3. **中文键需加引号**：TOML 裸键仅限 ASCII，用户手写 `情绪 = "阴郁"` 会解析失败，
   必须写 `"情绪" = "阴郁"`。这在实现时被测试当场抓到。
   缓解措施见下节——损坏不再导致章节消失。

## 关联决定：front matter 损坏时不丢弃章节

实现扫描时的常规做法是「解析失败就跳过该文件」。但这意味着：用户手动改坏一行
front matter，章节就从目录树上凭空消失，而正文明明完好地躺在磁盘上。
用户的第一反应必然是「我的稿子没了」。

这与 §0 禁令 1「禁止任何可能静默丢失用户正文的路径」精神相悖——虽然文件没被删，
但从用户视角看和丢失无异。

故改为**降级显示**：
- 受损章仍出现在树上，标题显示 `⚠ <文件名>（元数据损坏）`，`damaged` 字段带原因；
- `save_body` 对受损章**拒绝写入**（`Error::ChapterDamaged`），
  以免覆盖掉用户还能人工救回的内容；
- 正文文件一字不动。

测试：`damaged_chapter_stays_visible_and_unwritable`、`save_body_refuses_damaged_chapter`。

## 后续

M6 的导入导出若需兼容外部 YAML front matter 的文稿，在导入侧做一次性转换即可，
不影响本项目的磁盘格式。
