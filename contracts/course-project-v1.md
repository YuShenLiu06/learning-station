# 课程物理项目 v1

新课程使用 `library/courses/<courseId>/`；A/B 互为兄弟目录。
`course_project_root(libraryRoot, courseId)` 返回现存且无符号链接的 canonical 根，供 Pi cwd 与 Git 共用。
缺失/旧平铺课程返回 `COURSE_PROJECT_REQUIRED`，不隐式复制、迁移或初始化真实旧库。

- `library_tree::create_project(store, title, body, requestId)`：幂等新建物理课程；旧 `create` 保持旧课程行为。
- `course_project::create_material(store, Metadata, body)`：显式创建物理课程/其子文档。
- 文档唯一正文：`materials/documents/<stableId>.md`。已有子文档创建入口按父课程归一化到此处。
- 附件唯一正文：`materials/assets/<courseId>/<assetId>/<escapedOriginalName>`。
  `%` 编码 `%25`，`:` 编码 `%3A`；不接受路径成分。编码超 255 字节或与回执重名时用 `content.<ext>`（隐藏名用 `.content.<ext>`），原名仍保留在回执。
- 导入先在隐藏 `.import-*` 目录写齐并同步正文/回执，再在附件锁内原子发布。失败不留下可见半成品；崩溃遗留暂存不进入列表或材料，保留待核对，不自动删除。
- `.asset.json` 是原始导入回执。原生编辑直接修改正文，read/list 返回当前 hash/size；相同请求重试返回原导入回执，不覆盖编辑。
- 文档索引只扫描物理课程的 `materials/documents/`；附件、隐藏文件及材料外会话/Git 状态不成为文档。
- 原生编辑文档后，已有 read 立即读当前文件，陈旧 save 返回 CONFLICT。
- create/save/tree move 共用物理归属校验：课程内改正文/重新排序保留路径；旧节点不能只改 parentId 挂入项目，项目节点也不能据此跨课程或移出项目。
- 原生编辑 parentId 后，严格归属解析拒绝跨物理课程；树视图发出 PHYSICAL_PARENT 警告并按物理课程显示，不替用户重写正文。
- 仅完整项目路径及实际根文档确认物理归属；旧嵌套 courses 路径和偶含 materials 的旧路径保持索引/创建行为。旧请求对应平铺课程时，create_project 明确报错，不伪报新项目或自动迁移。
- 旧文档、旧附件布局及其完整性校验原样保留。旧检查点读取与新课程独立 Git 由下一层提供。

本层不改 UI、Pi session/entry 身份、资源发现、工具权限、Git 历史或聊天摄入。
测试仅使用临时合成数据；回退本层代码前保留整个课程目录，新数据不会被自动删除。

## 独立 Git 检查点

`MaterialGit::init_course(libraryRoot, courseId)` 只初始化新物理课程的普通 `.git/`；
`open_course` 校验课程绑定、独立 objects/refs，拒绝共享 worktree、alternates、symlink/hardlink 或其他课程仓库。
不重置既有仓库，不自动初始化/迁移旧库。`MaterialGit::open` 仅只读兼容旧 bare 快照；旧创建器仅在合成测试编译。

`FrozenMaterials::capture_project(store, courseId)` 冻结该课程当前 `materials/` 文件；两次捕获相同才返回。
无文件系统事务承诺；返回后 owned bytes 不受后续编辑影响。附件采用物理路径，不再生成第二套可编辑投影。
捕获遵守 ContentStore 现有回收记录，按原路径及托管文档 ID 排除已回收材料；托管 Markdown 扩展名与 ContentStore 共用大小写不敏感识别，包含 .md/.MD/.Md。回收后为 Git 删除、恢复后重新新增，旧固定 OID 仍可读取原文。捕获中记录变化报 TRASH_STATE_CHANGED，调用方须刷新后重新冻结。
回收不会删除原物理文件；Pi 的原始 cwd 工具不能用磁盘存在推断当前材料可见性，后续接入仍须复用现有回收过滤。本层不改变 Pi 工具授权或搬移材料。
仅纳入既有允许清单中的文本/代码/图片；排除隐藏文件、密钥/供应商配置名称、缓存、嵌套 Git 源码仓库和未知扩展。
材料外的 Pi/其他文件不自动摄入。排除报告位于材料树外，已知附件保留 assetId，未注册物理路径的 assetId 为空。

`commit(frozen, parentCommitOid?)` 成功发布持久 ref 后才返回 Checkpoint；失败保留旧回执，未发布对象不代表快照成功。
checkpointCommitOid 定位整个检查点；materialTreeOid 从 `COMMIT:materials` 派生；文件 blob OID 按需从 files 获取。
父提交的材料外树项原样保留，预留多个 Pi session 同仓，但不捕获当前聊天或实现联合回滚。
transcript-only 提交改变 checkpointCommitOid 而保持 materialTreeOid；不把 commit OID 写入自身内容。

files/read/checkpoint/compare 均使用固定 OID；read 返回完整原始字节。
compare 复用原生 name-status/numstat；聚焦文本返回完整 UTF-8 patch（非 UTF-8 patch 使用完整 patchBytes），二进制仅文件变化。
基础层不静默截断；后续发送层若超容量须显式说明。Git 禁止外部 diff/textconv/hooks、网络和 lazy fetch。
