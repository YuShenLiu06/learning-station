# ContextBridge v1 清单契约

职责、角色和恢复见[上下文桥](../docs/context-bridge.md)与[原生会话](pi-native-session-v1.md)。UI类型保持兼容，无新前端IPC。

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
