//! 版本历史：快照、保留策略、内容寻址存储。见 doc.md §6.9、§5.1。
//!
//! # 为什么内容寻址
//!
//! 快照 id = `blake3(正文)` 前 16 字节，blob 路径由 id 导出。于是「反复保存
//! 相同内容」天然不额外占空间——第 100 次保存和第 1 次落到同一个文件上。
//! §6.9 的验收项之一正是：连续保存同样内容 100 次，objects 下只有 1 个 blob。
//!
//! # 为什么不是 FIFO
//!
//! §6.9 说得很直白：纯 FIFO 下，一个下午的密集自动快照会把上个月的手稿全部
//! 挤掉——而用户想回退的恰恰是上个月那版。故默认 `thinned`：近的留得密，
//! 远的留得稀，pinned/有标签的永不淘汰。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::id::ChapterId;

/// 每章的快照上限（§6.9）。
pub const MAX_PER_CHAPTER: usize = 40;
/// pinned/有标签的快照另设上限（§6.9：满则提示用户手动清理）。
pub const MAX_PROTECTED: usize = 20;
/// 「最近 N 条全保留」（§6.9）。
const KEEP_RECENT: usize = 10;
/// zstd 压缩级别。3 是速度与体积的常用折中；快照要在保存路径上同步做，不能慢。
const ZSTD_LEVEL: i32 = 3;

/// 快照触发来源（§6.9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Auto,
    BeforeFormat,
    BeforeReplace,
    BeforeImport,
    BeforeDelete,
}

impl Trigger {
    /// 强制快照：无视自动快照的阈值（§6.9）。
    pub fn is_forced(self) -> bool {
        matches!(
            self,
            Self::BeforeFormat | Self::BeforeImport | Self::BeforeReplace | Self::BeforeDelete
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "手动",
            Self::Auto => "自动",
            Self::BeforeFormat => "排版前",
            Self::BeforeReplace => "替换前",
            Self::BeforeImport => "导入前",
            Self::BeforeDelete => "删除前",
        }
    }
}

/// 快照 id = blake3(正文) 前 16 字节，十六进制。
///
/// **内容即身份**：相同正文必得相同 id，这正是去重的全部机制。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapshotId(String);

impl SnapshotId {
    pub fn of(content: &str) -> Self {
        // 取前 16 字节再转十六进制，而不是「转完再切字符串」——
        // 后者要靠「blake3 的 hex 全是 ASCII」这个额外前提才安全，
        // 而这里根本不需要那个前提。
        let h = blake3::hash(content.as_bytes());
        let mut s = String::with_capacity(32);
        for b in &h.as_bytes()[..16] {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
        }
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// blob 的相对路径：`objects/<前2位>/<其余>.zst`（§5.1）。
    ///
    /// 分桶是为了别让一个目录塞进几万个文件——有些文件系统在那种规模下会变慢。
    fn blob_rel(&self) -> PathBuf {
        let (bucket, rest) = self.0.split_at(2);
        Path::new("objects")
            .join(bucket)
            .join(format!("{rest}.zst"))
    }
}

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一条快照的元数据。正文本身在 blob 里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub chapter: ChapterId,
    pub created: DateTime<Local>,
    pub trigger: Trigger,
    /// 用户命名的里程碑，如「投稿版」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 钉住则永不淘汰。
    #[serde(default)]
    pub pinned: bool,
    /// 字数（含标点口径，与状态栏同源）。
    #[serde(default)]
    pub words: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SnapshotId>,
    /// 未知字段透传——与 config/front matter 同一条前向兼容原则。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Snapshot {
    /// 是否受保护：pinned 或有标签的，永不淘汰（§6.9）。
    pub fn is_protected(&self) -> bool {
        self.pinned || self.label.is_some()
    }
}

/// 保留策略（§6.9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    /// 近密远疏。默认。
    #[default]
    Thinned,
    /// 简单 FIFO。
    Fifo,
}

/// 某本书的历史库。
pub struct History {
    /// `books/<book-id>/history/`
    root: PathBuf,
}

impl History {
    pub fn new(book_dir: &Path) -> Self {
        Self {
            root: book_dir.join("history"),
        }
    }

    fn refs_path(&self, ch: ChapterId) -> PathBuf {
        self.root.join("refs").join(format!("{ch}.json"))
    }

    fn blob_path(&self, id: &SnapshotId) -> PathBuf {
        self.root.join(id.blob_rel())
    }

    /// 读某章的快照链（按时间升序）。
    ///
    /// 文件不存在 = 还没有快照，不是错误。
    /// **文件损坏也不是致命错**：历史是附加价值，正文才是命根子。
    /// 坏了就当没有历史，让用户继续写——而不是拿一个 JSON 语法错误把他挡在门外。
    pub fn list(&self, ch: ChapterId) -> Vec<Snapshot> {
        let path = self.refs_path(ch);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        match serde_json::from_str::<Vec<Snapshot>>(&text) {
            Ok(mut v) => {
                v.sort_by_key(|s| s.created);
                v
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "快照链损坏，按无历史处理");
                Vec::new()
            }
        }
    }

    fn save_refs(&self, ch: ChapterId, snaps: &[Snapshot]) -> Result<()> {
        let path = self.refs_path(ch);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        let json = serde_json::to_string_pretty(snaps).map_err(|e| Error::ChapterParse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        // 走原子写：快照链写坏了，等于所有历史一起丢（§0 禁令 1）。
        crate::atomic::write(&path, json.as_bytes())
    }

    /// 取某快照的正文。
    pub fn read(&self, id: &SnapshotId) -> Result<String> {
        let path = self.blob_path(id);
        let bytes = std::fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let raw = zstd::decode_all(&bytes[..]).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        String::from_utf8(raw).map_err(|e| Error::ChapterParse {
            path,
            message: format!("快照不是合法 UTF-8：{e}"),
        })
    }

    /// 打一条快照。
    ///
    /// 返回 `None` 表示因去重未新建（内容与上一条相同，只更新了时间戳）。
    pub fn snapshot(
        &self,
        ch: ChapterId,
        content: &str,
        trigger: Trigger,
        label: Option<String>,
        retention: Retention,
    ) -> Result<Option<Snapshot>> {
        let id = SnapshotId::of(content);
        let mut snaps = self.list(ch);

        // 去重（§6.9）：与**上一条**内容相同则不新建，只更新时间戳。
        //
        // 只比上一条而非全链：A→B→A 时那个回头的 A 是一次真实的「改回去」，
        // 用户会想在历史里看到它。
        if let Some(last) = snaps.last_mut()
            && last.id == id
        {
            last.created = Local::now();
            // 带标签的快照重新触发时，标签要跟上——用户是在给**此刻**命名。
            if label.is_some() {
                last.label = label;
                last.pinned = true;
            }
            self.save_refs(ch, &snaps)?;
            return Ok(None);
        }

        // 写 blob。内容寻址：已存在就是同样的内容，不必重写。
        let blob = self.blob_path(&id);
        if !blob.exists() {
            if let Some(parent) = blob.parent() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_owned(),
                    source,
                })?;
            }
            let packed =
                zstd::encode_all(content.as_bytes(), ZSTD_LEVEL).map_err(|source| Error::Io {
                    path: blob.clone(),
                    source,
                })?;
            crate::atomic::write(&blob, &packed)?;
        }

        let snap = Snapshot {
            id: id.clone(),
            chapter: ch,
            created: Local::now(),
            trigger,
            pinned: label.is_some(),
            label,
            words: mj_text::count::count_with_punct(content) as u64,
            parent: snaps.last().map(|s| s.id.clone()),
            extra: serde_json::Map::new(),
        };
        snaps.push(snap.clone());

        // 淘汰 + 回收 blob。
        let dropped = apply_retention(&mut snaps, retention);
        self.save_refs(ch, &snaps)?;
        self.gc_blobs(&dropped)?;

        Ok(Some(snap))
    }

    /// 给快照打/去标签。
    pub fn set_label(&self, ch: ChapterId, id: &SnapshotId, label: Option<String>) -> Result<()> {
        let mut snaps = self.list(ch);
        for s in snaps.iter_mut().filter(|s| s.id == *id) {
            s.pinned = label.is_some();
            s.label = label.clone();
        }
        self.save_refs(ch, &snaps)
    }

    /// 钉住/取消。
    pub fn set_pinned(&self, ch: ChapterId, id: &SnapshotId, pinned: bool) -> Result<()> {
        let mut snaps = self.list(ch);
        for s in snaps.iter_mut().filter(|s| s.id == *id) {
            s.pinned = pinned;
        }
        self.save_refs(ch, &snaps)
    }

    /// 受保护的快照数（§6.9：上限 20，满则提示用户手动清理）。
    pub fn protected_count(&self, ch: ChapterId) -> usize {
        self.list(ch).iter().filter(|s| s.is_protected()).count()
    }

    /// 回收不再被引用的 blob。
    ///
    /// 必须扫**全书**的快照链：内容寻址意味着两章内容相同就共用一个 blob，
    /// 只看本章会把别章还在用的 blob 删掉。
    fn gc_blobs(&self, dropped: &[Snapshot]) -> Result<()> {
        if dropped.is_empty() {
            return Ok(());
        }
        let alive = self.all_live_ids();

        for s in dropped {
            if alive.contains(&s.id) {
                continue;
            }
            let path = self.blob_path(&s.id);
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::debug!(id = %s.id, "回收快照 blob"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // 回收失败不该让保存失败——顶多多占点空间。
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "回收 blob 失败"),
            }
        }
        Ok(())
    }

    /// 全书所有快照链里还活着的 id。
    fn all_live_ids(&self) -> HashSet<SnapshotId> {
        let mut out = HashSet::new();
        let Ok(entries) = std::fs::read_dir(self.root.join("refs")) else {
            return out;
        };
        for e in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            if let Ok(v) = serde_json::from_str::<Vec<Snapshot>>(&text) {
                out.extend(v.into_iter().map(|s| s.id));
            }
        }
        out
    }

    /// 统计 objects 下的 blob 数。供验收测试。
    pub fn blob_count(&self) -> usize {
        let mut n = 0;
        let Ok(buckets) = std::fs::read_dir(self.root.join("objects")) else {
            return 0;
        };
        for b in buckets.flatten() {
            if let Ok(files) = std::fs::read_dir(b.path()) {
                n += files.flatten().count();
            }
        }
        n
    }
}

/// 施加保留策略，返回被淘汰的快照。
///
/// `snaps` 原地改为保留下来的（仍按时间升序）。
pub fn apply_retention(snaps: &mut Vec<Snapshot>, retention: Retention) -> Vec<Snapshot> {
    let now = Local::now();
    let keep = match retention {
        Retention::Fifo => keep_fifo(snaps),
        Retention::Thinned => keep_thinned(snaps, now),
    };

    let mut dropped = Vec::new();
    let mut kept = Vec::new();
    for s in snaps.drain(..) {
        if keep.contains(&snapshot_key(&s)) {
            kept.push(s);
        } else {
            dropped.push(s);
        }
    }
    *snaps = kept;
    dropped
}

/// 唯一标识一条快照链条目。
///
/// 不能只用 id：A→B→A 时链上会有两条同 id 的记录，
/// 用 id 去筛会把两条一起留下或一起删掉。加上时间戳才能分辨。
type SnapKey = (String, i64);

fn snapshot_key(s: &Snapshot) -> SnapKey {
    (s.id.0.clone(), s.created.timestamp_micros())
}

/// FIFO：留最近的 40 条（受保护的除外）。
fn keep_fifo(snaps: &[Snapshot]) -> HashSet<SnapKey> {
    let mut out: HashSet<SnapKey> = snaps
        .iter()
        .filter(|s| s.is_protected())
        .map(snapshot_key)
        .collect();

    let mut rest: Vec<&Snapshot> = snaps.iter().filter(|s| !s.is_protected()).collect();
    rest.sort_by_key(|s| std::cmp::Reverse(s.created));
    out.extend(rest.into_iter().take(MAX_PER_CHAPTER).map(snapshot_key));
    out
}

/// thinned：近密远疏（§6.9 的优先级表）。
fn keep_thinned(snaps: &[Snapshot], now: DateTime<Local>) -> HashSet<SnapKey> {
    // 1. 受保护的永不淘汰，且**不占 40 的额度**。
    let mut out: HashSet<SnapKey> = snaps
        .iter()
        .filter(|s| s.is_protected())
        .map(snapshot_key)
        .collect();

    let mut rest: Vec<&Snapshot> = snaps.iter().filter(|s| !s.is_protected()).collect();
    rest.sort_by_key(|s| std::cmp::Reverse(s.created)); // 新 → 旧

    // 2. 最近 10 条全保留。
    let mut kept: Vec<&Snapshot> = rest.iter().take(KEEP_RECENT).copied().collect();

    // 3~5. 其余按时间桶抽稀，每桶留最新的一条。
    let mut seen_buckets = HashSet::new();
    for s in rest.iter().skip(KEEP_RECENT) {
        let bucket = time_bucket(now, s.created);
        // rest 已按新→旧排序，故每个桶里第一个碰到的就是最新的。
        if seen_buckets.insert(bucket) {
            kept.push(s);
        }
    }

    // 超出 40 时，从最旧的开始淘汰（对应「优先级最低的桶」——
    // 桶的优先级本就随时间递减，最旧的必在最低的桶里）。
    kept.sort_by_key(|s| std::cmp::Reverse(s.created));
    kept.truncate(MAX_PER_CHAPTER);

    out.extend(kept.into_iter().map(snapshot_key));
    out
}

/// 时间桶：24 小时内按小时，30 天内按天，更早按周（§6.9）。
///
/// 返回 (类别, 序号)——不同类别的序号不能混，否则「第 3 小时」会与「第 3 天」撞车。
fn time_bucket(now: DateTime<Local>, created: DateTime<Local>) -> (u8, i64) {
    let age = now.signed_duration_since(created);
    if age < Duration::hours(24) {
        (0, age.num_hours())
    } else if age < Duration::days(30) {
        (1, age.num_days())
    } else {
        (2, age.num_weeks())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn snap(id: &str, ago: Duration) -> Snapshot {
        Snapshot {
            id: SnapshotId(id.into()),
            chapter: ChapterId::generate(),
            created: Local::now() - ago,
            trigger: Trigger::Auto,
            label: None,
            pinned: false,
            words: 0,
            parent: None,
            extra: serde_json::Map::new(),
        }
    }

    fn history() -> (tempfile::TempDir, History) {
        let d = tempfile::tempdir().unwrap();
        let h = History::new(d.path());
        (d, h)
    }

    // ---- 内容寻址 ----

    #[test]
    fn same_content_gives_same_id() {
        assert_eq!(SnapshotId::of("雪落了一夜"), SnapshotId::of("雪落了一夜"));
        assert_ne!(SnapshotId::of("雪落了一夜"), SnapshotId::of("雪落了两夜"));
    }

    #[test]
    fn id_is_32_hex_chars() {
        let id = SnapshotId::of("雪");
        assert_eq!(id.as_str().len(), 32, "前 16 字节 = 32 个十六进制字符");
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn blob_path_is_bucketed() {
        let id = SnapshotId::of("雪");
        let rel = id.blob_rel();
        let s = rel.to_string_lossy();
        assert!(s.starts_with("objects/"), "{s}");
        assert!(s.ends_with(".zst"), "{s}");
        // 前两位分桶
        assert_eq!(rel.components().count(), 3, "objects/<桶>/<文件>");
    }

    // ---- 快照与去重 ----

    #[test]
    fn snapshot_roundtrips_content() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        let text = "　　雪落了一夜。他推开门。";

        let s = h
            .snapshot(ch, text, Trigger::Manual, None, Retention::Thinned)
            .unwrap()
            .unwrap();
        assert_eq!(h.read(&s.id).unwrap(), text, "快照必须逐字还原");
    }

    #[test]
    fn snapshot_preserves_cjk_and_emoji() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        let text = "　　「雪落了。」👨‍👩‍👧\n　　他推开门。";
        let s = h
            .snapshot(ch, text, Trigger::Manual, None, Retention::Thinned)
            .unwrap()
            .unwrap();
        assert_eq!(h.read(&s.id).unwrap(), text);
    }

    /// §6.9 验收：连续保存同样内容 100 次，objects 下只有 1 个 blob。
    #[test]
    fn saving_identical_content_100_times_makes_one_blob() {
        let (_d, h) = history();
        let ch = ChapterId::generate();

        for _ in 0..100 {
            h.snapshot(ch, "雪落了一夜", Trigger::Auto, None, Retention::Thinned)
                .unwrap();
        }
        assert_eq!(h.blob_count(), 1, "内容寻址应天然去重");
        assert_eq!(h.list(ch).len(), 1, "去重后链上也只该有一条");
    }

    /// 去重只更新时间戳，不新建（§6.9）。
    #[test]
    fn dedup_updates_timestamp_only() {
        let (_d, h) = history();
        let ch = ChapterId::generate();

        let first = h
            .snapshot(ch, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        assert!(first.is_some(), "第一次应新建");

        let second = h
            .snapshot(ch, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        assert!(second.is_none(), "内容相同应返回 None（未新建）");
        assert_eq!(h.list(ch).len(), 1);
    }

    /// 内容变了就该新建。
    #[test]
    fn different_content_creates_new_snapshot() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        h.snapshot(ch, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        h.snapshot(ch, "雪落", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        assert_eq!(h.list(ch).len(), 2);
        assert_eq!(h.blob_count(), 2);
    }

    /// A→B→A：回头的那个 A 是一次真实的「改回去」，用户会想看到它。
    #[test]
    fn returning_to_earlier_content_is_a_new_entry() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        h.snapshot(ch, "A", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        h.snapshot(ch, "B", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        h.snapshot(ch, "A", Trigger::Auto, None, Retention::Thinned)
            .unwrap();

        assert_eq!(h.list(ch).len(), 3, "改回去也是一次改动");
        assert_eq!(h.blob_count(), 2, "但 A 的内容只存一份");
    }

    #[test]
    fn snapshot_records_parent_chain() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        let a = h
            .snapshot(ch, "A", Trigger::Auto, None, Retention::Thinned)
            .unwrap()
            .unwrap();
        let b = h
            .snapshot(ch, "B", Trigger::Auto, None, Retention::Thinned)
            .unwrap()
            .unwrap();
        assert_eq!(b.parent, Some(a.id));
    }

    #[test]
    fn label_pins_the_snapshot() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        let s = h
            .snapshot(
                ch,
                "雪",
                Trigger::Manual,
                Some("投稿版".into()),
                Retention::Thinned,
            )
            .unwrap()
            .unwrap();
        assert_eq!(s.label.as_deref(), Some("投稿版"));
        assert!(s.pinned, "有标签的应自动钉住");
        assert!(s.is_protected());
    }

    #[test]
    fn list_is_empty_without_snapshots() {
        let (_d, h) = history();
        assert!(h.list(ChapterId::generate()).is_empty());
    }

    /// 快照链损坏不该让人写不了字——历史是附加价值，正文才是命根子。
    #[test]
    fn corrupt_refs_degrade_to_no_history() {
        let (d, h) = history();
        let ch = ChapterId::generate();
        std::fs::create_dir_all(d.path().join("history/refs")).unwrap();
        std::fs::write(
            d.path().join(format!("history/refs/{ch}.json")),
            "这不是 JSON",
        )
        .unwrap();

        assert!(h.list(ch).is_empty(), "坏了就当没有历史");
        // 仍然能继续打快照。
        assert!(
            h.snapshot(ch, "雪", Trigger::Auto, None, Retention::Thinned)
                .is_ok()
        );
    }

    // ---- 保留策略 ----

    #[test]
    fn fifo_keeps_most_recent_40() {
        let mut snaps: Vec<Snapshot> = (0..50)
            .map(|i| snap(&format!("{i:032x}"), Duration::minutes(50 - i as i64)))
            .collect();
        let dropped = apply_retention(&mut snaps, Retention::Fifo);
        assert_eq!(snaps.len(), MAX_PER_CHAPTER);
        assert_eq!(dropped.len(), 10);
    }

    #[test]
    fn thinned_keeps_recent_ten_untouched() {
        // 20 条，全在最近 1 小时内。
        let mut snaps: Vec<Snapshot> = (0..20)
            .map(|i| snap(&format!("{i:032x}"), Duration::minutes(20 - i as i64)))
            .collect();
        apply_retention(&mut snaps, Retention::Thinned);

        // 最近 10 条必在；其余同属「1 小时内」的桶，只留最新 1 条。
        assert!(snaps.len() >= KEEP_RECENT, "最近 10 条必须全留");
        assert!(
            snaps.len() <= KEEP_RECENT + 1,
            "同桶只留一条: {}",
            snaps.len()
        );
    }

    /// §6.9 验收：打满 40 条后继续保存，pinned 的仍在，且时间跨度覆盖仍在。
    #[test]
    fn thinned_preserves_pinned_and_time_span() {
        let mut snaps = Vec::new();
        // 一条 3 个月前的、钉住的「投稿版」。
        let mut old_pinned = snap("aa", Duration::days(90));
        old_pinned.pinned = true;
        old_pinned.label = Some("投稿版".into());
        snaps.push(old_pinned);

        // 一条一个月前的普通快照。
        snaps.push(snap("bb", Duration::days(35)));
        // 一条一周前的。
        snaps.push(snap("cc", Duration::days(7)));

        // 一个下午的密集自动快照：60 条，全在最近 3 小时内。
        for i in 0..60 {
            snaps.push(snap(&format!("{i:032x}"), Duration::minutes(180 - i)));
        }

        apply_retention(&mut snaps, Retention::Thinned);

        // pinned 必在。
        assert!(
            snaps.iter().any(|s| s.label.as_deref() == Some("投稿版")),
            "钉住的投稿版被挤掉了——这正是 §6.9 说纯 FIFO 不行的原因"
        );
        // 时间跨度仍覆盖：一个月前、一周前的都还在。
        assert!(snaps.iter().any(|s| s.id.0 == "bb"), "一个月前的被挤掉了");
        assert!(snaps.iter().any(|s| s.id.0 == "cc"), "一周前的被挤掉了");
        // 总数受控（受保护的不占额度）。
        let unprotected = snaps.iter().filter(|s| !s.is_protected()).count();
        assert!(unprotected <= MAX_PER_CHAPTER, "未受保护的应 ≤ 40");
    }

    /// 受保护的不占 40 的额度（§6.9）。
    #[test]
    fn protected_snapshots_do_not_consume_the_quota() {
        let mut snaps = Vec::new();
        for i in 0..15 {
            let mut s = snap(&format!("p{i:031x}"), Duration::days(100 + i as i64));
            s.pinned = true;
            snaps.push(s);
        }
        for i in 0..50 {
            snaps.push(snap(&format!("{i:032x}"), Duration::minutes(50 - i)));
        }
        apply_retention(&mut snaps, Retention::Thinned);

        assert_eq!(
            snaps.iter().filter(|s| s.pinned).count(),
            15,
            "15 条钉住的应全部保留"
        );
    }

    #[test]
    fn thinned_thins_old_snapshots_by_bucket() {
        let mut snaps = Vec::new();
        // 同一天内的 10 条（都在 5 天前），加上最近 10 条占满「全保留」额度。
        for i in 0..10 {
            snaps.push(snap(&format!("r{i:031x}"), Duration::minutes(i)));
        }
        for i in 0..10 {
            snaps.push(snap(
                &format!("o{i:031x}"),
                Duration::days(5) + Duration::minutes(i),
            ));
        }
        apply_retention(&mut snaps, Retention::Thinned);

        let old_kept = snaps.iter().filter(|s| s.id.0.starts_with('o')).count();
        assert_eq!(
            old_kept, 1,
            "同一天的旧快照只该留最新 1 条，实得 {old_kept}"
        );
    }

    #[test]
    fn time_buckets_do_not_collide_across_scales() {
        let now = Local::now();
        // 「3 小时前」与「3 天前」不能落进同一个桶。
        let a = time_bucket(now, now - Duration::hours(3));
        let b = time_bucket(now, now - Duration::days(3));
        assert_ne!(a, b, "不同尺度的桶不能撞车");
    }

    #[test]
    fn empty_retention_is_safe() {
        let mut snaps: Vec<Snapshot> = Vec::new();
        assert!(apply_retention(&mut snaps, Retention::Thinned).is_empty());
    }

    // ---- blob 回收 ----

    /// 淘汰快照时，blob 只在没人引用后才删（§6.9）。
    #[test]
    fn gc_keeps_blobs_still_referenced_by_other_chapters() {
        let (_d, h) = history();
        let a = ChapterId::generate();
        let b = ChapterId::generate();

        // 两章内容相同 → 共用一个 blob。
        h.snapshot(a, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        h.snapshot(b, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap();
        assert_eq!(h.blob_count(), 1, "相同内容应共用 blob");

        // 把 a 的快照挤掉。
        let id = SnapshotId::of("雪");
        h.gc_blobs(&[Snapshot {
            id: id.clone(),
            chapter: a,
            created: Local::now(),
            trigger: Trigger::Auto,
            label: None,
            pinned: false,
            words: 0,
            parent: None,
            extra: serde_json::Map::new(),
        }])
        .unwrap();

        assert_eq!(h.blob_count(), 1, "b 还在用，不能删");
        assert!(h.read(&id).is_ok(), "b 的快照必须还读得出来");
    }

    #[test]
    fn set_label_and_pin() {
        let (_d, h) = history();
        let ch = ChapterId::generate();
        let s = h
            .snapshot(ch, "雪", Trigger::Auto, None, Retention::Thinned)
            .unwrap()
            .unwrap();
        assert!(!s.is_protected());

        h.set_label(ch, &s.id, Some("投稿版".into())).unwrap();
        assert_eq!(h.protected_count(ch), 1);

        h.set_label(ch, &s.id, None).unwrap();
        assert_eq!(h.protected_count(ch), 0);

        h.set_pinned(ch, &s.id, true).unwrap();
        assert_eq!(h.protected_count(ch), 1);
    }

    #[test]
    fn trigger_forced_flags() {
        assert!(Trigger::BeforeFormat.is_forced());
        assert!(Trigger::BeforeReplace.is_forced());
        assert!(!Trigger::Auto.is_forced());
        assert!(!Trigger::Manual.is_forced());
    }
}
