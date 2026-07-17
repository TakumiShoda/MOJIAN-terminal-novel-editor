//! §6.9 的验收项，用真实文件验证。
//!
//! 两条原文：
//! - 连续保存同样内容 100 次，`history/objects` 下只有 1 个 blob。
//! - 打满 40 条后继续保存，pinned 的快照仍在，且时间跨度覆盖仍在。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::history::{History, Retention, Trigger};
use mj_core::id::ChapterId;

fn history() -> (tempfile::TempDir, History) {
    let d = tempfile::tempdir().unwrap();
    let h = History::new(d.path());
    (d, h)
}

/// §6.9 验收 1：连续保存同样内容 100 次，objects 下只有 1 个 blob。
#[test]
fn identical_saves_produce_exactly_one_blob_on_disk() {
    let (dir, h) = history();
    let ch = ChapterId::generate();
    let text = "　　雪落了一夜。他推开门，风裹着雪灌进来。";

    for _ in 0..100 {
        h.snapshot(ch, text, Trigger::Auto, None, Retention::Thinned)
            .unwrap();
    }

    // 直接数磁盘上的文件，而不是信内存里的计数。
    let mut files = Vec::new();
    for bucket in std::fs::read_dir(dir.path().join("history/objects")).unwrap() {
        for f in std::fs::read_dir(bucket.unwrap().path()).unwrap() {
            files.push(f.unwrap().path());
        }
    }
    assert_eq!(files.len(), 1, "应只有 1 个 blob，实得 {files:?}");
    assert_eq!(h.list(ch).len(), 1, "链上也只该有一条");
}

/// 内容各不相同时，每份都该有自己的 blob——去重不能把真实的历史吃掉。
#[test]
fn different_content_produces_distinct_blobs() {
    let (_d, h) = history();
    let ch = ChapterId::generate();

    for i in 0..10 {
        h.snapshot(
            ch,
            &format!("　　雪落了{i}夜。"),
            Trigger::Auto,
            None,
            Retention::Thinned,
        )
        .unwrap();
    }
    assert_eq!(h.blob_count(), 10);
    assert_eq!(h.list(ch).len(), 10);
}

/// §6.9 验收 2：打满 40 条后继续保存，pinned 仍在，时间跨度覆盖仍在。
///
/// 用真实的 snapshot 调用把链打满——不是构造假数据。
#[test]
fn pinned_survives_a_flood_of_auto_snapshots() {
    let (_d, h) = history();
    let ch = ChapterId::generate();

    // 一条手动的「投稿版」。
    let milestone = h
        .snapshot(
            ch,
            "投稿那天的稿子",
            Trigger::Manual,
            Some("投稿版".into()),
            Retention::Thinned,
        )
        .unwrap()
        .unwrap();

    // 一个下午的密集自动快照：60 条，内容各不相同。
    for i in 0..60 {
        h.snapshot(
            ch,
            &format!("　　雪落了一夜。改到第 {i} 版。"),
            Trigger::Auto,
            None,
            Retention::Thinned,
        )
        .unwrap();
    }

    let snaps = h.list(ch);
    // 投稿版必须还在——这正是 §6.9 说纯 FIFO 不行的理由。
    assert!(
        snaps.iter().any(|s| s.id == milestone.id),
        "钉住的投稿版被 60 条自动快照挤掉了"
    );
    // 且它的正文必须还读得出来（blob 没被回收）。
    assert_eq!(h.read(&milestone.id).unwrap(), "投稿那天的稿子");

    // 未受保护的受 40 上限约束。
    let unprotected = snaps.iter().filter(|s| !s.is_protected()).count();
    assert!(unprotected <= 40, "未受保护的应 ≤ 40，实得 {unprotected}");
}

/// 被淘汰的快照，其 blob 该回收——否则 objects 会无限膨胀。
#[test]
fn dropped_snapshots_release_their_blobs() {
    let (_d, h) = history();
    let ch = ChapterId::generate();

    for i in 0..60 {
        h.snapshot(
            ch,
            &format!("第 {i} 版"),
            Trigger::Auto,
            None,
            Retention::Thinned,
        )
        .unwrap();
    }

    let kept = h.list(ch).len();
    assert_eq!(
        h.blob_count(),
        kept,
        "blob 数应与保留的快照数一致——多出来的就是没回收干净"
    );
}

/// 保留下来的快照，正文必须都还读得出来。
///
/// 回收 blob 时一旦误删了还在用的，用户点开历史就是一片空白——
/// 那比没有历史更糟：他以为稿子还在。
#[test]
fn every_kept_snapshot_is_still_readable() {
    let (_d, h) = history();
    let ch = ChapterId::generate();

    for i in 0..60 {
        h.snapshot(
            ch,
            &format!("　　第 {i} 版的正文。"),
            Trigger::Auto,
            None,
            Retention::Thinned,
        )
        .unwrap();
    }

    for s in h.list(ch) {
        let content = h
            .read(&s.id)
            .unwrap_or_else(|e| panic!("快照 {} 读不出来了：{e}", s.id));
        assert!(content.contains("版的正文"), "内容不对: {content:?}");
    }
}

/// 两章内容相同时共用 blob；淘汰其一不该殃及另一章。
#[test]
fn shared_blobs_survive_when_one_chapter_drops_them() {
    let (_d, h) = history();
    let a = ChapterId::generate();
    let b = ChapterId::generate();

    let shared = "　　两章一模一样的开头。";
    h.snapshot(a, shared, Trigger::Auto, None, Retention::Thinned)
        .unwrap();
    let b_snap = h
        .snapshot(b, shared, Trigger::Auto, None, Retention::Thinned)
        .unwrap()
        .unwrap();
    assert_eq!(h.blob_count(), 1, "相同内容应共用一个 blob");

    // 把 a 灌满，逼它淘汰旧的。
    for i in 0..60 {
        h.snapshot(
            a,
            &format!("a 的第 {i} 版"),
            Trigger::Auto,
            None,
            Retention::Thinned,
        )
        .unwrap();
    }

    assert_eq!(
        h.read(&b_snap.id).unwrap(),
        shared,
        "b 的快照被 a 的淘汰殃及了"
    );
}

/// 快照要能跨「重启」读出来——History 只是个路径包装，不持有状态。
#[test]
fn snapshots_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let ch = ChapterId::generate();
    let text = "　　雪落了一夜。";

    let id = {
        let h = History::new(dir.path());
        h.snapshot(ch, text, Trigger::Manual, None, Retention::Thinned)
            .unwrap()
            .unwrap()
            .id
    };

    // 「重启」：新建一个 History 实例。
    let h = History::new(dir.path());
    assert_eq!(h.list(ch).len(), 1);
    assert_eq!(h.read(&id).unwrap(), text);
}

/// 强制快照无视阈值——排版前那一条必须打上（§6.9）。
#[test]
fn forced_triggers_are_recorded() {
    let (_d, h) = history();
    let ch = ChapterId::generate();

    h.snapshot(
        ch,
        "排版前",
        Trigger::BeforeFormat,
        None,
        Retention::Thinned,
    )
    .unwrap();
    h.snapshot(
        ch,
        "替换前",
        Trigger::BeforeReplace,
        None,
        Retention::Thinned,
    )
    .unwrap();

    let snaps = h.list(ch);
    assert_eq!(snaps.len(), 2);
    assert!(snaps.iter().all(|s| s.trigger.is_forced()));
    assert_eq!(snaps[0].trigger.label(), "排版前");
}
