//! Logical hierarchy is stored in Markdown metadata; moving a node never moves
//! or renames a file. Old flat documents remain roots without any migration.
use crate::{
    content::{self, ContentStore},
    types::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_DEPTH: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub course_id: String,
    pub kind: String,
    pub title: String,
    pub version: String,
    pub order: i64,
    pub path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTree {
    pub nodes: Vec<LibraryNode>,
    pub warnings: Vec<String>,
}

fn visible(kind: &str) -> bool {
    matches!(
        kind,
        "course" | "folder" | "section" | "knowledge" | "exploration" | "exercise"
    )
}

/// Uses only the current index and indexed documents. Malformed relationships
/// become visible roots in the returned view; this function never writes files.
pub fn list(store: &ContentStore) -> Result<LibraryTree> {
    let mut nodes = BTreeMap::<String, LibraryNode>::new();
    let mut warnings = Vec::new();
    for summary in store
        .summaries()
        .into_iter()
        .filter(|item| visible(&item.kind))
    {
        let mut node = LibraryNode {
            id: summary.id.clone(),
            parent_id: None,
            course_id: summary.id,
            kind: summary.kind,
            title: summary.title,
            version: summary.version,
            order: 0,
            path: summary.path,
        };
        match store.read(&node.id) {
            Ok(document) => {
                if !visible(&document.metadata.kind) {
                    warnings.push(format!(
                        "HIDDEN_KIND: {} 的外部类型已更改，未作为课程节点显示",
                        node.id
                    ));
                    continue;
                }
                node.kind = document.metadata.kind.clone();
                node.title = document.metadata.title.clone();
                node.version = document.version;
                node.path = document.path;
                match parent(&document.metadata) {
                    Ok(parent) => node.parent_id = parent,
                    Err(_) => warnings.push(format!(
                        "INVALID_PARENT: {} 的父节点格式无效，暂按根节点显示",
                        node.id
                    )),
                }
                match document.metadata.extra.get("order") {
                    Some(value) => match value.as_i64() {
                        Some(order) => node.order = order,
                        None => warnings.push(format!(
                            "INVALID_ORDER: {} 的排序值无效，暂按0显示",
                            node.id
                        )),
                    },
                    None => {}
                }
            }
            Err(error) => warnings.push(format!(
                "READ_FAILURE: {} 无法重新读取（{}），暂显示索引中的根节点",
                node.id, error.code
            )),
        }
        nodes.insert(node.id.clone(), node);
    }
    let ids: Vec<String> = nodes.keys().cloned().collect();
    for id in &ids {
        let node = &nodes[id];
        let warning = if node.kind == "course" && node.parent_id.is_some() {
            Some(format!(
                "COURSE_NOT_ROOT: {id} 是课程却有父节点，暂按根节点显示"
            ))
        } else if let Some(parent_id) = &node.parent_id {
            match nodes.get(parent_id) {
                None => Some(format!(
                    "MISSING_PARENT: {id} 的父节点 {parent_id} 不存在，暂按根节点显示"
                )),
                Some(parent) if parent.kind == "exercise" => Some(format!(
                    "LEAF_PARENT: {id} 不能挂在练习 {parent_id} 下，暂按根节点显示"
                )),
                _ => None,
            }
        } else {
            None
        };
        if let Some(warning) = warning {
            warnings.push(warning);
            nodes.get_mut(id).unwrap().parent_id = None;
        }
    }
    // Break every member of a cycle into a root, preserving other descendants.
    let mut complete = BTreeSet::new();
    for id in &ids {
        let mut path = Vec::<String>::new();
        let mut positions = BTreeMap::<String, usize>::new();
        let mut next = Some(id.clone());
        while let Some(cursor) = next {
            if complete.contains(&cursor) {
                break;
            }
            if let Some(start) = positions.get(&cursor).copied() {
                for cycle_id in &path[start..] {
                    warnings.push(format!(
                        "PARENT_CYCLE: {cycle_id} 存在父节点循环，暂按根节点显示"
                    ));
                    nodes.get_mut(cycle_id).unwrap().parent_id = None;
                }
                break;
            }
            positions.insert(cursor.clone(), path.len());
            path.push(cursor.clone());
            next = nodes[&cursor].parent_id.clone();
        }
        complete.extend(path);
    }
    let children = child_map(&nodes);
    let mut queue: VecDeque<(String, usize, String)> = nodes
        .values()
        .filter(|node| node.parent_id.is_none())
        .map(|node| (node.id.clone(), 1, node.id.clone()))
        .collect();
    while let Some((id, depth, mut root)) = queue.pop_front() {
        let effective_depth = if depth > MAX_DEPTH {
            warnings.push(format!("DEPTH_LIMIT: {id} 超过12层，暂按根节点显示"));
            nodes.get_mut(&id).unwrap().parent_id = None;
            root = id.clone();
            1
        } else {
            depth
        };
        nodes.get_mut(&id).unwrap().course_id = root.clone();
        if let Some(children) = children.get(&id) {
            queue.extend(
                children
                    .iter()
                    .map(|child| (child.clone(), effective_depth + 1, root.clone())),
            );
        }
    }
    let mut nodes: Vec<_> = nodes.into_values().collect();
    nodes.sort_by(|a, b| {
        a.parent_id
            .cmp(&b.parent_id)
            .then(a.order.cmp(&b.order))
            .then(a.title.cmp(&b.title))
            .then(a.id.cmp(&b.id))
    });
    Ok(LibraryTree { nodes, warnings })
}

/// Strict resolution for side-effect scopes: malformed/missing/cyclic ancestry
/// is an error rather than silently widening the effective course scope.
pub fn course_id(store: &ContentStore, id: &str) -> Result<String> {
    Ok(ancestry(store, id)?.last().unwrap().id.clone())
}

/// Explicit new-course entry point; legacy create remains migration-free.
pub fn create_project(
    store: &mut ContentStore,
    title: &str,
    body: &str,
    request_id: &str,
) -> Result<Document> {
    create_impl(store, "course", title, None, body, request_id, true)
}

pub fn create(
    store: &mut ContentStore,
    kind: &str,
    title: &str,
    parent_id: Option<&str>,
    body: &str,
    request_id: &str,
) -> Result<Document> {
    create_impl(store, kind, title, parent_id, body, request_id, false)
}

fn create_impl(
    store: &mut ContentStore,
    kind: &str,
    title: &str,
    parent_id: Option<&str>,
    body: &str,
    request_id: &str,
    project: bool,
) -> Result<Document> {
    if !matches!(kind, "course" | "folder" | "section" | "exploration")
        || title.trim().is_empty()
        || title.len() > 500
        || body.len() > 1_500_000
        || request_id.len() < 8
        || request_id.len() > 128
        || request_id.chars().any(char::is_control)
    {
        return Err(invalid("节点类型、标题、正文大小或防重复编号无效。"));
    }
    if let Some(parent) = parent_id {
        checked_id(parent)?;
    }
    if kind == "course" && parent_id.is_some() {
        return Err(hierarchy("课程只能位于根层级。"));
    }
    let title = title.trim();
    let id = format!("node_{}", &hash(request_id)[..32]);
    let digest = hash(&serde_json::to_string(
        &json!({"kind":kind,"title":title,"parentId":parent_id,"body":body}),
    )?);
    let relative = if project {
        crate::course_project::material_path(&id, &id)?
    } else if let Some(parent) = parent_id {
        let document = store.read(parent)?;
        if document.path.split('/').nth(2) == Some("materials") {
            crate::course_project::material_path(&course_id(store, parent)?, &id)?
        } else {
            format!("courses/{id}.md")
        }
    } else {
        format!("courses/{id}.md")
    };
    let existing = match store.read(&id) {
        Ok(document) => Some(document),
        Err(error) if error.code == "NOT_FOUND" => {
            if store
                .root()
                .join(&relative)
                .try_exists()
                .map_err(CoreError::io)?
            {
                Some(store.register(&relative)?)
            } else {
                None
            }
        }
        Err(error) => return Err(error),
    };
    if let Some(existing) = existing {
        if existing.id != id
            || existing
                .metadata
                .extra
                .get("createRequestDigest")
                .and_then(Value::as_str)
                != Some(digest.as_str())
        {
            return Err(CoreError::new(
                "CONFLICT",
                "此防重复编号已用于另一节点或另一份内容。",
            ));
        }
        return Ok(existing);
    }
    let depth = validate_parent(store, &id, kind, parent_id)?;
    if depth + 1 > MAX_DEPTH {
        return Err(hierarchy("节点层级最多12层。"));
    }
    let tree = list(store)?;
    let order = tree
        .nodes
        .iter()
        .filter(|node| node.parent_id.as_deref() == parent_id)
        .map(|node| node.order)
        .max()
        .unwrap_or(-1)
        .checked_add(1)
        .ok_or_else(|| hierarchy("同级排序值已达到上限。"))?;
    let mut extra = BTreeMap::new();
    extra.insert("parentId".into(), json!(parent_id));
    extra.insert("order".into(), json!(order));
    extra.insert("createRequestDigest".into(), json!(digest));
    let metadata = Metadata {
        id,
        title: title.into(),
        kind: kind.into(),
        goals: vec![],
        tags: vec![],
        links: vec![],
        version: 1,
        extra,
    };
    store.create(metadata, body, &relative)
}

pub fn move_node(
    store: &mut ContentStore,
    id: &str,
    parent_id: Option<&str>,
    order: i64,
    expected_version: &str,
) -> Result<Document> {
    checked_id(id)?;
    let current = store.read(id)?;
    if !visible(&current.metadata.kind) {
        return Err(hierarchy("该文档不是可移动的课程节点。"));
    }
    if current.version != expected_version {
        return Err(CoreError::new(
            "CONFLICT",
            "节点已被其他操作更新，请刷新后再移动。",
        ));
    }
    if current.path.split('/').nth(2) == Some("materials") && current.metadata.kind != "course" {
        let target_course = parent_id
            .map(|parent| course_id(store, parent))
            .transpose()?;
        if target_course.as_deref() != Some(course_id(store, id)?.as_str()) {
            return Err(hierarchy("独立课程材料不能通过重新挂载跨课程移动。"));
        }
    }
    let parent_depth = validate_parent(store, id, &current.metadata.kind, parent_id)?;
    let height = subtree_height(store, id)?;
    if parent_depth + height > MAX_DEPTH {
        return Err(hierarchy("移动后节点及其子节点会超过12层。"));
    }
    let mut metadata = current.metadata.clone();
    metadata.extra.insert("parentId".into(), json!(parent_id));
    metadata.extra.insert("order".into(), json!(order));
    let content = content::serialize(&metadata, &current.body)?;
    store.save(id, &content, expected_version)
}

fn checked_id(id: &str) -> Result<()> {
    if !valid_id(id) {
        return Err(invalid("节点编号必须是有效稳定ID，不能使用文件路径。"));
    }
    Ok(())
}
fn parent(metadata: &Metadata) -> Result<Option<String>> {
    match metadata.extra.get("parentId") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if valid_id(id) => Ok(Some(id.clone())),
        _ => Err(hierarchy("Markdown中的parentId不是有效稳定ID。")),
    }
}
/// The returned documents are the exact versions whose metadata resolved scope.
pub(crate) fn ancestry(store: &ContentStore, id: &str) -> Result<Vec<Document>> {
    checked_id(id)?;
    let mut result = Vec::new();
    let mut visited = BTreeSet::new();
    let mut next = Some(id.to_owned());
    while let Some(id) = next {
        if !visited.insert(id.clone()) {
            return Err(hierarchy("父节点链存在循环。"));
        }
        if result.len() == MAX_DEPTH {
            return Err(hierarchy("父节点链超过12层。"));
        }
        let document = store.read(&id)?;
        if !visible(&document.metadata.kind) {
            return Err(hierarchy("父节点链包含非课程文档。"));
        }
        if !result.is_empty() && document.metadata.kind == "exercise" {
            return Err(hierarchy("练习是叶节点，不能作为父节点。"));
        }
        next = parent(&document.metadata)?;
        if document.metadata.kind == "course" && next.is_some() {
            return Err(hierarchy("课程只能位于根层级。"));
        }
        result.push(document);
    }
    Ok(result)
}
fn validate_parent(
    store: &ContentStore,
    id: &str,
    kind: &str,
    parent_id: Option<&str>,
) -> Result<usize> {
    if kind == "course" && parent_id.is_some() {
        return Err(hierarchy("课程只能位于根层级。"));
    }
    if let Some(parent_id) = parent_id {
        checked_id(parent_id)?;
        let chain = ancestry(store, parent_id)?;
        if chain[0].metadata.kind == "exercise" {
            return Err(hierarchy("练习是叶节点，不能作为父节点。"));
        }
        if chain.iter().any(|node| node.id == id) {
            return Err(hierarchy("不能把节点移动到自身或其后代下。"));
        }
        Ok(chain.len())
    } else {
        Ok(0)
    }
}
fn child_map(nodes: &BTreeMap<String, LibraryNode>) -> BTreeMap<String, Vec<String>> {
    let mut children = BTreeMap::<String, Vec<String>>::new();
    for node in nodes.values() {
        if let Some(parent_id) = &node.parent_id {
            children
                .entry(parent_id.clone())
                .or_default()
                .push(node.id.clone());
        }
    }
    children
}
fn subtree_height(store: &ContentStore, id: &str) -> Result<usize> {
    let mut children = BTreeMap::<String, Vec<String>>::new();
    for summary in store
        .summaries()
        .into_iter()
        .filter(|node| visible(&node.kind))
    {
        if summary.id == id {
            continue;
        }
        let document = store.read(&summary.id)?;
        if let Ok(Some(parent_id)) = parent(&document.metadata) {
            children.entry(parent_id).or_default().push(document.id);
        }
    }
    let mut stack = vec![(id.to_owned(), 1)];
    let mut visited = BTreeSet::new();
    let mut height = 1;
    while let Some((id, depth)) = stack.pop() {
        if !visited.insert(id.clone()) {
            return Err(hierarchy("子节点存在循环。"));
        }
        height = height.max(depth);
        if depth > MAX_DEPTH {
            return Err(hierarchy("现有子节点超过12层，不能移动整棵子树。"));
        }
        if let Some(children) = children.get(&id) {
            stack.extend(children.iter().map(|child| (child.clone(), depth + 1)));
        }
    }
    Ok(height)
}
fn invalid(message: &str) -> CoreError {
    CoreError::new("INVALID_INPUT", message)
}
fn hierarchy(message: &str) -> CoreError {
    CoreError::new("INVALID_HIERARCHY", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, ContentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        (dir, store)
    }
    fn legacy(store: &mut ContentStore, id: &str, kind: &str, parent_id: Option<&str>) -> Document {
        let mut extra = BTreeMap::new();
        if let Some(parent_id) = parent_id {
            extra.insert("parentId".into(), json!(parent_id));
        }
        store
            .create(
                Metadata {
                    id: id.into(),
                    title: id.into(),
                    kind: kind.into(),
                    goals: vec![],
                    tags: vec![],
                    links: vec![],
                    version: 1,
                    extra,
                },
                "Untouched original body",
                &format!("old/{id}.md"),
            )
            .unwrap()
    }
    #[test]
    fn nested_creation_reopen_and_reorder_preserve_ids_paths_body_and_history() {
        let (dir, mut store) = store();
        let course = create(
            &mut store,
            "course",
            "English",
            None,
            "Course body",
            "create-course-1",
        )
        .unwrap();
        let folder = create(
            &mut store,
            "folder",
            "Reading",
            Some(&course.id),
            "",
            "create-folder-1",
        )
        .unwrap();
        let section = create(
            &mut store,
            "section",
            "Lesson",
            Some(&folder.id),
            "Keep this body",
            "create-section-1",
        )
        .unwrap();
        assert_eq!(course_id(&store, &section.id).unwrap(), course.id);
        let moved = move_node(
            &mut store,
            &section.id,
            Some(&course.id),
            -10,
            &section.version,
        )
        .unwrap();
        assert_eq!(moved.id, section.id);
        assert_eq!(moved.path, section.path);
        assert_eq!(moved.body, section.body);
        assert_eq!(
            moved.metadata.extra["createdAt"],
            section.metadata.extra["createdAt"]
        );
        assert!(dir
            .path()
            .join("runtime/history")
            .join(&section.id)
            .join(format!("{}.md", section.version))
            .is_file());
        drop(store);
        let mut reopened =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        reopened.refresh().unwrap();
        let tree = list(&reopened).unwrap();
        assert!(tree.warnings.is_empty());
        let node = tree
            .nodes
            .iter()
            .find(|node| node.id == section.id)
            .unwrap();
        assert_eq!(node.parent_id.as_deref(), Some(course.id.as_str()));
        assert_eq!(node.order, -10);
        assert_eq!(node.course_id, course.id);
        assert_eq!(reopened.read(&section.id).unwrap().body, "Keep this body");
        let replay = create(
            &mut reopened,
            "section",
            "Lesson",
            Some(&folder.id),
            "Keep this body",
            "create-section-1",
        )
        .unwrap();
        assert_eq!(replay.version, moved.version);
    }
    #[test]
    fn create_is_idempotent_even_before_refresh_and_changed_payload_conflicts() {
        let (dir, mut store) = store();
        let first = create(
            &mut store,
            "section",
            "First",
            None,
            "Original",
            "idempotent-create",
        )
        .unwrap();
        assert_eq!(
            create(
                &mut store,
                "section",
                "First",
                None,
                "Original",
                "idempotent-create"
            )
            .unwrap()
            .version,
            first.version
        );
        assert_eq!(
            create(
                &mut store,
                "section",
                "Changed",
                None,
                "Original",
                "idempotent-create"
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        drop(store);
        let mut reopened =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert!(reopened.summaries().is_empty());
        assert_eq!(
            create(
                &mut reopened,
                "section",
                "First",
                None,
                "Original",
                "idempotent-create"
            )
            .unwrap()
            .id,
            first.id
        );
        assert_eq!(reopened.summaries().len(), 1);
    }
    #[test]
    fn moves_reject_stale_versions_cycles_missing_parents_and_leaf_parents() {
        let (_dir, mut store) = store();
        let root = create(&mut store, "folder", "Root", None, "", "move-root-folder").unwrap();
        let child = create(
            &mut store,
            "section",
            "Child",
            Some(&root.id),
            "Child body",
            "move-child-section",
        )
        .unwrap();
        assert_eq!(
            move_node(&mut store, &root.id, Some(&child.id), 0, &root.version)
                .unwrap_err()
                .code,
            "INVALID_HIERARCHY"
        );
        assert!(move_node(&mut store, &child.id, Some("missing"), 0, &child.version).is_err());
        let exercise = legacy(&mut store, "exercise_leaf", "exercise", None);
        assert_eq!(
            move_node(&mut store, &child.id, Some(&exercise.id), 0, &child.version)
                .unwrap_err()
                .code,
            "INVALID_HIERARCHY"
        );
        let moved = move_node(&mut store, &child.id, None, 4, &child.version).unwrap();
        assert_eq!(
            move_node(&mut store, &child.id, None, 5, &child.version)
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(store.read(&child.id).unwrap().version, moved.version);
        assert_eq!(
            create(
                &mut store,
                "course",
                "Bad",
                Some(&root.id),
                "",
                "course-must-be-root"
            )
            .unwrap_err()
            .code,
            "INVALID_HIERARCHY"
        );
        assert!(create(
            &mut store,
            "knowledge",
            "No UI create",
            None,
            "",
            "unsupported-ui-kind"
        )
        .is_err());
        assert!(move_node(&mut store, "../outside", None, 0, "").is_err());
    }
    #[test]
    fn list_repairs_only_its_view_for_bad_parents_cycles_and_legacy_roots() {
        let (_dir, mut store) = store();
        let legacy_section = legacy(&mut store, "old_section", "section", None);
        let legacy_knowledge = legacy(&mut store, "old_knowledge", "knowledge", None);
        let orphan = legacy(&mut store, "orphan", "section", Some("missing"));
        let cycle_a = legacy(&mut store, "cycle_a", "folder", Some("cycle_b"));
        let cycle_b = legacy(&mut store, "cycle_b", "folder", Some("cycle_a"));
        let before: BTreeMap<_, _> = store
            .summaries()
            .into_iter()
            .map(|item| {
                let doc = store.read(&item.id).unwrap();
                (doc.id, doc.content)
            })
            .collect();
        let tree = list(&store).unwrap();
        for doc in [
            &legacy_section,
            &legacy_knowledge,
            &orphan,
            &cycle_a,
            &cycle_b,
        ] {
            let node = tree.nodes.iter().find(|node| node.id == doc.id).unwrap();
            assert!(node.parent_id.is_none());
            assert_eq!(node.course_id, doc.id);
            assert_eq!(store.read(&doc.id).unwrap().content, before[&doc.id]);
        }
        assert!(tree
            .warnings
            .iter()
            .any(|warning| warning.starts_with("MISSING_PARENT:")));
        assert!(tree
            .warnings
            .iter()
            .any(|warning| warning.starts_with("PARENT_CYCLE:")));
        assert!(course_id(&store, &orphan.id).is_err());
        assert!(course_id(&store, &cycle_a.id).is_err());
        assert_eq!(
            course_id(&store, &legacy_section.id).unwrap(),
            legacy_section.id
        );
    }
    #[test]
    fn depth_checks_include_the_whole_moved_subtree() {
        let (_dir, mut store) = store();
        let root = create(&mut store, "folder", "Root", None, "", "depth-root-folder").unwrap();
        let mut parent_id = root.id.clone();
        for depth in 2..=12 {
            parent_id = create(
                &mut store,
                "folder",
                &format!("Depth {depth}"),
                Some(&parent_id),
                "",
                &format!("depth-node-{depth}"),
            )
            .unwrap()
            .id;
        }
        assert_eq!(
            create(
                &mut store,
                "section",
                "Too deep",
                Some(&parent_id),
                "",
                "depth-overflow-node"
            )
            .unwrap_err()
            .code,
            "INVALID_HIERARCHY"
        );
        let other = create(&mut store, "folder", "Other", None, "", "depth-other-root").unwrap();
        assert_eq!(
            move_node(&mut store, &root.id, Some(&other.id), 0, &root.version)
                .unwrap_err()
                .code,
            "INVALID_HIERARCHY"
        );
        let depth12 = store.read(&parent_id).unwrap();
        assert_eq!(course_id(&store, &parent_id).unwrap(), root.id);
        assert_eq!(store.read(&parent_id).unwrap().content, depth12.content);
    }
}
