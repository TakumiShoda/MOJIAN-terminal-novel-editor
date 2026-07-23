//! 领域模型。见 doc.md §5.3。

use std::path::PathBuf;

use ropey::Rope;
use serde::{Deserialize, Serialize};

use crate::id::{BookId, ChapterId, CharacterId, VolumeId};

/// 稀疏排序的步长。见 doc.md §5.3。
///
/// 新建/移动只取相邻两者的中值，不重写整卷——否则拖动一章要改写四百个文件。
pub const ORDER_STEP: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChapterStatus {
    #[default]
    Draft,
    Revised,
    Done,
}

impl ChapterStatus {
    /// 树上的状态色点用（doc.md §6.2）。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Draft => "○",
            Self::Revised => "◐",
            Self::Done => "●",
        }
    }
}

/// 书。`volumes` 有序。
#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    pub id: BookId,
    pub title: String,
    pub author: String,
    pub synopsis: String,
    pub genre: Vec<String>,
    pub target_words: Option<u64>,
    pub created: String,
    pub updated: String,
    /// 置顶：书架上排到最前（§6.1 [MUST]）。
    pub pinned: bool,
    /// 归档：完成/搁置的书，书架上沉到最底、置灰，但不删（§6.1 [MUST]）。
    pub archived: bool,
    pub volumes: Vec<Volume>,
    /// 未知字段透传，回写时保留（§5.3）。
    pub extra: toml::Table,
}

impl Book {
    pub fn new(id: BookId, title: impl Into<String>, author: impl Into<String>) -> Self {
        let now = crate::now_rfc3339();
        Self {
            id,
            title: title.into(),
            author: author.into(),
            synopsis: String::new(),
            genre: Vec::new(),
            target_words: None,
            created: now.clone(),
            updated: now,
            pinned: false,
            archived: false,
            volumes: Vec::new(),
            extra: toml::Table::new(),
        }
    }

    pub fn chapter_count(&self) -> usize {
        self.volumes.iter().map(|v| v.chapters.len()).sum()
    }

    /// 按 id 找章及其所属卷。
    pub fn find_chapter(&self, ch: ChapterId) -> Option<(&Volume, &ChapterMeta)> {
        self.volumes
            .iter()
            .find_map(|v| v.chapters.iter().find(|c| c.id == ch).map(|c| (v, c)))
    }

    pub fn find_chapter_mut(&mut self, ch: ChapterId) -> Option<&mut ChapterMeta> {
        self.volumes
            .iter_mut()
            .find_map(|v| v.chapters.iter_mut().find(|c| c.id == ch))
    }

    pub fn find_volume_mut(&mut self, vol: VolumeId) -> Option<&mut Volume> {
        self.volumes.iter_mut().find(|v| v.id == vol)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub id: VolumeId,
    pub title: String,
    /// 稀疏排序，步长 10（§5.3）。
    pub order: u32,
    pub synopsis: String,
    /// 有序；正文懒加载。
    pub chapters: Vec<ChapterMeta>,
    pub extra: toml::Table,
}

impl Volume {
    pub fn new(id: VolumeId, title: impl Into<String>, order: u32) -> Self {
        Self {
            id,
            title: title.into(),
            order,
            synopsis: String::new(),
            chapters: Vec::new(),
            extra: toml::Table::new(),
        }
    }
}

/// 章的元数据。正文不在此处——按需加载（§5.3）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterMeta {
    pub id: ChapterId,
    pub title: String,
    pub order: u32,
    pub status: ChapterStatus,
    /// 字数缓存。§5.2：以实际正文为准，不一致时重算。
    pub word_count: Option<u64>,
    pub tags: Vec<String>,
    /// 相对 workspace 根的路径。存相对路径而非绝对：
    /// workspace 整体搬移后仍然有效（§1 纯文本为真相，用户会自己拷目录）。
    pub path: PathBuf,
    pub updated: Option<String>,
    /// front matter 损坏时为 Some(原因)。
    ///
    /// 此类章仍出现在树上——正文就在磁盘上，从界面消失会让用户以为稿子丢了。
    /// 但**不可写**：`save_body` 会拒绝，以免覆盖掉本可人工救回的内容。
    pub damaged: Option<String>,
}

/// 正文。按需加载，卸载时释放（§5.3）。
#[derive(Debug, Clone)]
pub struct ChapterBody {
    pub id: ChapterId,
    /// ropey 缓冲：大章节 O(log n) 编辑（§6.3）。内容只含 LF（ADR 0003）。
    pub text: Rope,
    pub dirty: bool,
}

impl ChapterBody {
    pub fn new(id: ChapterId, text: impl AsRef<str>) -> Self {
        Self {
            id,
            text: Rope::from_str(text.as_ref()),
            dirty: false,
        }
    }
}

/// 在 `prev` 与 `next` 之间求一个 order。
///
/// 稀疏排序的核心（§5.3）：取中值；中值耗尽（相邻）时返回 None，
/// 由调用方触发整卷 renumber。
pub fn order_between(prev: Option<u32>, next: Option<u32>) -> Option<u32> {
    match (prev, next) {
        // 插到最前：留出前方空间。
        (None, None) => Some(ORDER_STEP),
        (None, Some(n)) => {
            if n <= 1 {
                None // 前方已无空位
            } else {
                Some(n / 2)
            }
        }
        // 插到最后：直接加一个步长。
        (Some(p), None) => Some(p.saturating_add(ORDER_STEP)),
        (Some(p), Some(n)) => {
            if n.saturating_sub(p) <= 1 {
                None // 中值耗尽
            } else {
                Some(p + (n - p) / 2)
            }
        }
    }
}

/// 整卷重排 order，步长恢复 10。中值耗尽时调用（§5.3）。
pub fn renumber(orders: &mut [u32]) {
    for (i, o) in orders.iter_mut().enumerate() {
        *o = (i as u32 + 1) * ORDER_STEP;
    }
}

/// 角色间的一条关系（§6.7）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub target: CharacterId,
    pub label: String,
}

/// 角色卡（§6.7）。`characters/<id>.toml`。
///
/// `name` + `aliases` 有两处硬用途：注入 jieba 用户词典（校对不误报的前提），
/// 以及喂专名一致性检查。其余字段是给作者看的设定，程序不解读。
#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    pub id: CharacterId,
    pub name: String,
    pub aliases: Vec<String>,
    pub role: String,
    pub gender: String,
    pub age: String,
    pub background: String,
    pub personality: String,
    pub appearance: String,
    pub habits: String,
    pub speech: String,
    pub relations: Vec<Relation>,
    pub first_appearance: Option<ChapterId>,
    pub notes: String,
    /// 用户自定义字段（`[custom]`），回写保留。
    pub custom: toml::Table,
}

impl Character {
    pub fn new(id: CharacterId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            aliases: Vec::new(),
            role: String::new(),
            gender: String::new(),
            age: String::new(),
            background: String::new(),
            personality: String::new(),
            appearance: String::new(),
            habits: String::new(),
            speech: String::new(),
            relations: Vec::new(),
            first_appearance: None,
            notes: String::new(),
            custom: toml::Table::new(),
        }
    }

    /// 校对用的全部称谓：名 + 别名，去空。
    pub fn all_names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str())
            .chain(self.aliases.iter().map(|s| s.as_str()))
            .filter(|s| !s.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_between_takes_midpoint() {
        assert_eq!(order_between(Some(10), Some(20)), Some(15));
        assert_eq!(order_between(Some(10), Some(11)), None, "相邻应耗尽");
        assert_eq!(order_between(Some(10), Some(12)), Some(11));
    }

    #[test]
    fn order_between_at_edges() {
        assert_eq!(order_between(None, None), Some(ORDER_STEP));
        assert_eq!(order_between(Some(10), None), Some(20), "插到最后");
        assert_eq!(order_between(None, Some(10)), Some(5), "插到最前");
        assert_eq!(order_between(None, Some(1)), None, "最前已无空位");
    }

    #[test]
    fn order_between_saturates_at_max() {
        // 不得溢出 panic（release 下开了 overflow-checks）。
        assert_eq!(order_between(Some(u32::MAX), None), Some(u32::MAX));
    }

    #[test]
    fn renumber_restores_step() {
        let mut orders = [3, 4, 5, 100];
        renumber(&mut orders);
        assert_eq!(orders, [10, 20, 30, 40]);
    }

    /// renumber 后必须能继续插入——这是它存在的意义。
    #[test]
    fn renumber_makes_room_again() {
        let mut orders = [10, 11, 12];
        assert_eq!(order_between(Some(orders[0]), Some(orders[1])), None);
        renumber(&mut orders);
        assert_eq!(order_between(Some(orders[0]), Some(orders[1])), Some(15));
    }

    #[test]
    fn status_symbols_are_distinct() {
        use std::collections::HashSet;
        let syms: HashSet<_> = [
            ChapterStatus::Draft,
            ChapterStatus::Revised,
            ChapterStatus::Done,
        ]
        .iter()
        .map(|s| s.symbol())
        .collect();
        assert_eq!(syms.len(), 3);
    }
}
