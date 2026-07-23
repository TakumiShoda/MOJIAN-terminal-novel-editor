//! 磁盘读写、原子写、扫描。见 doc.md §5.1、§6.1、§6.2。
//!
//! 铁律（§0 禁令 1、3）：
//! - **磁盘是唯一真相**。内存里的 `Book` 只是当前视图，任何修改都要落盘。
//! - 所有写盘走 `atomic::write`，没有例外。
//! - 正文读入即归一化 LF，写出按配置转换（ADR 0003）。
//!
//! 目录布局见 §5.1：
//! ```text
//! books/<book-id>/book.toml
//!                /volumes/<order>-<slug>/volume.toml
//!                                       /chapters/<order>-<slug>.md
//! ```
//! 目录名里的 order 与 slug **仅供人眼**；真相在 toml 里的 id 与 order。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chapter_file::{ChapterFile, FrontMatter};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::id::{BookId, ChapterId, CharacterId, VolumeId};
use crate::model::{
    Book, ChapterBody, ChapterMeta, Character, Relation, Volume, order_between, renumber,
};
use crate::slug::slugify;
use crate::workspace::Workspace;

pub struct Store {
    ws: Workspace,
    config: Config,
}

// ---- 磁盘上的 toml 形态 ----
//
// 与内存模型分开：内存模型有 volumes/chapters 的树，磁盘上它们是各自的文件。
// 混用一个类型会导致「存一本书要序列化整棵树」，与懒加载冲突。

#[derive(Debug, Serialize, Deserialize)]
struct BookToml {
    id: BookId,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    synopsis: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    genre: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_words: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated: Option<String>,
    /// 置顶/归档（§6.1）。默认 false，故老书读进来不受影响。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    archived: bool,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Serialize, Deserialize)]
struct VolumeToml {
    id: VolumeId,
    #[serde(default)]
    title: String,
    #[serde(default)]
    order: u32,
    #[serde(default)]
    synopsis: String,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Serialize, Deserialize)]
struct RelationToml {
    target: CharacterId,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CharacterToml {
    id: CharacterId,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    gender: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    age: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    background: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    personality: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    appearance: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    habits: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    speech: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    relations: Vec<RelationToml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_appearance: Option<ChapterId>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notes: String,
    // §6.7 的 `[custom]`：用户自定义字段。放最后，TOML 里会渲染成一个表段。
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    custom: toml::Table,
}

impl CharacterToml {
    fn into_model(self) -> Character {
        Character {
            id: self.id,
            name: self.name,
            aliases: self.aliases,
            role: self.role,
            gender: self.gender,
            age: self.age,
            background: self.background,
            personality: self.personality,
            appearance: self.appearance,
            habits: self.habits,
            speech: self.speech,
            relations: self
                .relations
                .into_iter()
                .map(|r| Relation {
                    target: r.target,
                    label: r.label,
                })
                .collect(),
            first_appearance: self.first_appearance,
            notes: self.notes,
            custom: self.custom,
        }
    }

    fn from_model(c: &Character) -> Self {
        Self {
            id: c.id,
            name: c.name.clone(),
            aliases: c.aliases.clone(),
            role: c.role.clone(),
            gender: c.gender.clone(),
            age: c.age.clone(),
            background: c.background.clone(),
            personality: c.personality.clone(),
            appearance: c.appearance.clone(),
            habits: c.habits.clone(),
            speech: c.speech.clone(),
            relations: c
                .relations
                .iter()
                .map(|r| RelationToml {
                    target: r.target,
                    label: r.label.clone(),
                })
                .collect(),
            first_appearance: c.first_appearance,
            notes: c.notes.clone(),
            custom: c.custom.clone(),
        }
    }
}

impl Store {
    pub fn new(ws: Workspace, config: Config) -> Self {
        Self { ws, config }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    // ---- 书架（§6.1）----

    /// 扫描 books/ 下的所有书。
    ///
    /// §6.1 验收：手动往 books/ 里丢一个符合布局的目录，重启后能识别。
    /// 故这里以**目录扫描**为准，不依赖 library.toml——后者只是排序/置顶的缓存。
    ///
    /// §6.1 性能：只读 book.toml + volume.toml，**不读正文**。
    pub fn list_books(&self) -> Result<Vec<Book>> {
        let dir = self.ws.books_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(Error::Io { path: dir, source }),
        };

        let mut books = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            // 单本书损坏不应让整个书架打不开——跳过并记日志（§5.4 的同类原则）。
            match self.load_book_at(&entry.path()) {
                Ok(b) => books.push(b),
                Err(e) => {
                    tracing::warn!(path = %entry.path().display(), error = %e, "跳过无法解析的书");
                }
            }
        }
        // 置顶的排最前、归档的沉最底，各组内按书名（§6.1）。
        books.sort_by(|a, b| {
            (a.archived, !a.pinned, &a.title).cmp(&(b.archived, !b.pinned, &b.title))
        });
        Ok(books)
    }

    /// 置顶/取消置顶一本书（§6.1 [MUST]）。
    pub fn set_book_pinned(&mut self, id: BookId, on: bool) -> Result<()> {
        let mut b = self.load_book(id)?;
        b.pinned = on;
        self.save_book_meta(&b)
    }

    /// 归档/取消归档一本书（§6.1 [MUST]）。归档不删，只是从主视图沉下去。
    pub fn set_book_archived(&mut self, id: BookId, on: bool) -> Result<()> {
        let mut b = self.load_book(id)?;
        b.archived = on;
        self.save_book_meta(&b)
    }

    pub fn load_book(&self, id: BookId) -> Result<Book> {
        self.load_book_at(&self.book_dir(id))
    }

    fn load_book_at(&self, dir: &Path) -> Result<Book> {
        let path = dir.join("book.toml");
        let text = read_to_string(&path)?;
        let bt: BookToml = toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source: Box::new(source),
        })?;

        let mut volumes = self.load_volumes(dir)?;
        volumes.sort_by_key(|v| v.order);

        Ok(Book {
            id: bt.id,
            title: bt.title,
            author: bt.author,
            synopsis: bt.synopsis,
            genre: bt.genre,
            target_words: bt.target_words,
            created: bt.created.unwrap_or_default(),
            updated: bt.updated.unwrap_or_default(),
            pinned: bt.pinned,
            archived: bt.archived,
            volumes,
            extra: bt.extra,
        })
    }

    fn load_volumes(&self, book_dir: &Path) -> Result<Vec<Volume>> {
        let vdir = book_dir.join("volumes");
        let entries = match std::fs::read_dir(&vdir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(Error::Io { path: vdir, source }),
        };

        let mut volumes = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            match self.load_volume_at(&entry.path()) {
                Ok(v) => volumes.push(v),
                Err(e) => {
                    tracing::warn!(path = %entry.path().display(), error = %e, "跳过无法解析的卷");
                }
            }
        }
        Ok(volumes)
    }

    fn load_volume_at(&self, dir: &Path) -> Result<Volume> {
        let path = dir.join("volume.toml");
        let text = read_to_string(&path)?;
        let vt: VolumeToml = toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source: Box::new(source),
        })?;

        let mut chapters = self.load_chapter_metas(dir)?;
        chapters.sort_by_key(|c| c.order);

        Ok(Volume {
            id: vt.id,
            title: vt.title,
            order: vt.order,
            synopsis: vt.synopsis,
            chapters,
            extra: vt.extra,
        })
    }

    /// 读章节的元数据。只读 front matter，不留正文（§6.1 性能：不读正文）。
    ///
    /// 注意：这里仍需读整个文件——front matter 在文件头部，但 Rust 没有
    /// 「只读前 N 行」的零成本方式。真正的性能保证在索引（§5.4，M2）。
    /// 此处的关键是**不把正文留在内存里**。
    fn load_chapter_metas(&self, vol_dir: &Path) -> Result<Vec<ChapterMeta>> {
        let cdir = vol_dir.join("chapters");
        let entries = match std::fs::read_dir(&cdir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(Error::Io { path: cdir, source }),
        };

        let mut metas = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            match self.load_chapter_meta_at(&path) {
                Ok(m) => metas.push(m),
                Err(e) => {
                    // 绝不静默丢弃：正文就在磁盘上，从树里消失会让用户以为稿子没了
                    // （§0 禁令 1 的精神——不能有静默丢正文的路径）。
                    // 降级为「受损章」：仍出现在树上，标题给出提示，但不可写
                    // （damaged=true 时 save_body 会拒绝，以免覆盖掉救得回来的内容）。
                    tracing::warn!(path = %path.display(), error = %e, "章节 front matter 损坏，降级显示");
                    metas.push(damaged_meta(&path, self.relative(&path), &e));
                }
            }
        }
        Ok(metas)
    }

    fn load_chapter_meta_at(&self, path: &Path) -> Result<ChapterMeta> {
        let raw = read_to_string(path)?;
        let file =
            ChapterFile::parse(&raw, ChapterId::generate()).map_err(|e| Error::ChapterParse {
                path: path.to_owned(),
                message: e.to_string(),
            })?;
        Ok(self.meta_from_file(&file.meta, path))
    }

    fn meta_from_file(&self, fm: &FrontMatter, path: &Path) -> ChapterMeta {
        ChapterMeta {
            id: fm.id,
            title: fm.title.clone(),
            // order 取自文件名前缀；文件名是排序的真相载体（§5.1 目录名 = 排序号 + slug）。
            order: order_from_filename(path).unwrap_or(0),
            status: fm.status,
            word_count: fm.words,
            tags: fm.tags.clone(),
            path: self.relative(path),
            updated: fm.updated.clone(),
            damaged: None,
        }
    }

    /// 转成相对 workspace 的路径。绝对路径存进元数据会让 workspace 无法搬移。
    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(self.ws.root())
            .unwrap_or(path)
            .to_path_buf()
    }

    // ---- 创建（§6.1、§6.2）----

    /// 新建书。
    pub fn create_book(&mut self, title: &str, author: &str) -> Result<Book> {
        let id = BookId::generate();
        let book = Book::new(id, title, author);
        let dir = self.book_dir(id);
        std::fs::create_dir_all(dir.join("volumes")).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        self.save_book_meta(&book)?;
        Ok(book)
    }

    /// 新建卷。`after` 为 None 时插到最前。
    pub fn create_volume(
        &mut self,
        book: BookId,
        title: &str,
        after: Option<VolumeId>,
    ) -> Result<VolumeId> {
        let mut b = self.load_book(book)?;
        let order = self.next_volume_order(&mut b, after)?;

        let id = VolumeId::generate();
        let vol = Volume::new(id, title, order);
        let dir = self.volume_dir(book, &vol);
        std::fs::create_dir_all(dir.join("chapters")).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        self.save_volume_meta(book, &vol)?;
        Ok(id)
    }

    /// 新建章。`after` 为 None 时插到卷首。
    pub fn create_chapter(
        &mut self,
        book: BookId,
        vol: VolumeId,
        title: &str,
        after: Option<ChapterId>,
    ) -> Result<ChapterId> {
        let mut b = self.load_book(book)?;
        let order = self.next_chapter_order(&mut b, book, vol, after)?;

        let id = ChapterId::generate();
        let mut fm = FrontMatter::new(id, title);
        let now = crate::now_rfc3339();
        fm.created = Some(now.clone());
        fm.updated = Some(now);

        let file = ChapterFile {
            meta: fm,
            body: String::new(),
        };
        let path = self.chapter_path(book, vol, order, title)?;
        self.write_chapter_file(&path, &file)?;
        Ok(id)
    }

    // ---- 结构管理：重命名 / 删除 / 移动（§6.1、§6.2）----
    //
    // 一条铁律（§6.2 line 319 [MUST]）：**改名/移动只动元数据与文件名，
    // 绝不碰正文、绝不重新生成 ID**。ID 是稳定真相，正文是用户的手稿——
    // 一次改名把正文动了，就是最不该发生的那种事（§0 禁令 1）。

    /// 给书改名。书目录名就是 BookId，不带 slug，故只改 `book.toml` 里的标题，
    /// 不搬任何目录。
    pub fn rename_book(&mut self, id: BookId, new_title: &str) -> Result<()> {
        let mut b = self.load_book(id)?;
        b.title = new_title.to_string();
        self.save_book_meta(&b)
    }

    /// 给卷改名。卷目录名是 `{order:03}-{slug}`，改名要连目录一起搬——
    /// 目录里的章跟着走，它们的 id 与正文都不受影响（路径下次扫盘重新导出）。
    pub fn rename_volume(&mut self, book: BookId, vol: VolumeId, new_title: &str) -> Result<()> {
        let b = self.load_book(book)?;
        let v = b
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;
        let old_dir = self.volume_dir(book, v);

        let mut nv = v.clone();
        nv.title = new_title.to_string();
        let new_dir = self.volume_dir(book, &nv);

        if old_dir != new_dir {
            std::fs::rename(&old_dir, &new_dir).map_err(|source| Error::Io {
                path: old_dir,
                source,
            })?;
        }
        // volume.toml 落到（可能已改名的）新目录里。
        self.save_volume_meta(book, &nv)
    }

    /// 给章改名。改 front matter 的标题，并把文件名的 slug 部分跟着换掉，
    /// **order 前缀、id、正文全不动**（§6.2 [MUST]）。受损章拒绝改名。
    pub fn rename_chapter(&mut self, book: BookId, ch: ChapterId, new_title: &str) -> Result<()> {
        let path = self.find_chapter_path(book, ch)?;
        let raw = read_to_string(&path)?;
        let mut file = ChapterFile::parse(&raw, ch).map_err(|e| Error::ChapterDamaged {
            path: path.clone(),
            message: e.to_string(),
        })?;

        file.meta.title = new_title.to_string();
        file.meta.updated = Some(crate::now_rfc3339());
        self.write_chapter_file(&path, &file)?;

        // 文件名 = `{order:04}-{slug}.md`，order 前缀保持，只换 slug。
        let order = order_from_filename(&path).unwrap_or(0);
        let new_path = path.with_file_name(format!("{:04}-{}.md", order, slugify(new_title)));
        if new_path != path {
            std::fs::rename(&path, &new_path).map_err(|source| Error::Io { path, source })?;
        }
        Ok(())
    }

    /// 删章：移到书内 `trash/chapters/`（§0 可撤销）。
    pub fn delete_chapter(&self, book: BookId, ch: ChapterId) -> Result<()> {
        let src = self.find_chapter_path(book, ch)?;
        let trash = self.book_dir(book).join("trash").join("chapters");
        std::fs::create_dir_all(&trash).map_err(|source| Error::Io {
            path: trash.clone(),
            source,
        })?;
        let dst = trash.join(format!("{ch}.md"));
        std::fs::rename(&src, &dst).map_err(|source| Error::Io { path: src, source })
    }

    /// 删卷：整个卷目录（连同其中的章）移到书内 `trash/volumes/<卷id>/`。
    pub fn delete_volume(&self, book: BookId, vol: VolumeId) -> Result<()> {
        let b = self.load_book(book)?;
        let v = b
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;
        let src = self.volume_dir(book, v);
        let trash = self.book_dir(book).join("trash").join("volumes");
        std::fs::create_dir_all(&trash).map_err(|source| Error::Io {
            path: trash.clone(),
            source,
        })?;
        let dst = trash.join(vol.to_string());
        std::fs::rename(&src, &dst).map_err(|source| Error::Io { path: src, source })
    }

    /// 删书：整本移到工作区级 `trash/books/<书id>/`（§0 可撤销）。
    pub fn delete_book(&self, id: BookId) -> Result<()> {
        let src = self.book_dir(id);
        if !src.exists() {
            return Err(Error::Io {
                path: src.clone(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "书不存在"),
            });
        }
        let trash = self.ws.trash_dir().join("books");
        std::fs::create_dir_all(&trash).map_err(|source| Error::Io {
            path: trash.clone(),
            source,
        })?;
        let dst = trash.join(id.to_string());
        std::fs::rename(&src, &dst).map_err(|source| Error::Io { path: src, source })
    }

    /// 移动章：调到 `target_vol` 里、排在 `after` 之后（`after` 为 None 排到卷首）。
    ///
    /// 卷内重排与跨卷移动是同一件事——都是「换个卷、换个 order、把文件搬过去」。
    /// id、标题、正文全不动，只有 order 前缀和所在目录变（§6.2 [MUST]）。
    pub fn move_chapter(
        &mut self,
        book: BookId,
        ch: ChapterId,
        target_vol: VolumeId,
        after: Option<ChapterId>,
    ) -> Result<()> {
        let src = self.find_chapter_path(book, ch)?;
        // 取这一章的标题（文件名 slug 要用它重建）。受损章不给移。
        let raw = read_to_string(&src)?;
        let file = ChapterFile::parse(&raw, ch).map_err(|e| Error::ChapterDamaged {
            path: src.clone(),
            message: e.to_string(),
        })?;
        let title = file.meta.title.clone();

        let mut b = self.load_book(book)?;
        let order = self.next_chapter_order(&mut b, book, target_vol, after)?;
        // 目标卷可能在 next_chapter_order 的 renumber 里被重排过，重新加载取准目录。
        let dst = self.chapter_path(book, target_vol, order, &title)?;
        if dst == src {
            return Ok(());
        }
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        std::fs::rename(&src, &dst).map_err(|source| Error::Io { path: src, source })
    }

    // ---- 角色卡（§6.7）----

    fn characters_dir(&self, book: BookId) -> PathBuf {
        self.book_dir(book).join("characters")
    }

    fn character_path(&self, book: BookId, id: CharacterId) -> PathBuf {
        self.characters_dir(book).join(format!("{id}.toml"))
    }

    /// 新建角色（只给名字，其余留空待编辑）。
    pub fn create_character(&mut self, book: BookId, name: &str) -> Result<Character> {
        let c = Character::new(CharacterId::generate(), name);
        self.save_character(book, &c)?;
        Ok(c)
    }

    /// 落盘一张角色卡（原子写）。
    pub fn save_character(&self, book: BookId, c: &Character) -> Result<()> {
        let dir = self.characters_dir(book);
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        let ct = CharacterToml::from_model(c);
        let text = toml::to_string(&ct).map_err(|e| Error::ChapterParse {
            path: self.character_path(book, c.id),
            message: e.to_string(),
        })?;
        crate::atomic::write(&self.character_path(book, c.id), text.as_bytes())
    }

    pub fn load_character(&self, book: BookId, id: CharacterId) -> Result<Character> {
        let path = self.character_path(book, id);
        let text = read_to_string(&path)?;
        let ct: CharacterToml = toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source: Box::new(source),
        })?;
        Ok(ct.into_model())
    }

    /// 列出一本书的全部角色，按名字排序（§6.7 列表页）。
    ///
    /// 单张卡解析失败只跳过并记日志，不连累其余——一张坏卡不该让整个角色页打不开。
    pub fn list_characters(&self, book: BookId) -> Result<Vec<Character>> {
        let dir = self.characters_dir(book);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            match read_to_string(&path).and_then(|t| {
                toml::from_str::<CharacterToml>(&t).map_err(|source| Error::ConfigParse {
                    path: path.clone(),
                    source: Box::new(source),
                })
            }) {
                Ok(ct) => out.push(ct.into_model()),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "跳过无法解析的角色卡")
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// 删除角色：移到书内 `trash/characters/`，不真删（§0：破坏性操作可撤销）。
    pub fn delete_character(&self, book: BookId, id: CharacterId) -> Result<()> {
        let src = self.character_path(book, id);
        let trash = self.book_dir(book).join("trash").join("characters");
        std::fs::create_dir_all(&trash).map_err(|source| Error::Io {
            path: trash.clone(),
            source,
        })?;
        let dst = trash.join(format!("{id}.toml"));
        std::fs::rename(&src, &dst).map_err(|source| Error::Io { path: src, source })
    }

    // ---- 正文读写（§6.2 契约）----

    /// 按需加载正文（§5.3）。
    pub fn load_body(&self, book: BookId, ch: ChapterId) -> Result<ChapterBody> {
        let path = self.find_chapter_path(book, ch)?;
        let raw = read_to_string(&path)?;
        let file = ChapterFile::parse(&raw, ch).map_err(|e| Error::ChapterParse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        // 正文已在 parse 中归一化为 LF。
        Ok(ChapterBody::new(ch, &file.body))
    }

    /// 原子写正文：tmp -> fsync -> rename -> fsync(dir)（§6.2 契约、§0 禁令 1）。
    ///
    /// 保留 front matter 的全部字段（含未知字段），只更新正文、字数与时间。
    pub fn save_body(&mut self, book: BookId, body: &ChapterBody) -> Result<()> {
        let path = self.find_chapter_path(book, body.id)?;

        let raw = read_to_string(&path)?;

        // 受损章拒绝写入：front matter 无法解析时写回等于覆盖原文件，
        // 而那份内容用户还可能人工救回（§0 禁令 1）。
        let mut file = ChapterFile::parse(&raw, body.id).map_err(|e| Error::ChapterDamaged {
            path: path.clone(),
            message: e.to_string(),
        })?;

        file.body = body.text.to_string();
        file.meta.updated = Some(crate::now_rfc3339());
        file.meta.words = Some(mj_text::count::count_han_and_punct(&file.body) as u64);

        self.write_chapter_file(&path, &file)?;

        // 正文已安全落盘，swp 使命完成。留着它下次启动会误报「有未保存的改动」，
        // 而狼来了喊多了，真出事时用户就不看了。
        // 删除失败不影响本次保存——正文已经安全，残留的 swp 下次比对内容后会被清掉。
        if let Err(e) = crate::swap::remove(&path) {
            tracing::warn!(path = %path.display(), error = %e, "清理 swp 失败");
        }
        Ok(())
    }

    /// 序列化并原子写。行尾按配置转换（ADR 0003）。
    fn write_chapter_file(&self, path: &Path, file: &ChapterFile) -> Result<()> {
        let text = file.to_text().map_err(|e| Error::ChapterParse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        let out = mj_text::eol::denormalize(&text, self.config.general.line_ending);
        crate::atomic::write(path, out.as_bytes())
    }

    // ---- 元数据落盘 ----

    fn save_book_meta(&self, book: &Book) -> Result<()> {
        let bt = BookToml {
            id: book.id,
            title: book.title.clone(),
            author: book.author.clone(),
            synopsis: book.synopsis.clone(),
            genre: book.genre.clone(),
            target_words: book.target_words,
            created: Some(book.created.clone()),
            updated: Some(crate::now_rfc3339()),
            pinned: book.pinned,
            archived: book.archived,
            extra: book.extra.clone(),
        };
        let text = toml::to_string(&bt).map_err(|e| Error::ChapterParse {
            path: self.book_dir(book.id).join("book.toml"),
            message: e.to_string(),
        })?;
        crate::atomic::write(&self.book_dir(book.id).join("book.toml"), text.as_bytes())
    }

    fn save_volume_meta(&self, book: BookId, vol: &Volume) -> Result<()> {
        let vt = VolumeToml {
            id: vol.id,
            title: vol.title.clone(),
            order: vol.order,
            synopsis: vol.synopsis.clone(),
            extra: vol.extra.clone(),
        };
        let dir = self.volume_dir(book, vol);
        let text = toml::to_string(&vt).map_err(|e| Error::ChapterParse {
            path: dir.join("volume.toml"),
            message: e.to_string(),
        })?;
        crate::atomic::write(&dir.join("volume.toml"), text.as_bytes())
    }

    // ---- 路径 ----

    fn book_dir(&self, id: BookId) -> PathBuf {
        self.ws.books_dir().join(id.to_string())
    }

    fn volume_dir(&self, book: BookId, vol: &Volume) -> PathBuf {
        self.book_dir(book).join("volumes").join(format!(
            "{:03}-{}",
            vol.order,
            slugify(&vol.title)
        ))
    }

    fn chapter_path(
        &self,
        book: BookId,
        vol: VolumeId,
        order: u32,
        title: &str,
    ) -> Result<PathBuf> {
        let b = self.load_book(book)?;
        let v = b
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;
        Ok(self.volume_dir(book, v).join("chapters").join(format!(
            "{:04}-{}.md",
            order,
            slugify(title)
        )))
    }

    /// 由 id 反查章节文件的绝对路径。
    ///
    /// 走目录扫描而非缓存：文件可能被用户在外部改名/移动（§1 纯文本为真相，
    /// 用户拿 git 或记事本操作是被鼓励的），缓存会失效，磁盘不会。
    pub fn chapter_file_path(&self, book: BookId, ch: ChapterId) -> Result<PathBuf> {
        self.find_chapter_path(book, ch)
    }

    fn find_chapter_path(&self, book: BookId, ch: ChapterId) -> Result<PathBuf> {
        let b = self.load_book(book)?;
        for v in &b.volumes {
            for c in &v.chapters {
                if c.id == ch {
                    return Ok(self.ws.root().join(&c.path));
                }
            }
        }
        Err(Error::ChapterNotFound { id: ch })
    }

    // ---- 稀疏排序（§5.3）----

    /// 求新卷的 order；中值耗尽则整卷 renumber 后重试。
    fn next_volume_order(&mut self, book: &mut Book, after: Option<VolumeId>) -> Result<u32> {
        let orders: Vec<u32> = book.volumes.iter().map(|v| v.order).collect();
        let idx = match after {
            None => None,
            Some(id) => Some(
                book.volumes
                    .iter()
                    .position(|v| v.id == id)
                    .ok_or(Error::VolumeNotFound { id })?,
            ),
        };
        let (prev, next) = neighbors(&orders, idx);

        if let Some(o) = order_between(prev, next) {
            return Ok(o);
        }

        // 中值耗尽：整卷 renumber（§5.3）。
        tracing::info!("卷 order 中值耗尽，触发 renumber");
        let mut new_orders = orders.clone();
        renumber(&mut new_orders);
        for (v, o) in book.volumes.iter_mut().zip(&new_orders) {
            v.order = *o;
        }
        self.rewrite_all_volume_meta(book)?;

        let (prev, next) = neighbors(&new_orders, idx);
        order_between(prev, next).ok_or(Error::OrderExhausted)
    }

    fn next_chapter_order(
        &mut self,
        book: &mut Book,
        book_id: BookId,
        vol: VolumeId,
        after: Option<ChapterId>,
    ) -> Result<u32> {
        let v = book
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;
        let orders: Vec<u32> = v.chapters.iter().map(|c| c.order).collect();
        let idx = match after {
            None => None,
            Some(id) => Some(
                v.chapters
                    .iter()
                    .position(|c| c.id == id)
                    .ok_or(Error::ChapterNotFound { id })?,
            ),
        };
        let (prev, next) = neighbors(&orders, idx);

        if let Some(o) = order_between(prev, next) {
            return Ok(o);
        }

        tracing::info!(volume = %vol, "章 order 中值耗尽，触发 renumber");
        self.renumber_chapters(book_id, vol)?;
        // renumber 后重新加载，拿到新的 order。
        let b = self.load_book(book_id)?;
        let v = b
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;
        let new_orders: Vec<u32> = v.chapters.iter().map(|c| c.order).collect();
        let (prev, next) = neighbors(&new_orders, idx);
        order_between(prev, next).ok_or(Error::OrderExhausted)
    }

    fn rewrite_all_volume_meta(&self, book: &Book) -> Result<()> {
        for v in &book.volumes {
            self.save_volume_meta(book.id, v)?;
        }
        Ok(())
    }

    /// 整卷 renumber：改的是文件名里的 order 前缀，正文一字不动（§6.2 [MUST]）。
    fn renumber_chapters(&mut self, book: BookId, vol: VolumeId) -> Result<()> {
        let b = self.load_book(book)?;
        let v = b
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .ok_or(Error::VolumeNotFound { id: vol })?;

        let mut orders: Vec<u32> = v.chapters.iter().map(|c| c.order).collect();
        renumber(&mut orders);

        for (c, new_order) in v.chapters.iter().zip(&orders) {
            if c.order == *new_order {
                continue;
            }
            let old = self.ws.root().join(&c.path);
            let new = self.volume_dir(book, v).join("chapters").join(format!(
                "{:04}-{}.md",
                new_order,
                slugify(&c.title)
            ));
            std::fs::rename(&old, &new).map_err(|source| Error::Io { path: old, source })?;
        }
        Ok(())
    }
}

/// 为 front matter 损坏的章造一份占位元数据。
///
/// id 由路径哈希导出而非随机：随机的话每次扫描都变，章会在树上乱跳，
/// 且「打开受损章查看原因」这类操作无法定位。路径不变则 id 不变。
fn damaged_meta(abs_path: &Path, rel_path: PathBuf, err: &Error) -> ChapterMeta {
    let digest = blake3::hash(abs_path.to_string_lossy().as_bytes());
    let raw = u64::from_le_bytes(digest.as_bytes()[..8].try_into().unwrap_or([0; 8]));

    let name = abs_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("(未知文件)");

    ChapterMeta {
        id: ChapterId::from_raw(raw),
        // 标题直接说明问题：用户在树上就能看见，不必去翻日志。
        title: format!("⚠ {name}（元数据损坏）"),
        order: order_from_filename(abs_path).unwrap_or(u32::MAX),
        status: crate::model::ChapterStatus::Draft,
        word_count: None,
        tags: Vec::new(),
        path: rel_path,
        updated: None,
        damaged: Some(err.to_string()),
    }
}

/// 取 `idx` 位置前后的 order。`idx` 为 None 表示插到最前。
fn neighbors(orders: &[u32], idx: Option<usize>) -> (Option<u32>, Option<u32>) {
    match idx {
        None => (None, orders.first().copied()),
        Some(i) => (orders.get(i).copied(), orders.get(i + 1).copied()),
    }
}

/// 从文件名前缀取 order，如 `0010-kaipian.md` -> 10。
fn order_from_filename(path: &Path) -> Option<u32> {
    path.file_name()?.to_str()?.split('-').next()?.parse().ok()
}

fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}
