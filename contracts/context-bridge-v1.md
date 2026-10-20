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

内部运行快照可含 Run.sourceDocumentId：它由 Rust 在请求接受时从真实材料/练习账本归一化，区别于可编辑页面 ID。结果持久化与工具派发沿用该来源和冻结 manifest，不依靠页面元数据或课程会话的首次 sectionId 重新选源。旧运行字段可缺省；不新增前端必填参数或读取权限。

## Focus

`GenerationRequest.focus`、`Run.focus` 与 assistant 自身 `entry.data.focus` 使用 [Focus v1](focus-v1.md) 的同一个冻结对象；Pi turn metadata 接收同一值。Focus 不从最新 Session 或页面回填。queued Focus 表示已冻结请求，不冒充 Pi 已接收材料基线。

Focus 使用同 checkpoint 的 Git binary 判定移除自动材料正文，清单保留0字符元数据；`metadataOnlySourceIds` 同步排除 Pi 旧自动材料投影。用户明确选区/引用及经授权的主动文本 read 沿各自原协议，不能把元数据身份当作图片正文读取权限。
