//! Git plumbing over frozen, course-owned bytes. No worktree, index, checkout,
//! material commands, network, UI or model/session acceptance state is involved.
use crate::{
    content::ContentStore,
    types::{valid_id, CoreError, Document, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub course_id: String,
    pub checkpoint_commit_oid: String,
    pub material_tree_oid: String,
    /// Exclusions recorded by this commit, outside the material content tree.
    #[serde(default)]
    pub excluded_files: Vec<ExcludedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedFile {
    pub asset_id: String,
    pub file_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MaterialFile {
    /// Relative to this course's material tree, never a host path.
    pub path: String,
    pub blob_oid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// Git name-status, with rename detection disabled (a move is D + A).
    pub status: String,
    /// None for Git's binary '-' numstat, never a made-up line count.
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    pub binary: bool,
    /// Complete UTF8 Git patch for a focused file; see patch_bytes otherwise.
    pub patch: Option<String>,
    /// Complete native patch when Git emits non-UTF8 bytes; never lossy decoded.
    pub patch_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialDiff {
    /// First comparison uses Git's empty tree; read() supplies full initial bytes.
    pub initial: bool,
    pub changes: Vec<FileChange>,
}

/// Private entries prevent callers from smuggling arbitrary host files into Git.
#[derive(Debug, PartialEq, Eq)]
pub struct FrozenMaterials {
    course_id: String,
    entries: BTreeMap<String, Vec<u8>>,
    excluded_files: Vec<ExcludedFile>,
}

impl FrozenMaterials {
    /// Call under the application's Core lock, after refreshing its index.
    /// Two equal captures reject observed external edits; this is not a filesystem
    /// transaction. Once returned, later edits cannot alter these owned bytes.
    /// Each stored document is the exact version used to resolve its ancestry.
    pub fn capture(store: &ContentStore, course_id: &str) -> Result<Self> {
        Self::capture_with(store, course_id, |id| {
            crate::library_tree::ancestry(store, id)
        })
    }

    fn capture_with(
        store: &ContentStore,
        course_id: &str,
        mut resolve: impl FnMut(&str) -> Result<Vec<Document>>,
    ) -> Result<Self> {
        let first = Self::collect(store, course_id, &mut resolve)?;
        if first != Self::collect(store, course_id, &mut resolve)? {
            return Err(conflict());
        }
        Ok(first)
    }

    fn collect(
        store: &ContentStore,
        course_id: &str,
        resolve: &mut impl FnMut(&str) -> Result<Vec<Document>>,
    ) -> Result<Self> {
        check_id(course_id)?;
        let mut entries = BTreeMap::new();
        let mut remaining = 64 * 1024 * 1024_usize;
        let mut insert = |path, bytes: Vec<u8>| -> Result<()> {
            if let Some(previous) = entries.get(&path) {
                return if previous == &bytes {
                    Ok(())
                } else {
                    Err(conflict())
                };
            }
            remaining = remaining.checked_sub(bytes.len()).ok_or_else(limit)?;
            if entries.len() >= 4096 {
                return Err(limit());
            }
            entries.insert(path, bytes);
            Ok(())
        };
        let ids = std::iter::once(course_id.to_owned()).chain(
            store
                .summaries()
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.kind.as_str(),
                        "course" | "folder" | "section" | "knowledge" | "exploration" | "exercise"
                    )
                })
                .map(|s| s.id)
                .filter(|id| id != course_id),
        );
        for id in ids {
            // Scope and stored bytes come from the SAME strict ancestry result.
            let chain = resolve(&id)?;
            let root = &chain.last().ok_or_else(scope)?.id;
            if id == course_id && root != course_id {
                return Err(scope());
            }
            if root != course_id {
                continue;
            }
            for document in chain {
                if document.path != format!("courses/{}.md", document.id)
                    && !(document.metadata.kind == "exercise" && document.path == format!("exercises/{}.md", document.id)) {
                    return Err(scope());
                }
                insert(
                    format!("documents/{}.md", document.id),
                    document.content.into_bytes(),
                )?;
            }
        }
        let mut excluded_files = Vec::new();
        for (asset, bytes) in crate::assets::checkpoint_bytes(store, course_id)? {
            if let Some(reason) = exclusion_reason(&asset.file_name) {
                excluded_files.push(ExcludedFile {
                    asset_id: asset.id,
                    file_name: asset.file_name,
                    reason: reason.into(),
                });
                continue;
            }
            let path = attachment_path(&asset.id, &asset.file_name)?;
            insert(path, bytes)?;
        }
        Ok(Self {
            course_id: course_id.into(),
            entries,
            excluded_files,
        })
    }
}

pub struct MaterialGit {
    directory: PathBuf,
}

impl MaterialGit {
    /// Explicit opt-in only. The parent must exist; never initializes an existing
    /// directory or a directory nested in a software source Git repository.
    pub fn create_new(directory: &Path) -> Result<Self> {
        let parent = directory.parent().ok_or_else(scope)?.canonicalize()?;
        let name = directory
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(scope)?;
        check_path(name)?;
        if !directory.is_absolute()
            || directory
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(scope());
        }
        for ancestor in parent.ancestors() {
            if ancestor.join(".git").exists()
                || ancestor.join("objects").is_dir() && ancestor.join("HEAD").exists()
            {
                return Err(scope());
            }
        }
        let directory = parent.join(name);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let repo = Self { directory };
        // An interrupted initialization is left intact, never silently reset.
        repo.git(
            &["init", "--bare", "--template=", "--object-format=sha1", "."],
            &[],
        )?;
        fs::write(repo.directory.join("learning-materials-v1"), b"1\n")?;
        Ok(repo)
    }

    pub fn open(directory: &Path) -> Result<Self> {
        if fs::symlink_metadata(directory)?.file_type().is_symlink() {
            return Err(scope());
        }
        let repo = Self {
            directory: directory.canonicalize()?,
        };
        if fs::read(repo.directory.join("learning-materials-v1"))? != b"1\n"
            || repo.text(&["rev-parse", "--is-bare-repository"], &[])? != "true"
        {
            return Err(scope());
        }
        Ok(repo)
    }

    /// Returns success only after Git durably creates a reachable immutable ref.
    /// On failure, callers retain the old receipt/baseline and may retry the same
    /// frozen input. Unreachable objects left by Git are harmless, not receipts.
    pub fn commit(&self, frozen: &FrozenMaterials, parent: Option<&str>) -> Result<Checkpoint> {
        let mut blobs = Vec::new();
        for (path, bytes) in &frozen.entries {
            check_path(path)?;
            blobs.push((
                path.clone(),
                self.text(&["hash-object", "-w", "--no-filters", "--stdin"], bytes)?,
            ));
        }
        let material = self.tree(&blobs)?;
        let courses = self.tree_entries(&[("tree", &material, &frozen.course_id)])?;
        let report = self.text(
            &["hash-object", "-w", "--no-filters", "--stdin"],
            &serde_json::to_vec(&frozen.excluded_files)?,
        )?;
        let reports =
            self.tree_entries(&[("blob", &report, &format!("{}.json", frozen.course_id))])?;
        let root = self.tree_entries(&[
            ("tree", &courses, "materials"),
            ("tree", &reports, "material-exclusions"),
        ])?;
        let mut args = vec!["commit-tree", &root];
        if let Some(parent) = parent {
            self.checkpoint(&frozen.course_id, parent)?;
            args.extend(["-p", parent]);
        }
        // Neither the message nor any blob contains its own commit OID.
        let oid = self.text(&args, b"Course materials checkpoint\n")?;
        check_oid(&oid)?;
        self.git(
            &[
                "update-ref",
                &format!("refs/checkpoints/{}/{}", frozen.course_id, oid),
                &oid,
            ],
            &[],
        )?;
        self.checkpoint(&frozen.course_id, &oid)
    }

    /// Derives material identity from the commit, rather than trusting metadata.
    pub fn checkpoint(&self, course_id: &str, commit_oid: &str) -> Result<Checkpoint> {
        check_id(course_id)?;
        check_oid(commit_oid)?;
        if self.text(&["cat-file", "-t", commit_oid], &[])? != "commit" {
            return Err(scope());
        }
        let material_tree_oid = self.text(
            &[
                "rev-parse",
                "--verify",
                &format!("{commit_oid}:materials/{course_id}"),
            ],
            &[],
        )?;
        check_oid(&material_tree_oid)?;
        if self.text(&["cat-file", "-t", &material_tree_oid], &[])? != "tree" {
            return Err(scope());
        }
        let report_path = format!("material-exclusions/{course_id}.json");
        let report = self.git(&["ls-tree", "-z", commit_oid, "--", &report_path], &[])?;
        let excluded_files = if report.is_empty() {
            Vec::new() // Original v1 checkpoints had no exclusion report.
        } else {
            let row = std::str::from_utf8(&report).map_err(|_| scope())?;
            let (header, name) = row.split_once('\t').ok_or_else(scope)?;
            let parts: Vec<_> = header.split(' ').collect();
            if parts.len() != 3
                || parts[0] != "100644"
                || parts[1] != "blob"
                || name != format!("{report_path}\0")
            {
                return Err(scope());
            }
            check_oid(parts[2])?;
            serde_json::from_slice(&self.git(&["cat-file", "blob", parts[2]], &[])?)?
        };
        Ok(Checkpoint {
            course_id: course_id.into(),
            checkpoint_commit_oid: commit_oid.into(),
            material_tree_oid,
            excluded_files,
        })
    }

    pub fn files(&self, course_id: &str, commit_oid: &str) -> Result<Vec<MaterialFile>> {
        let cp = self.checkpoint(course_id, commit_oid)?;
        let raw = self.git(&["ls-tree", "-rz", &cp.material_tree_oid], &[])?;
        let mut files = Vec::new();
        for record in raw.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let record = std::str::from_utf8(record).map_err(|_| scope())?;
            let (header, path) = record.split_once('\t').ok_or_else(scope)?;
            let parts: Vec<_> = header.split(' ').collect();
            if parts.len() != 3 || parts[0] != "100644" || parts[1] != "blob" {
                return Err(scope());
            }
            check_path(path)?;
            check_oid(parts[2])?;
            files.push(MaterialFile {
                path: path.into(),
                blob_oid: parts[2].into(),
            });
        }
        Ok(files)
    }

    /// Exact blob bytes, including binary or text beyond preview limits.
    pub fn read(&self, course_id: &str, commit_oid: &str, path: &str) -> Result<Vec<u8>> {
        check_path(path)?;
        let file = self
            .files(course_id, commit_oid)?
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(scope)?;
        self.git(&["cat-file", "blob", &file.blob_oid], &[])
    }

    pub fn compare(
        &self,
        course_id: &str,
        old: Option<&str>,
        new: &str,
        focus: Option<&str>,
    ) -> Result<MaterialDiff> {
        if let Some(focus) = focus {
            check_path(focus)?;
        }
        let new = self.checkpoint(course_id, new)?.material_tree_oid;
        let old_tree = match old {
            Some(oid) => self.checkpoint(course_id, oid)?.material_tree_oid,
            None => self.tree_entries(&[])?,
        };
        let diff = |flags: &[&str], path: Option<&str>| {
            let mut args = vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--no-color",
            ];
            args.extend_from_slice(flags);
            args.extend([old_tree.as_str(), new.as_str(), "--"]);
            if let Some(path) = path {
                args.push(path);
            }
            self.git(&args, &[])
        };
        let status = diff(&["--name-status", "-z"], None)?;
        let stats = diff(&["--numstat", "-z"], None)?;
        let mut counts = BTreeMap::new();
        for row in stats.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let text = std::str::from_utf8(row).map_err(|_| scope())?;
            let mut parts = text.splitn(3, '\t');
            let a = parts.next().ok_or_else(scope)?;
            let d = parts.next().ok_or_else(scope)?;
            let path = parts.next().ok_or_else(scope)?;
            let number = |s: &str| {
                if s == "-" {
                    Ok(None)
                } else {
                    s.parse::<u64>().map(Some).map_err(|_| scope())
                }
            };
            counts.insert(path.to_owned(), (number(a)?, number(d)?));
        }
        let fields: Vec<_> = status
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        if fields.len() % 2 != 0 {
            return Err(scope());
        }
        let mut changes = Vec::new();
        for pair in fields.chunks_exact(2) {
            let status = std::str::from_utf8(pair[0])
                .map_err(|_| scope())?
                .to_owned();
            let path = std::str::from_utf8(pair[1])
                .map_err(|_| scope())?
                .to_owned();
            check_path(&path)?;
            let (added, deleted) = counts.remove(&path).ok_or_else(scope)?;
            let binary = added.is_none() || deleted.is_none();
            let (patch, patch_bytes) = if !binary && focus == Some(path.as_str()) {
                match String::from_utf8(diff(&["--patch", "--full-index"], Some(&path))?) {
                    Ok(text) => (Some(text), None),
                    Err(error) => (None, Some(error.into_bytes())),
                }
            } else {
                (None, None)
            };
            changes.push(FileChange {
                path,
                status,
                added,
                deleted,
                binary,
                patch,
                patch_bytes,
            });
        }
        Ok(MaterialDiff {
            initial: old.is_none(),
            changes,
        })
    }

    fn tree(&self, blobs: &[(String, String)]) -> Result<String> {
        let mut directories = BTreeMap::<&str, Vec<(String, String)>>::new();
        let mut entries = Vec::<(&str, String, String)>::new();
        for (path, oid) in blobs {
            if let Some((dir, rest)) = path.split_once('/') {
                directories
                    .entry(dir)
                    .or_default()
                    .push((rest.into(), oid.clone()));
            } else {
                entries.push(("blob", oid.clone(), path.clone()));
            }
        }
        for (dir, children) in directories {
            entries.push(("tree", self.tree(&children)?, dir.into()));
        }
        self.tree_entries(
            &entries
                .iter()
                .map(|(t, o, n)| (*t, o.as_str(), n.as_str()))
                .collect::<Vec<_>>(),
        )
    }

    fn tree_entries(&self, entries: &[(&str, &str, &str)]) -> Result<String> {
        let mut input = Vec::new();
        for (kind, oid, name) in entries {
            input.extend_from_slice(
                format!(
                    "{} {kind} {oid}\t{name}\0",
                    if *kind == "tree" { "040000" } else { "100644" }
                )
                .as_bytes(),
            );
        }
        self.text(&["mktree", "-z"], &input)
    }

    fn text(&self, args: &[&str], input: &[u8]) -> Result<String> {
        String::from_utf8(self.git(args, input)?)
            .map(|s| s.trim().to_owned())
            .map_err(|_| git_error())
    }

    fn git(&self, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_ALLOW_PROTOCOL", "")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("LC_ALL", "C")
            .env("GIT_AUTHOR_NAME", "Learning Station")
            .env("GIT_AUTHOR_EMAIL", "checkpoint@localhost")
            .env("GIT_COMMITTER_NAME", "Learning Station")
            .env("GIT_COMMITTER_EMAIL", "checkpoint@localhost")
            .current_dir(&self.directory)
            .args([
                "--no-pager",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.fsync=committed,reference",
                "-c",
                "core.fsyncMethod=fsync",
                "-c",
                "gc.auto=0",
                "-c",
                "protocol.allow=never",
            ])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| git_error())?;
        // Input commands (hash-object/mktree/commit-tree) emit only one OID.
        // Dropping stdin closes it before waiting, including a failed write.
        let written = child.stdin.take().ok_or_else(git_error)?.write_all(input);
        let output = child.wait_with_output();
        written.map_err(|_| git_error())?;
        let output = output.map_err(|_| git_error())?;
        if !output.status.success() {
            return Err(git_error());
        }
        Ok(output.stdout)
    }
}

fn check_id(id: &str) -> Result<()> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(scope())
    }
}
fn check_oid(oid: &str) -> Result<()> {
    if matches!(oid.len(), 40 | 64)
        && oid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(scope())
    }
}
fn check_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|p| p.is_empty() || p.starts_with('.') || p == "..")
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err(scope())
    } else {
        Ok(())
    }
}
/// Canonical logical path, including existing attachment names containing ':'.
pub fn attachment_path(asset_id: &str, file_name: &str) -> Result<String> {
    check_id(asset_id)?;
    if file_name.contains(['/', '\\']) || file_name.contains("..") {
        return Err(scope());
    }
    let encoded = file_name.replace('%', "%25").replace(':', "%3A");
    let path = format!("attachments/{asset_id}/{encoded}");
    check_path(&path)?;
    Ok(path)
}

fn exclusion_reason(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if name.starts_with('.') {
        return Some("hidden_file");
    }
    let stem = Path::new(&name)
        .file_stem()?
        .to_str()?
        .replace(['-', '.'], "_");
    let sensitive = [
        "secret",
        "secrets",
        "credential",
        "credentials",
        "password",
        "passwords",
        "token",
        "tokens",
        "provider",
        "providers",
        "models",
        "auth",
        "config",
        "configuration",
        "api_key",
        "api_keys",
        "private_key",
        "id_rsa",
        "id_ed25519",
        "cache",
    ];
    let qualifiers = [
        "config",
        "backup",
        "local",
        "prod",
        "production",
        "dev",
        "test",
    ];
    sensitive
        .iter()
        .any(|reserved| {
            stem == *reserved
                || stem
                    .strip_prefix(reserved)
                    .and_then(|rest| rest.strip_prefix('_'))
                    .is_some_and(|rest| rest.split('_').all(|part| qualifiers.contains(&part)))
        })
        .then_some("sensitive_file_name")
}
fn conflict() -> CoreError {
    CoreError::new("CONFLICT", "材料冻结期间发生变化，请重试。")
}
fn scope() -> CoreError {
    CoreError::new("MATERIAL_SCOPE", "材料路径、课程归属或固定 Git 对象无效。")
}
fn git_error() -> CoreError {
    CoreError::new(
        "MATERIAL_GIT",
        "Git 检查点操作失败，未确认新快照；请保留旧版本并重试。",
    )
}
fn limit() -> CoreError {
    CoreError::new(
        "LIMIT_EXCEEDED",
        "材料检查点超过4096文件或64MiB，未生成快照。",
    )
}

#[cfg(test)]
#[path = "material_git_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::retry_busy;
