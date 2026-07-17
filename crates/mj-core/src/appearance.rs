//! 角色出场统计（§6.7 [SHOULD]）：每个角色在各章被提及多少次，最近一次在哪章，
//! 之后隔了多少章没再出现——好一眼看出谁「消失」太久了。
//!
//! 现在直接扫正文计数：全文索引（FTS5）属 M6，届时可迁过去。计数是按需触发的
//! 一次性动作，不在打字热路径上，扫一本书完全够快。
//!
//! 核心 `tally` 是纯函数（吃「已载入的章节 + 角色名集」，吐统计），可单测；
//! `count_appearances` 只是套一层读盘。

use crate::error::Result;
use crate::id::{BookId, CharacterId};
use crate::store::Store;

/// 一个角色的出场统计。
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    pub id: CharacterId,
    pub name: String,
    /// 全书被提及总次数（名 + 别名各自出现次数之和）。
    pub total: usize,
    /// 最近一次被提及的章：`(章序号 0 起, 章标题)`。从未提及则为 None。
    pub last: Option<(usize, String)>,
    /// 参与统计的章数（受损章不计）。
    pub total_chapters: usize,
}

impl Appearance {
    /// 最近一次出场之后，隔了多少章没再出现。从未出场返回 None。
    pub fn chapters_since_last(&self) -> Option<usize> {
        self.last
            .as_ref()
            .map(|(i, _)| self.total_chapters.saturating_sub(1).saturating_sub(*i))
    }
}

/// 一章的最小信息：标题 + 正文。
pub struct ChapterText {
    pub title: String,
    pub body: String,
}

/// 一个角色的称谓集：id + 名 + 全部称谓（名 + 别名，去空）。
pub struct NameSet {
    pub id: CharacterId,
    pub name: String,
    pub needles: Vec<String>,
}

/// 数一个称谓集在一段正文里的出现次数（各称谓非重叠计数求和）。
fn count_in(body: &str, needles: &[String]) -> usize {
    needles
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| body.matches(n.as_str()).count())
        .sum()
}

/// 纯统计：给定按阅读顺序排好的章节与角色称谓集，产出每个角色的出场统计。
pub fn tally(chars: &[NameSet], chapters: &[ChapterText]) -> Vec<Appearance> {
    let total_chapters = chapters.len();
    chars
        .iter()
        .map(|c| {
            let mut total = 0;
            let mut last = None;
            for (i, ch) in chapters.iter().enumerate() {
                let n = count_in(&ch.body, &c.needles);
                if n > 0 {
                    total += n;
                    last = Some((i, ch.title.clone()));
                }
            }
            Appearance {
                id: c.id,
                name: c.name.clone(),
                total,
                last,
                total_chapters,
            }
        })
        .collect()
}

/// 读盘 + 统计。章节按阅读顺序（卷序 → 章序），受损章跳过（读不出正文）。
pub fn count_appearances(store: &Store, book: BookId) -> Result<Vec<Appearance>> {
    let b = store.load_book(book)?;
    let chars = store.list_characters(book)?;

    let name_sets: Vec<NameSet> = chars
        .iter()
        .map(|c| NameSet {
            id: c.id,
            name: c.name.clone(),
            needles: c.all_names().map(|s| s.to_string()).collect(),
        })
        .collect();

    let mut chapters = Vec::new();
    for vol in &b.volumes {
        for ch in &vol.chapters {
            if ch.damaged.is_some() {
                continue;
            }
            // 单章读失败只跳过：一章读不出不该让整个统计报错。
            match store.load_body(book, ch.id) {
                Ok(body) => chapters.push(ChapterText {
                    title: ch.title.clone(),
                    body: body.text.to_string(),
                }),
                Err(e) => {
                    tracing::warn!(chapter = %ch.id, error = %e, "出场统计：跳过读不出的章");
                }
            }
        }
    }

    Ok(tally(&name_sets, &chapters))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn nameset(name: &str, aliases: &[&str]) -> NameSet {
        let mut needles = vec![name.to_string()];
        needles.extend(aliases.iter().map(|s| s.to_string()));
        NameSet {
            id: CharacterId::generate(),
            name: name.to_string(),
            needles,
        }
    }

    fn chap(title: &str, body: &str) -> ChapterText {
        ChapterText {
            title: title.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn counts_mentions_across_chapters() {
        let chars = [nameset("沈砚", &[])];
        let chapters = [
            chap("第一章", "沈砚推门而入，沈砚看着雪。"),
            chap("第二章", "那天没有他。"),
            chap("第三章", "沈砚又来了。"),
        ];
        let stats = tally(&chars, &chapters);
        assert_eq!(stats[0].total, 3, "共 3 次提及");
        assert_eq!(
            stats[0].last.as_ref().unwrap().0,
            2,
            "最近在第三章（索引 2）"
        );
    }

    #[test]
    fn aliases_count_too() {
        let chars = [nameset("沈砚", &["小砚", "沈公子"])];
        let chapters = [chap("第一章", "小砚笑了，沈公子点头，沈砚起身。")];
        let stats = tally(&chars, &chapters);
        assert_eq!(stats[0].total, 3, "名 + 两个别名各一次");
    }

    #[test]
    fn chapters_since_last_flags_long_absence() {
        let chars = [nameset("沈砚", &[])];
        let chapters = [
            chap("1", "沈砚"),
            chap("2", "无"),
            chap("3", "无"),
            chap("4", "无"),
        ];
        let stats = tally(&chars, &chapters);
        // 最近在索引 0，共 4 章 → 之后隔了 3 章。
        assert_eq!(stats[0].chapters_since_last(), Some(3));
    }

    #[test]
    fn never_mentioned_has_no_last() {
        let chars = [nameset("路人", &[])];
        let chapters = [chap("1", "没有这个人。")];
        let stats = tally(&chars, &chapters);
        assert_eq!(stats[0].total, 0);
        assert!(stats[0].last.is_none());
        assert_eq!(stats[0].chapters_since_last(), None);
    }

    #[test]
    fn overlapping_names_counted_per_string() {
        // 「沈砚」与「沈墨」都含「沈」，但各按整名计数，不互相污染。
        let chars = [nameset("沈砚", &[]), nameset("沈墨", &[])];
        let chapters = [chap("1", "沈砚和沈墨在一起，沈砚先走。")];
        let stats = tally(&chars, &chapters);
        assert_eq!(stats[0].total, 2, "沈砚 两次");
        assert_eq!(stats[1].total, 1, "沈墨 一次");
    }
}
