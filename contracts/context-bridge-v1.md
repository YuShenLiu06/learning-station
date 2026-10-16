# ContextBridge v1

ContextBridge 是学习站状态到 Pi 的薄适配层，未安装 pi-readcache，不扩展工具权限，不切换 Pi 底座。

## 顺序与职责

提交 GenerationRequest 后，在 Core 锁内核实会话及授权来源并冻结一致版本/实际展示范围；历史与资料提供器产出投影。ContextBridge 集中去重、排序、执行已有预算，冻结时间和本次状态，得到普通 `user` 消息中的有序 JSON：learningHistory → context（自动资料基准/变更链，再一次性引用）→ 本轮 metadata/time → contextBridgeManifest → question。然后才捕获凭据、预占 Run 并进入 Pi。Pi 仍使用 Agent.prompt(user)，固定应用规则为 system；后续工具循环由 Pi 管理，沿用本次冻结请求。没有伪造 toolCall/toolResult。

提供器接口 `ContextProvider::provide() -> PromptPart{key,lifetime,position,value}`。生命周期仅 Stable、Turn、Versioned、OneShot；无插件注册、第二会话框架、通用权限或新 focus 功能。资料授权和有效条件仍由既有业务模块核实。

## 实际投影清单

`Run.contextBridgeManifest?: ContextBridgeManifest`；每条已保存 assistant 自身 `entry.data.contextBridgeManifest` 同名快照。该对象与实际发送 user JSON 的 `contextBridgeManifest` 完全相同。历史缺字段明确未记录，不能借 Session 当前来源回填；预览接口没有此字段，不冒充已发送。

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

UI 仅在现有 RequestContextView 显示“上下文桥 · 本次投影清单”，映射新增/沿用/更新/重载；kind 区分资料/有效历史/本次引用/本轮状态/稳定协议。range.complete=false 明确“仅前缀/部分”；仅显示 sourceId 和短指纹，不新增文件打开权限。不将投影或 queued 状态说成服务端已读/已接收。

## 资料增量边界

只自动课程/关联文件与明确链接的文本附件使用基准/差异；显式一次性引用保持原消费和失败重试快照，不纳入基准。正文范围、哈希及版本冻结；工具读取始终返回该受控请求范围的实际正文片段。基准必须同时有有效历史锚点和本次真实重放的 full 正文，损坏、裁剪、边界/分支、丢失或过大则正文重载。部分文件始终标注范围，保守重载；失败/取消不推进基准。只在模型上下文投影改变，原学习历史展示不改写。

当前每请求新建 Agent；即使本轮零新增，API 仍要发送必要的保留基准。报告须区分本轮新增量和实际请求总量，不承诺无状态请求总 token 必然下降。
