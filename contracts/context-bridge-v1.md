# ContextBridge v1 清单契约

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
    range?: {startChar: number; endChar: number; complete: boolean; totalBytes: number};
    reason: string;
  }>;
  projectedChars: number;
  projectedBytes: number;
};
```

`history`仅为旧快照兼容；新模型历史由Pi管理，不再列出Rust裁剪记录。此表是桥片段清单，不是全部原生消息目录或服务端阅读证明。

内部冻结包新增 `nativeMaterials`（经授权核验的当前完整范围、哈希及候选frames）和 `nativeReferences`。适配器核对实际Pi投影后选择沿用、diff或full重载；不把旧history拼成新userJSON。界面仍只显示sourceId/短指纹和部分范围，不新增路径打开，也不承诺总token下降。

## 课程可读范围与实际正文

`Run.courseReadManifest?: ManifestItem[]` 是独立的课程授权目录及精确版本快照，不含正文，不发送给模型。目录项 `includedChars=0` 不表示已接收；旧 Run 缺省时沿用原 `contextManifest` 授权。目录包含本课程可见笔记和附件，内部工具范围上限为4096份；单次显式引用仍遵守原有数量限制。

普通课程对话不自动遍历当前笔记正文；显式选区、练习来源和 Citation 继续加入真实正文。`Context.manifest` / `Run.contextManifest` 仅列实际正文来源，`contextBridgeManifest` 仍以 Pi 实际投影为准，目录不能混入三者。

受控 `read` 回包提供 `includedInInitialContext`、`authorizationScope`、`delivery=tool-result`、`range` 和 `truncated`。这些字段说明初始正文快照及本次返回范围，不证明远端已收到；Pi 的真实工具消息、投影与持久回执才是 session 正文事实。读取核实冻结版本、当前课程归属及回收状态；附件复用原有校验，仅返回纯文本，图片明确拒绝。

Focus provider 后续仍接既有 `nativeMaterials` 投影接口：首次全文、有效完整基准才使用 Git 差量、未变不重复，新 session 或压缩丢失基准恢复全文。本切片保留该接口，不声称已完成 Focus 与 UI 的组合验收。

## 内嵌内容

`learning-embed` 块是现有笔记正文的一部分，不增加 bridge 的授权、网络抓取或来源编号。网页/视频链接不自动变为知识来源；AI 回答引用的编号不能冒充 `manifest` 中的材料编号。

内部运行快照可含 Run.sourceDocumentId：它由 Rust 在请求接受时从真实材料/练习账本归一化，区别于可编辑页面 ID。结果持久化与工具派发沿用该来源和冻结 manifest，不依靠页面元数据或课程会话的首次 sectionId 重新选源。旧运行字段可缺省；不新增前端必填参数或读取权限。

课程会话尚未提供Focus时，`documentId` 为空或省略表示本轮无自动材料；`selection` 此时必须为空。显式 `injections` 逐项核对版本、选区与本课程归属，且不改变本轮的 `sourceDocumentId`。真实探索/练习传材料编号；Run 与已保存问题冻结各自来源，恢复不回落当前阅读文档。

课程目录与已读事实分别记账：`Run.courseReadManifest` 是提交时可读来源及版本的授权元数据快照，不含正文，不是已读基准或第二缓存；`Run.contextManifest`/`Context.manifest` 保留实际发送正文语义，界面“实际来源”只取该实际快照。课程可读清单与实际来源分开展示；`includedChars=0` 不构成已收到/已读的证据。`contextBridgeManifest` 记录实际发出的正文范围，工具read回执说明实际版本、范围及截断；每会话实际收到的正文事实继续由Pi消息/投影负责，不重打包历史。

Focus由独立冻结接口传入，仍自动加载聚焦正文；本层空来源只取消无Focus开局的自动笔记加载，不取消Focus正文或显式Citation补充。Focus首次或Pi实际上下文缺基准时完整提供；有效全文基准存在时可沿固定Git OID提交聚焦diff，无变化不重复，接收基准按会话独立并由Pi实际投影/回执确认，Git提交不算模型已收。
