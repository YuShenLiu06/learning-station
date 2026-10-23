# 后端（src-tauri）开发约定

- 修改文件元数据、版本、冻结或 diff 行为前，先读 [docs/file-metadata-and-versioning.md](../docs/file-metadata-and-versioning.md)。
- `file_metadata` 与 `versioning` 是底层纯数据接口：无上下文语义、不注册桥部件、不写提示词；"当前文件"状态只存在于 `file_metadata.nowfile()`。
- 版本出口唯一：init/commit、版本 hash、stat、diff、固定版本读只经 `versioning`，不在其他模块新增 Git 或哈希路径；原 Focus/Changes 底层职责并入这两个接口，不再以旧名扩展。
- 接口行为变更同步 docs 与 contracts（focus-v1、material-changes-v1），并串行运行 Rust / sidecar / 前端回归。
