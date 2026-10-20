# ContextBridge v1 清单契约

## 课程可读范围与实际正文

- `Run.courseReadManifest?: ManifestItem[]` 仅是本次提交的课程可读来源与精确版本快照，不含正文、不进入模型 payload，也不是已读/已收到/完整基准证据。其 `includedChars` 不参与投影判断；兼容值0不能解释为阅读状态。字段缺省的旧Run继续使用原授权快照。
- `Context.manifest` / `Run.contextManifest` 保持实际正文来源语义，课程目录不能混入其中。现有UI仅将此实际字段渲染为来源；如需显示课程可读目录须另列，不合并为已发送来源。
- `contextBridgeManifest` 在 Pi 完成真实投影后记录该请求提供的实际来源、版本和截断范围；queued/unknown不冒充已收到。Rust read 回包另提供实际 `sourceId/version/includedChars/truncated` 和返回范围，原生工具消息及持久回执保留各session的接收事实；不从课程Git存在或授权清单推导基准。
- 普通开局没有Focus时不自动遍历笔记。Focus继续自动加载：无有效完整正文时给全文，有保留基准才可沿固定OLD/NEW Git版本给完整差量，无变化不重复；新session、压缩或基准失效恢复全文。此层复用 Pi 既有material projection/receipt/compaction，不新增缓存或重打包历史。其他文件summary与显式Citation补充范围由各自provider接缝接入，保留双来源。

职责、角色和恢复见[上下文桥](../docs/context-bridge.md)与[原生会话](pi-native-session-v1.md)。UI清单类型保持兼容；课程会话管理 IPC 见原生会话契约。

`Run.contextBridgeManifest?` 在Pi完成实际材料投影后保存；queued初始可空。该对象同时进入本轮原生turn custom message，最终assistant自身 `entry.data.contextBridgeManifest`复制自己的快照。预览/旧缺字段不从Session补齐。

```ts
type ContextBridgeManifest = {
  version: 1;
  entries: Array<{
    id: string;
    kind: 'material' | 'history' | 'reference' | 'turn' | 'stable';
    status: 'added' | 'retained' | 'updated' | 'reloaded';
    sourceId?: string;
    version: string;
    textHash?: string;
    includedChars: number;
    locator?: {startChar: number; endChar: number};
    range?: {startChar: number; endChar: number; complete: boolean; totalBytes: number};
    reason: string;
  }>;
  projectedChars: number;
  projectedBytes: number;
};
```

`history`仅为旧快照兼容；新模型历史由Pi管理，不再列出Rust裁剪记录。此表是桥片段清单，不是全部原生消息目录或服务端阅读证明。

内部冻结包新增 `nativeMaterials`（经授权核验的当前完整范围、哈希及候选frames）和 `nativeReferences`。适配器核对实际Pi投影后选择沿用、diff或full重载；不把旧history拼成新userJSON。界面仍只显示sourceId/短指纹和部分范围，不新增路径打开，也不承诺总token下降。

## 内嵌内容

`learning-embed` 块是现有笔记正文的一部分，不增加 bridge 的授权、网络抓取或来源编号。网页/视频链接不自动变为知识来源；AI 回答引用的编号不能冒充 `manifest` 中的材料编号。

显式材料选区采用 [Citation](citation-v1.md) 的 quoted_material 块；若同一来源已在自动材料中，保留一个材料 manifest 项，并以 citation:<id> 的 reference 项标记选区及位置。两类生命周期仍分开；本切片未接入 Focus 或 Git 版本差量去重。

内部运行快照可含 Run.sourceDocumentId：它由 Rust 在请求接受时从真实材料/练习账本归一化，区别于可编辑页面 ID。结果持久化与工具派发沿用该来源和冻结 manifest，不依靠页面元数据或课程会话的首次 sectionId 重新选源。旧运行字段可缺省；不新增前端必填参数或读取权限。

课程会话 `documentId` 为空或省略表示本轮无自动材料；`selection` 此时必须为空。显式 `injections` 逐项核对版本、选区与本课程归属，且不改变本轮的 `sourceDocumentId`。真实探索/练习传材料编号；Run 与已保存问题冻结各自来源，恢复不回落当前阅读文档。
