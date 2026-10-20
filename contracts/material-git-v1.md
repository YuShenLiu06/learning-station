# Git 材料检查点基础 v1（#14 第一层）

新增 Rust `material_git` 服务；本层不接 UI、App/Pi 请求、会话实际接收基准或回滚，不初始化/迁移真实课程库。

```rust,ignore
let git = MaterialGit::create_new(private_absolute_path)?; // 仅显式新建，父目录须存在且不在源码 Git 内
// 重开：MaterialGit::open(private_absolute_path)?
let frozen = FrozenMaterials::capture(&content_store, course_id)?;
let cp = git.commit(&frozen, previous_commit_oid)?;
let files = git.files(course_id, &cp.checkpoint_commit_oid)?;
let bytes = git.read(course_id, &cp.checkpoint_commit_oid, &files[0].path)?;
let diff = git.compare(course_id, previous_commit_oid, &cp.checkpoint_commit_oid, focus_path)?;
```

调用方在 Core 锁内刷新索引、冻结归属/材料，冻结结果移出锁后可提交；两次采集不一致报 CONFLICT，后续编辑不改变已冻结字节。这不是跨文件系统事务。64MiB/4096文件超限显式失败，不截断。仅纳入严格课程祖先链解析成功且原路径为 `courses/<id>.md` 的可见节点，及现有附件服务完整校验的该课程附件；不扫描整个 `courses` 作为一门课，不移动源文件。旧式其他物理路径拒绝，需另行明确投影策略。

仓库根为 `materials/<courseId>/`，其下 `documents/<nodeId>.md` 与 `attachments/<assetId>/<originalFileName>` 是 `files/read/compare` 使用的相对路径。材料树不含状态、Pi、源码、缓存或根配置；敏感配置/凭据名称附件排除，普通材料正文不进行秘密内容识别。拒绝越界、控制符、路径表达式、symlink 和非普通文件；复用 ContentStore/附件校验，不执行材料。仓库无远端操作，Git 清空继承环境、关闭 hooks/外部 diff/textconv/filter，使用原生 blob/tree/commit/update-ref 并启用对象/引用 fsync。调用方仅传应用私有可信目录；本服务不暴露任意路径 IPC。

`Checkpoint` 序列化为 `{courseId, checkpointCommitOid, materialTreeOid}`：全仓 commit 定位一次检查点，材料 tree 是该课程内容身份，由 `COMMIT:materials/COURSE` 派生；`MaterialFile` 为 `{path, blobOid}`。只接受完整 Git OID，不接受 HEAD/短 SHA/revision 表达式。每次成功 commit 有 `refs/checkpoints/<courseId>/<commitOid>` 持久可达；失败不返回成功回执，不推进旧基准。原输入可重试，保留失败产生的不可达对象交 Git 管理。

`MaterialDiff {initial, changes}`；每项 `{path, status, added, deleted, binary, patch}`。聚焦文本返回完整 Git patch（所有变化 hunk，不是整个文件全文）；未聚焦只返回 Git name-status/numstat；二进制行数为 None，patch 为 None。禁用重命名推断，移动表现为 D+A。无变化列表为空。首轮 `old=None` 与 Git 空树比较，`initial=true`，调用方用 files/read 补必要全文。基础接口不按发送预算截断；后续发送层若超限必须显式标识并保留固定版本读取入口。

未来同仓 Pi 可在材料树之外保留原 session/entry 身份：仅 transcript 变化会改变 commit，而材料 tree 不变。本层不自动纳入 Pi；任何未来扩展必须保留其他子树，不能直接用当前仅材料 commit 写入器做联合检查点。任何 OID 不写入自身内容；文件快照身份不等于模型已接收基准。

最短独立验收：`cargo test --locked --manifest-path src-tauri/Cargo.toml --no-default-features material_git`，只用临时合成库与真实 Git。无需账户或 GUI。回退此层提交即可停止使用；已有私有检查点保留，不清库。未验证 Windows、真实课程库、真实 GUI/模型和断电恢复。

原生命令依据：[mktree](https://git-scm.com/docs/git-mktree)、[diff](https://git-scm.com/docs/git-diff)、[update-ref](https://git-scm.com/docs/git-update-ref)、[fsync](https://git-scm.com/docs/git-config#Documentation/git-config.txt-corefsync)。
