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
- 仅完整项目路径及实际根文档确认物理归属；索引限制须先安全、有界读取根文档并确认 ID 匹配目录、kind=course、parentId 为空，不能凭同名普通文件存在判断项目。旧嵌套 courses 路径和偶含 materials 的旧路径保持索引/创建行为。旧请求对应平铺课程时，create_project 明确报错，不伪报新项目或自动迁移。
- 旧文档、旧附件布局及其完整性校验原样保留。旧检查点读取与新课程独立 Git 由下一层提供。

本层不改 UI、Pi session/entry 身份、资源发现、工具权限、Git 历史或聊天摄入。
测试仅使用临时合成数据；回退本层代码前保留整个课程目录，新数据不会被自动删除。
