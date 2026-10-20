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

调用方在 Core 锁内刷新索引、冻结归属/材料，冻结结果移出锁后可提交；两次采集不一致报 CONFLICT，后续编辑不改变已冻结字节。每份入树文档直接来自已解析祖先链的同一版本，归属校验后不再重读正文；同一祖先在不同链出现不同字节也报 CONFLICT。这不是跨文件系统事务，但任何纳入的文档及祖先版本必须自洽地归属于该课程。64MiB/4096文件超限显式失败，不截断。仅纳入严格课程祖先链解析成功且原路径为 `courses/<id>.md` 的可见节点，及现有附件服务完整校验的该课程附件；不扫描整个 `courses` 作为一门课，不移动源文件。旧式其他物理路径拒绝，需另行明确投影策略。

仓库根为 `materials/<courseId>/`，其下 `documents/<nodeId>.md` 与 `attachments/<assetId>/<escapedFileName>` 是 `files/read/compare` 使用的相对路径。使用公共 `attachment_path(asset_id, original_file_name)` 获取附件路径：原名 `%` 映射为 `%25`，`:` 映射为 `%3A`，其他字符保持原样，因此兼容存量合法冒号名称且不会与字面 `%3A` 名称冲突；调用方优先使用 files 返回的路径，不手拼路径。

材料树不含状态、Pi、源码、缓存或根配置；敏感配置/凭据名称附件排除，普通材料正文不进行秘密内容识别。名称采用完整保留名及明确配置/备份后缀匹配，不用子串误排 tokenization/author 等教学材料。隐藏名称以 hidden_file、敏感名称以 sensitive_file_name 记录排除原因。拒绝越界、控制符、路径表达式、symlink 和非普通文件；复用 ContentStore/附件校验，不执行材料。Git 子进程清空继承环境、设置 `GIT_NO_LAZY_FETCH=1`、空 `GIT_ALLOW_PROTOCOL` 与 `protocol.allow=never`，即便私有仓已有 promisor/transport 配置，查询缺失 OID 也只能在本地失败。关闭 hooks/外部 diff/textconv/filter，使用原生 blob/tree/commit/update-ref 并启用对象/引用 fsync。调用方仅传应用私有可信目录；本服务不暴露任意路径 IPC。

`Checkpoint` 序列化为 `{courseId, checkpointCommitOid, materialTreeOid, excludedFiles}`：全仓 commit 定位一次检查点，材料 tree 是该课程内容身份，由 `COMMIT:materials/COURSE` 派生；`MaterialFile` 仍为 `{path, blobOid}`。`excludedFiles` 每项为 `{assetId,fileName,reason}`，在同一 commit 的 `material-exclusions/<courseId>.json` 持久保存，位于材料树外；新增排除信息不会改变材料身份，checkpoint 重开可核对当时名称/原因，初版没有此文件时返回空列表。只接受完整 Git OID，不接受 HEAD/短 SHA/revision 表达式。每次成功 commit 有 `refs/checkpoints/<courseId>/<commitOid>` 持久可达；失败不返回成功回执，不推进旧基准。原输入可重试，保留失败产生的不可达对象交 Git 管理。

`MaterialDiff {initial, changes}`；每项 `{path, status, added, deleted, binary, patch, patchBytes}`。聚焦文件返回完整原生 Git patch（所有变化 hunk，不是整个文件全文）：UTF-8 可解码时放在 patch，否则放在 patchBytes 的完整字节数组；两者互斥，不做乱码替换，不因单文件编码失败丢失整个 changes。binary 保持 Git numstat 的原生 '-' 判定；Git 判断为二进制时行数、patch、patchBytes 都为 None，未聚焦也不返回 patch。禁用重命名推断，移动表现为 D+A。无变化列表为空。首轮 `old=None` 与 Git 空树比较，`initial=true`，调用方用 files/read 补必要全文。基础接口不按发送预算截断；后续发送层若超限必须显式标识并保留固定版本读取入口，不可把 patchBytes 当作空文本或没有变化。

未来同仓 Pi 可在材料树之外保留原 session/entry 身份：仅 transcript 变化会改变 commit，而材料 tree 不变。本层不自动纳入 Pi；任何未来扩展必须保留其他子树，不能直接用当前仅材料 commit 写入器做联合检查点。任何 OID 不写入自身内容；文件快照身份不等于模型已接收基准。

最短独立验收：`cargo test --locked --manifest-path src-tauri/Cargo.toml --no-default-features material_git`，只用临时合成库与真实 Git。无需账户或 GUI。回退此层提交即可停止使用；已有私有检查点保留，不清库。未验证 Windows、真实课程库、真实 GUI/模型和断电恢复。

原生命令依据：[mktree](https://git-scm.com/docs/git-mktree)、[diff](https://git-scm.com/docs/git-diff)、[update-ref](https://git-scm.com/docs/git-update-ref)、[fsync](https://git-scm.com/docs/git-config#Documentation/git-config.txt-corefsync)。

首轮修复回归包含确定性原子替换、祖先版本冲突、promisor/ext transport 禁止、正常教学名称/持久排除报告、存量冒号与百分号路径，以及前8192字节无NUL的可解码2×2 JPEG（17,017字节合成 fixture，非截图）的完整非UTF8 patch。附件锁占用返回可重试 BUSY，不创建检查点、不推进基准；锁释放后可重新执行只读 capture。回归显式验证此生命周期，测试夹具仅对 BUSY 做2秒内的有界重试，其他错误立即失败。无关损坏祖先的可用性、重复附件读验优化和同UID动态symlink窗口不在本轮修复范围。

#12 接入补充：应用原生练习位于 `exercises/<id>.md`；仅 `kind=exercise` 且该精确路径与 `courses/<id>.md` 同样经严格祖先链校验后映射为 `documents/<id>.md`。这不开放任意旧路径或会话状态文件。
