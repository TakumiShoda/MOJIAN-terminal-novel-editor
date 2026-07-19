//! 角色卡 CRUD 与校对上下文构建的集成测试。见 doc.md §6.7。
//!
//! 「重启」= 丢弃 Store 从磁盘重扫，验证磁盘是唯一真相。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::model::Relation;
use mj_core::proofing::{IgnoreSet, Proofer, build_context, ignore_key};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_text::proof::{CancelToken, ProofContext};

fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let store = Store::new(ws, Config::default());
    (dir, store)
}

fn reopen(dir: &tempfile::TempDir) -> (Workspace, Store) {
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    let store = Store::new(
        Workspace::resolve(Some(dir.path().to_path_buf())).unwrap(),
        Config::default(),
    );
    (ws, store)
}

#[test]
fn character_roundtrips_through_disk() {
    let (dir, mut store) = setup();
    let book = store.create_book("雪夜行", "沈砚").unwrap();

    let mut c = store.create_character(book.id, "沈砚").unwrap();
    c.aliases = vec!["沈公子".into(), "小砚".into()];
    c.role = "主角".into();
    c.gender = "男".into();
    c.age = "二十四".into();
    c.background = "出身书香门第。".into();
    c.speech = "口头禅：罢了。".into();
    c.custom
        .insert("武器".into(), toml::Value::String("青玉刀".into()));
    store.save_character(book.id, &c).unwrap();

    // 重启：从磁盘重读。
    let (_ws, store) = reopen(&dir);
    let loaded = store.load_character(book.id, c.id).unwrap();
    assert_eq!(loaded, c, "角色卡应一字不差地从磁盘还原");
    assert_eq!(
        loaded.custom.get("武器").and_then(|v| v.as_str()),
        Some("青玉刀"),
        "自定义字段要保留"
    );
}

#[test]
fn relations_and_first_appearance_persist() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷一", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();

    let master = store.create_character(book.id, "师父").unwrap();
    let mut c = store.create_character(book.id, "沈砚").unwrap();
    c.first_appearance = Some(ch);
    c.relations = vec![Relation {
        target: master.id,
        label: "师父".into(),
    }];
    store.save_character(book.id, &c).unwrap();

    let (_ws, store) = reopen(&dir);
    let loaded = store.load_character(book.id, c.id).unwrap();
    assert_eq!(loaded.first_appearance, Some(ch));
    assert_eq!(loaded.relations.len(), 1);
    assert_eq!(loaded.relations[0].target, master.id);
    assert_eq!(loaded.relations[0].label, "师父");
}

#[test]
fn list_characters_sorted_and_isolated_per_book() {
    let (dir, mut store) = setup();
    let a = store.create_book("甲书", "作者").unwrap();
    let b = store.create_book("乙书", "作者").unwrap();
    store.create_character(a.id, "周").unwrap();
    store.create_character(a.id, "陈").unwrap();
    store.create_character(a.id, "李").unwrap();
    store.create_character(b.id, "另一本书的人").unwrap();

    let (_ws, store) = reopen(&dir);
    let list = store.list_characters(a.id).unwrap();
    assert_eq!(list.len(), 3, "只列本书的角色");
    // 按名字排序。
    let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn delete_moves_to_trash_not_gone() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let c = store.create_character(book.id, "路人甲").unwrap();

    store.delete_character(book.id, c.id).unwrap();
    assert!(
        store.load_character(book.id, c.id).is_err(),
        "删除后正常路径读不到"
    );
    // §0：破坏性操作可撤销——文件应还在 trash 里。
    let trash = dir
        .path()
        .join("books")
        .join(book.id.to_string())
        .join("trash")
        .join("characters")
        .join(format!("{}.toml", c.id));
    assert!(trash.exists(), "删除的角色卡应进 trash，而非真删");
}

#[test]
fn build_context_pulls_names_and_aliases() {
    let (dir, mut store) = setup();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    let book = store.create_book("书", "作者").unwrap();

    let mut c = store.create_character(book.id, "沈砚").unwrap();
    c.aliases = vec!["小砚".into()];
    store.save_character(book.id, &c).unwrap();

    // 用户词典也应并入。
    std::fs::write(ws.user_dict_file(), "青玉刀 100 n\n# 注释行\n断魂谷\n").unwrap();

    let ctx = build_context(&store, &ws, book.id).unwrap();
    for want in ["沈砚", "小砚", "青玉刀", "断魂谷"] {
        assert!(
            ctx.names.iter().any(|n| n == want),
            "上下文缺少 {want}：{:?}",
            ctx.names
        );
    }
}

#[test]
fn proofer_flags_name_typo_using_character_names() {
    let (dir, mut store) = setup();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    let config = Config::default();
    let book = store.create_book("书", "作者").unwrap();
    store.create_character(book.id, "沈砚").unwrap();

    let ctx = build_context(&store, &ws, book.id).unwrap();
    let proofer = Proofer::from_workspace(&ws, &config);
    let issues = proofer
        .check_chapter(
            "那天沈研走进门。",
            &ctx,
            &IgnoreSet::default(),
            &CancelToken::new(),
        )
        .unwrap();
    assert!(
        issues.issues.iter().any(|i| i.rule_id == "name.suspect"),
        "应根据角色名「沈砚」把「沈研」标为可疑：{issues:?}"
    );
}

#[test]
fn ignored_issue_is_filtered_out() {
    let (_dir, _store) = setup();
    let ws = Workspace::resolve(Some(_dir.path().to_path_buf())).unwrap();
    let config = Config::default();
    let proofer = Proofer::from_workspace(&ws, &config);
    let text = "现场气氛如火如茶。";

    // 先跑一遍拿到那条错别字，算出它的忽略键。
    let first = proofer
        .check_chapter(
            text,
            &ProofContext::default(),
            &IgnoreSet::default(),
            &CancelToken::new(),
        )
        .unwrap();
    let typo = first
        .issues
        .iter()
        .find(|i| i.rule_id == "typo.confusion")
        .unwrap();
    let mut ignore = IgnoreSet::default();
    ignore.insert(ignore_key(text, typo));

    // 忽略后再跑，那条应消失。
    let second = proofer
        .check_chapter(text, &ProofContext::default(), &ignore, &CancelToken::new())
        .unwrap();
    assert!(
        !second.issues.iter().any(|i| i.rule_id == "typo.confusion"),
        "已忽略的问题不该再出现：{second:?}"
    );
}

#[test]
fn appearance_counts_mentions_across_chapters() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷一", None).unwrap();
    let c1 = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    let c2 = store
        .create_chapter(book.id, vol, "第二章", Some(c1))
        .unwrap();
    let c3 = store
        .create_chapter(book.id, vol, "第三章", Some(c2))
        .unwrap();
    let save = |store: &mut Store, ch, body: &str| {
        store
            .save_body(book.id, &mj_core::model::ChapterBody::new(ch, body))
            .unwrap();
    };
    save(&mut store, c1, "沈砚推门，沈砚看雪。\n");
    save(&mut store, c2, "这一章没有他。\n");
    save(&mut store, c3, "结尾又见沈砚。\n");

    let mut sy = store.create_character(book.id, "沈砚").unwrap();
    sy.aliases = vec!["小砚".into()];
    store.save_character(book.id, &sy).unwrap();
    store.create_character(book.id, "从未登场").unwrap();

    let (_ws, store) = reopen(&dir);
    let stats = mj_core::appearance::count_appearances(&store, book.id).unwrap();

    let shen = stats.iter().find(|a| a.name == "沈砚").unwrap();
    assert_eq!(shen.total, 3, "沈砚共提及 3 次");
    assert_eq!(shen.last.as_ref().unwrap().0, 2, "最近在第三章");
    assert_eq!(shen.total_chapters, 3);

    let ghost = stats.iter().find(|a| a.name == "从未登场").unwrap();
    assert_eq!(ghost.total, 0);
    assert!(ghost.last.is_none());
}

#[test]
fn user_confusion_overrides_builtin() {
    let (_dir, _store) = setup();
    let ws = Workspace::resolve(Some(_dir.path().to_path_buf())).unwrap();
    let config = Config::default();
    // 用户加一条自定义错别字。
    std::fs::write(ws.confusion_file(), "甲乙丙\t正确词\t\t自定义\n").unwrap();
    let proofer = Proofer::from_workspace(&ws, &config);
    let issues = proofer
        .check_chapter(
            "这里有甲乙丙。",
            &ProofContext::default(),
            &IgnoreSet::default(),
            &CancelToken::new(),
        )
        .unwrap();
    assert!(
        issues
            .issues
            .iter()
            .any(|i| i.suggestions == vec!["正确词".to_string()]),
        "用户混淆集条目应生效：{issues:?}"
    );
}
