// MDIO Worktree — VFS 工作区树子系统

use crate::vfs::MountMode;
use crate::backend::WorktreeInfo;
use ahash::AHashMap as HashMap;
use anyhow::{anyhow, Result};


// WT VFS: WtTree / WtNode / Binding。

/// 虚拟路径映射。my_path → target_path 方向：从本 WT 看到父 WT。
#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub my_path: String,       // "/home", "/data"
    pub target_path: String,   // "/docs", "/a1"
    pub mode: MountMode,
}


// ── Rule 数据结构 ──────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SourceKind {
    Bind,
    Remote { entry_id: String },
}

#[derive(Debug, Clone)]
pub(crate) struct RuleSegment {
    pub path: String,
    pub kind: SourceKind,
    pub mode: MountMode,
}

/// 一个 WT 的 realm 定义——完整描述 realm[bind + local + remote]
///
/// local 不在 Rule 中——local 是 storage_root 下不被 bind 或 remote 覆盖的文件。
/// bind 和 remote 是 Rule 中的段，由 path 前缀区分：
///   /share/* → Remote
///   否则     → Bind
#[derive(Debug, Clone, Default)]
pub(crate) struct Rule {
    pub segments: Vec<RuleSegment>,
}

/// worktrees.md 解析出的单条条目。
#[derive(Debug, Clone)]
pub(crate) struct WorktreeEntry {
    pub name: String,
    pub uid: String,
    pub rule: Option<String>,
    pub path: Option<String>,
    pub external_root: Option<String>,
    /// 三根分层的布局名（host 写、host 读；缺省 = 沿用 store 默认 = 旧扁平行）
    pub doc_subdir: Option<String>,
    pub wal_rel: Option<String>,
}

/// WT 树节点。
#[derive(Debug, Clone)]
pub(crate) struct WtNode {
    pub uid: String,
    pub name: String,
    pub parent: Option<String>,        // parent uid
    pub children: Vec<String>,         // child uids
    pub storage_root: String,
    pub bindings: Vec<Binding>,
    pub rule: Rule,
    pub external_root: Option<String>,
    pub doc_subdir: Option<String>,
    pub wal_rel: Option<String>,
}

/// WT 树。HashMap key = uid。
#[derive(Debug, Clone)]
pub(crate) struct WtTree {
    pub nodes: HashMap<String, WtNode>,
    pub dirty: bool,
}

impl Binding {
    pub(crate) fn parse(rule: &str) -> Result<Vec<Binding>> {
        let parts: Vec<&str> = rule.split(|c| c == ',' || c == ';' || c == ' ')
            .map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let mut out = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let segs: Vec<&str> = part.split(':').map(|s| s.trim()).collect();
            let target = segs[0];
            // 安全：拒绝源路径指向系统保护区
            // 注：/.mdio 保留名不列入（代码全在 /.mds/*）。
            if target == "/.mds" || target.starts_with("/.mds/") {
                return Err(anyhow!("restricted dot-folder: {}", target));
            }
            // alias 提取：segs[1] 若为 RO/RW 则是 mode suffix，不作为 alias
            let alias = if segs.len() > 1 && !segs[1].is_empty() {
                if segs[1].eq_ignore_ascii_case("RO") || segs[1].eq_ignore_ascii_case("RW") {
                    target.trim_end_matches('/').rsplit('/').next().unwrap_or(target)
                } else {
                    segs[1].trim_start_matches('/')
                }
            } else { target.trim_end_matches('/').rsplit('/').next().unwrap_or(target) };
            if alias.starts_with(".mds") {   // 保留点目录前缀
                return Err(anyhow!("alias cannot start with reserved dot-folder: {}", alias));
            }
            let my_path = if i == 0 { "/home".to_string() }
                          else { format!("/{}", alias) };
            // mode: segs[1] 或 segs[2] 中有 RO 即为只读
            let mode = if (segs.len() > 1 && segs[1].eq_ignore_ascii_case("RO"))
                       || (segs.len() > 2 && segs[2].eq_ignore_ascii_case("RO"))
                       { MountMode::RO } else { MountMode::RW };
            out.push(Binding { my_path, target_path: target.to_string(), mode });
        }
        Ok(out)
    }
}

// ── Rule 实现 ───────────────────────────────────────────────────

impl Rule {
    pub fn parse(text: &str) -> Self {
        let segments: Vec<RuleSegment> = text
            .split(|c: char| c == ' ' || c == ';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|part| {
                let (path, mode) = if let Some(p) = part.strip_suffix(":RO") {
                    (p.trim().to_string(), MountMode::RO)
                } else if let Some(p) = part.strip_suffix(":RW") {
                    (p.trim().to_string(), MountMode::RW)
                } else {
                    (part.to_string(), MountMode::RW)
                };
                let kind = if path.starts_with("/share/") {
                    let entry_id = path.rsplit('/').next()
                        .unwrap_or(&path).to_string();
                    SourceKind::Remote { entry_id }
                } else {
                    SourceKind::Bind
                };
                RuleSegment { path, kind, mode }
            })
            .collect();

        Rule { segments }
    }

    pub fn format(&self) -> String {
        self.segments.iter()
            .map(|s| {
                if s.mode == MountMode::RO {
                    format!("{}:RO", s.path)
                } else {
                    s.path.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn bind_text(&self) -> String {
        self.segments.iter()
            .filter(|s| matches!(s.kind, SourceKind::Bind))
            .map(|s| if s.mode == MountMode::RO {
                format!("{}:RO", s.path)
            } else {
                s.path.clone()
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn remote_entries(&self) -> Vec<(String, String)> {
        self.segments.iter()
            .filter_map(|s| {
                if let SourceKind::Remote { entry_id } = &s.kind {
                    Some((s.path.clone(), entry_id.clone()))
                } else { None }
            })
            .collect()
    }

    pub fn add_remote(&mut self, path: &str, entry_id: &str) -> Result<()> {
        if self.segments.iter().any(|s| s.path == path) {
            return Err(anyhow!("segment already exists: {}", path));
        }
        self.segments.push(RuleSegment {
            path: path.to_string(),
            kind: SourceKind::Remote { entry_id: entry_id.to_string() },
            mode: MountMode::RW,
        });
        Ok(())
    }

    pub fn remove_remote(&mut self, path: &str) {
        self.segments.retain(|s| {
            !(matches!(s.kind, SourceKind::Remote { .. }) && s.path == path)
        });
    }
}

// ── WtNode 方法 ─────────────────────────────────────────────────

pub(crate) fn path_matches(vfs_path: &str, my_path: &str) -> bool {
    vfs_path.starts_with(my_path)
        && (vfs_path.len() == my_path.len() || vfs_path.as_bytes()[my_path.len()] == b'/')
}

impl WtNode {
    /// 解析 VFS 虚拟路径。返回 (storage_key, mode, wt_uid)。
    pub fn resolve(&self, vfs_path: &str, tree: &WtTree) -> (String, MountMode, String) {
        for b in &self.bindings {
            if path_matches(vfs_path, &b.my_path) {
                let rest = &vfs_path[b.my_path.len()..];
                let full = if rest.is_empty() { b.target_path.clone() }
                           else { format!("{}/{}", b.target_path, rest.trim_start_matches('/')) };
                let parent_uid = match self.parent.as_ref() {
                    Some(p) => p,
                    // genesis 有 binding 但无 parent：用 binding mode 约束，fallback 到 local
                    None => {
                        let mode = if matches!(b.mode, MountMode::RO) { MountMode::RO } else { MountMode::RW };
                        return (format!("{}{}", self.storage_root, full.trim_start_matches('/')), mode, self.uid.clone());
                    }
                };
                let parent = match tree.nodes.get(parent_uid) {
                    Some(p) => p,
                    None => return (format!("{}{}", self.storage_root, vfs_path.trim_start_matches('/')), MountMode::RW, self.uid.clone()),
                };
                let (phys, parent_mode, owner) = parent.resolve(&full, tree);
                let mode = if matches!(b.mode, MountMode::RO) || matches!(parent_mode, MountMode::RO)
                           { MountMode::RO } else { MountMode::RW };
                return (phys, mode, owner);
            }
        }
        { let sr=self.storage_root.trim_end_matches('/'); let vfs_path=vfs_path.trim_start_matches('/'); let mut phys=String::with_capacity(sr.len()+1+vfs_path.len()); phys.push_str(sr); if !vfs_path.is_empty(){phys.push('/');phys.push_str(vfs_path);} if phys.is_empty()||phys=="/"{phys=String::from("/.root");} (phys, MountMode::RW, self.uid.clone()) }
    }

    /// 沿 parent 链上溯，返回第一个 external_root 非空的 WT（project 管辖者）。
    pub fn responsible_project<'a>(&self, tree: &'a WtTree) -> Option<&'a WtNode> {
        if self.external_root.is_some() { return tree.nodes.get(&self.uid); }
        let parent_uid = self.parent.as_ref()?;
        let parent = tree.nodes.get(parent_uid)?;
        parent.responsible_project(tree)
    }
}

// ── WtTree 查询方法 ─────────────────────────────────────────────

impl WtTree {
    pub fn parent_of(&self, uid: &str) -> Option<&str> {
        self.nodes.get(uid)?.parent.as_deref()
    }

    pub fn children_of(&self, uid: &str) -> &[String] {
        self.nodes.get(uid).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    /// 后序遍历：最深子先出，父最后。
    pub fn collect_descendants(&self, uid: &str) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(node) = self.nodes.get(uid) {
            for c_uid in &node.children {
                result.extend(self.collect_descendants(c_uid));
            }
            result.push(uid.to_string());
        }
        result
    }

    /// 按名称路径解析为 uid。如 resolve_name("a3x9k2bc", "docs/exp")。
    pub fn resolve_name(&self, from_uid: &str, path: &str) -> Option<String> {
        let mut current_uid = from_uid.to_string();
        for seg in path.trim_matches('/').split('/') {
            let node = self.nodes.get(&current_uid)?;
            let child_uid = node.children.iter()
                .find(|c_uid| self.nodes.get(*c_uid)
                    .map(|n| n.name.as_str()) == Some(seg))
                .cloned()?;
            current_uid = child_uid;
        }
        Some(current_uid)
    }

    pub fn list_all(&self, from_uid: &str, prefix: &str) -> Vec<(String, &WtNode)> {
        let mut result = Vec::new();
        if let Some(node) = self.nodes.get(from_uid) {
            for c_uid in &node.children {
                if let Some(child) = self.nodes.get(c_uid) {
                    let path = if prefix.is_empty() { child.name.clone() }
                               else { format!("{}/{}", prefix, child.name) };
                    let path_clone = path.clone();
                    result.push((path, child));
                    result.extend(self.list_all(c_uid, &path_clone));
                }
            }
        }
        result
    }

    /// BFS 顺序返回所有 WT 信息（parent-first，genesis 必在第一个）。
    pub fn list_worktrees(&self) -> Vec<WorktreeInfo> {
        let mut result = Vec::new();
        let mut queue: Vec<&str> = Vec::new();
        queue.push("genesis");
        while let Some(uid) = queue.pop() {
            if let Some(node) = self.nodes.get(uid) {
                result.push(WorktreeInfo {
                    uid: node.uid.clone(),
                    name: node.name.clone(),
                    storage_root: node.storage_root.clone(),
                    external_root: node.external_root.clone(),
                    doc_subdir: node.doc_subdir.clone(),
                    wal_rel: node.wal_rel.clone(),
                });
                for child_uid in &node.children {
                    if queue.iter().all(|&q| q != child_uid.as_str()) {
                        queue.push(child_uid);
                    }
                }
            }
        }
        result
    }
}


#[cfg(test)]
mod tests {
    // ── Rule 数据结构测试 ──────────────────────

    mod rule_unit {
        use crate::store::*;
        use super::super::*;

        #[test]
        fn parse_empty() {
            let r = Rule::parse("");
            assert!(r.segments.is_empty());
        }

        #[test]
        fn parse_single_bind() {
            let r = Rule::parse("/docs ");
            assert_eq!(r.segments.len(), 1);
            assert_eq!(r.segments[0].path, "/docs");
            assert_eq!(r.segments[0].kind, SourceKind::Bind);
            assert_eq!(r.segments[0].mode, MountMode::RW);
        }

        #[test]
        fn parse_multiple_binds() {
            let r = Rule::parse("/docs /data /public");
            assert_eq!(r.segments.len(), 3);
            assert!(r.segments.iter().all(|s| s.kind == SourceKind::Bind));
        }

        #[test]
        fn parse_remote() {
            let r = Rule::parse("/docs /share/team-a");
            assert_eq!(r.segments.len(), 2);
            assert_eq!(r.segments[0].kind, SourceKind::Bind);
            assert_eq!(r.segments[1].kind, SourceKind::Remote { entry_id: "team-a".into() });
            assert_eq!(r.segments[1].path, "/share/team-a");
        }

        #[test]
        fn parse_multiple_remotes() {
            let r = Rule::parse("/share/team-a /share/upstream /local");
            assert_eq!(r.segments.len(), 3);
            assert_eq!(r.segments[0].kind, SourceKind::Remote { entry_id: "team-a".into() });
            assert_eq!(r.segments[1].kind, SourceKind::Remote { entry_id: "upstream".into() });
            assert_eq!(r.segments[2].kind, SourceKind::Bind);
        }

        #[test]
        fn parse_ro_suffix() {
            let r = Rule::parse("/docs:RO /data:RW");
            assert_eq!(r.segments[0].mode, MountMode::RO);
            assert_eq!(r.segments[0].path, "/docs");
            assert_eq!(r.segments[1].mode, MountMode::RW);
        }

        #[test]
        fn parse_semicolon_separator() {
            let r = Rule::parse("/docs;/data;/share/remote");
            assert_eq!(r.segments.len(), 3);
            assert_eq!(r.segments[2].kind, SourceKind::Remote { entry_id: "remote".into() });
        }

        #[test]
        fn parse_remote_with_ro() {
            let r = Rule::parse("/share/team-a:RO");
            assert_eq!(r.segments[0].kind, SourceKind::Remote { entry_id: "team-a".into() });
            assert_eq!(r.segments[0].mode, MountMode::RO);
        }

        #[test]
        fn parse_naked_share_is_bind() {
            // "/share" does NOT start with "/share/" → Bind
            let r = Rule::parse("/share");
            assert_eq!(r.segments[0].kind, SourceKind::Bind);
        }

        #[test]
        fn parse_deep_share_path() {
            let r = Rule::parse("/share/org/team/project");
            assert_eq!(r.segments[0].kind, SourceKind::Remote { entry_id: "project".into() });
        }

        // ── format ──

        #[test]
        fn format_roundtrip() {
            let input = "/docs /share/team-a /data:RO";
            let r = Rule::parse(input);
            assert_eq!(r.format(), input);
        }

        #[test]
        fn format_empty() {
            assert_eq!(Rule::parse("").format(), "");
        }

        // ── bind_text ──

        #[test]
        fn bind_text_filters_remote() {
            let r = Rule::parse("/docs /share/team-a /data /share/up");
            assert_eq!(r.bind_text(), "/docs /data");
        }

        #[test]
        fn bind_text_all_bind() {
            let r = Rule::parse("/docs /data /public");
            assert_eq!(r.bind_text(), "/docs /data /public");
        }

        #[test]
        fn bind_text_all_remote_is_empty() {
            let r = Rule::parse("/share/a /share/b");
            assert!(r.bind_text().is_empty());
        }

        #[test]
        fn bind_text_feeds_binding_parse() {
            let r = Rule::parse("/docs /data:RO");
            let bt = r.bind_text();
            let bindings = Binding::parse(&bt).unwrap();
            assert_eq!(bindings.len(), 2);
            assert_eq!(bindings[0].my_path, "/home");
            assert_eq!(bindings[0].target_path, "/docs");
            assert_eq!(bindings[1].my_path, "/data");
            assert_eq!(bindings[1].target_path, "/data");
            assert_eq!(bindings[1].mode, MountMode::RO);
        }

        // ── remote_entries ──

        #[test]
        fn remote_entries_returns_remotes() {
            let r = Rule::parse("/docs /share/team-a /share/upstream");
            let re = r.remote_entries();
            assert_eq!(re.len(), 2);
            assert_eq!(re[0], ("/share/team-a".to_string(), "team-a".to_string()));
            assert_eq!(re[1], ("/share/upstream".to_string(), "upstream".to_string()));
        }

        #[test]
        fn remote_entries_empty_for_bind_only() {
            let r = Rule::parse("/docs /data");
            assert!(r.remote_entries().is_empty());
        }

        // ── add_remote / remove_remote ──

        #[test]
        fn add_remote_appends() {
            let mut r = Rule::parse("/docs");
            r.add_remote("/share/new", "new").unwrap();
            assert_eq!(r.segments.len(), 2);
            assert_eq!(r.segments[1].path, "/share/new");
            assert_eq!(r.segments[1].kind, SourceKind::Remote { entry_id: "new".into() });
        }

        #[test]
        fn add_remote_duplicate_fails() {
            let mut r = Rule::parse("/docs /share/existing");
            assert!(r.add_remote("/share/existing", "existing").is_err());
        }

        #[test]
        fn remove_remote() {
            let mut r = Rule::parse("/docs /share/team-a /data");
            r.remove_remote("/share/team-a");
            assert_eq!(r.segments.len(), 2);
            assert!(r.segments.iter().all(|s| matches!(s.kind, SourceKind::Bind)));
        }

        #[test]
        fn remove_remote_nonexistent_noop() {
            let mut r = Rule::parse("/docs /data");
            r.remove_remote("/share/nonexistent");
            assert_eq!(r.segments.len(), 2);
        }
    }
}
