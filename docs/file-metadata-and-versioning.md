# 文件元数据与版本管理接口（file_metadata / versioning）

> 状态：已实现（Layer 1＋2，refactor/versioning-interfaces 分支）。无课程文件经内容哈希取得版本；课程级变化（stat 与聚焦 diff）已并入 Focus 部件呈现；Focus 部件不再携带正文。本页即现行契约。

## 定位

两个底层纯数据接口，服务于上下文主链：

```
底层接口 → Focus 拼接 → ContextBridge 注册 → Pi 上下文 → 最终模型请求
```

- 接口不注册任何桥部件、不含提示词语义；Focus 部件是唯一组装者。
- 原底层的 Focus/Changes 职责并入这两个接口，模块不再沿用这两个名字；Focus 只保留为上层"当前正在看的文章"部件语义。
- "当前"概念只存在于 `file_metadata.nowfile()`；下游一律使用显式文件地址。
- 版本出口唯一：init/commit、版本 hash、stat、diff、固定版本读只能经 versioning，不得在其他模块另开 Git 或哈希路径。

## 公共类型 · FileAddr（文件地址）

两接口唯一的文件入参类型；`nowfile()` 的返回值不经转换即可传入所有方法：

```
FileAddr {
  id: string           // 稳定 ID：documentId / assetId
  path: string         // 冻结时刻已解析的仓库相对路径或磁盘路径
  kind: string         // markdown | text | image | pdf | exercise | unknown
  course?: CourseId    // 归属课程；无课程文件为空
}
```

- 值对象：由 `nowfile()` 在冻结时刻从当前应用状态解析产出（id→路径的解析只发生在这一步）；接口无状态，拿到地址后不回查"当前"。
- `kind = unknown`、`course` 为空均合法（Focus 全覆盖与内容哈希后端依赖此性质）。

## 接口 1 · file_metadata

| 方法 | 返回 | 说明 |
| --- | --- | --- |
| `nowfile()` | `FileAddr` | 当前聚焦文件（用户正在看的文件）。"当前"状态的唯一来源：前端当前打开文档。无缓存。 |
| `metadata(addr)` | `{ path, kind, sizeBytes, mtime }` | `addr: FileAddr`；std::fs 直读系统原生文件系统；无缓存、无状态。 |

- 不提供正文，不提供版本。
- 无法识别的种类也返回地址（Focus 全覆盖依赖此性质；无课程文件经内容哈希实现取得版本）。

## 接口 2 · versioning

| 方法 | 返回 | 说明 |
| --- | --- | --- |
| `snapshot(course)` | `{ commitHash, materialTreeOid, excludedFiles }` | 每次冻结调用：仓库已存在直接 commit，不存在先 init 再 commit。`course: CourseId`。 |
| `version_hash(addr)` | 版本串 | `addr: FileAddr`；Git blob OID 或内容 sha256（由 `addr.course` 是否为空决定实现），调用方不感知。 |
| `file_stat(addr, base?)` | `+n -m · A/M/D · binary` | `addr: FileAddr`；单文件变更统计，不带补丁正文。 |
| `all_stat(course, base?)` | 变更文件清单 | `course: CourseId`（取自 `addr.course`）；课程级变更统计，UI 变化视图与 Focus 部件的课程变化块消费，不物化 diff。 |
| `file_diff(addr, base?)` | diff 或全文 | `addr: FileAddr`；`base = None`（首见、基准被压缩丢失或超预算）时允许返回全文。 |
| `all_diff(course, base?)` | 全文件 diff | 全量补丁，受预算约束。 |
| `read(addr, version)` | 固定版本内容 | `addr: FileAddr`；read 学习工具授权、回执与幂等校验的唯一固定版本入口。 |

实现由接口屏蔽，仅两种：

- 课程 Git 仓库（`courses/<courseId>/materials/`）。
- 内容哈希（sha256 即版本，diff 内存比较）：无课程文件；平铺课程是否走此实现待重构决策确定。

## 调用链

Focus 拼接的全部输入由 `.nowfile` 派生：

```
now  = file_metadata.nowfile()          // -> FileAddr
file_metadata.metadata(now)
versioning.version_hash(now)
versioning.file_diff(now, base = 会话已注册版本)
versioning.all_stat(now.course, base)   // course 取自 FileAddr
```

## 与上下文的关系

- Focus 部件正文＝元数据＋版本 hash＋diff/全文＋课程级 stat；不再携带 paragraphs 正文、坐标行与全文重载投影。
- 生命周期：首见全文 → 未变不重发 → 小改 diff → 压缩丢基准重发全文；基准以 Pi 真实投影为准。
- 旁路消费方（不注册部件）：read 学习工具（`read`）、UI 课程变化视图（`all_stat`）、sidecar `loadChanges` 回调。

## 同步要求

接口行为变更同步本文、[ContextBridge](context-bridge.md) 与相关契约（contracts/focus-v1、contracts/material-changes-v1、contracts/context-bridge-v1），并按仓库纪律串行运行对应回归。
