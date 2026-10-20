# 课程物理项目 v1

新课程使用 `library/courses/<courseId>/`；A/B 互为兄弟目录。
`course_project_root(libraryRoot, courseId)` 返回现存且无符号链接的 canonical 根，供 Pi cwd 与 Git 共用。
缺失/旧平铺课程返回 `COURSE_PROJECT_REQUIRED`，不隐式复制、迁移或初始化真实旧库。

- `library_tree::create_project(store, title, body, requestId)`：幂等新建物理课程；旧 `create` 保持旧课程行为。
- `course_project::create_material(store, Metadata, body)`：显式创建物理课程/其子文档。
- 文档唯一正文：`materials/documents/<stableId>.md`。已有子文档创建入口按父课程归一化到此处。
- 附件唯一正文：`materials/assets/<courseId>/<assetId>/<escapedOriginalName>`。
  `%` 编码 `%25`，`:` 编码 `%3A`；不接受路径成分。
- `.asset.json` 是原始导入回执。原生编辑直接修改正文，read/list 返回当前 hash/size；相同请求重试返回原导入回执，不覆盖编辑。
- 文档索引只扫描物理课程的 `materials/documents/`；附件、隐藏文件及材料外会话/Git 状态不成为文档。
- 原生编辑文档后，已有 read 立即读当前文件，陈旧 save 返回 CONFLICT。
- 课程内重新排序/挂载保留路径；不以改 parentId 偷渡跨物理课程移动。
- 旧文档、旧附件布局及其完整性校验原样保留。旧检查点读取与新课程独立 Git 由下一层提供。

本层不改 UI、Pi session/entry 身份、资源发现、工具权限、Git 历史或聊天摄入。
测试仅使用临时合成数据；回退本层代码前保留整个课程目录，新数据不会被自动删除。
