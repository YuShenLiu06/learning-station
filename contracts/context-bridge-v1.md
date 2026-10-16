# ContextBridge v1 传输契约

职责、入口、消息角色、顺序、生命周期及回退见[ContextBridge 文档](../docs/context-bridge.md)。

`Run.contextBridgeManifest?` 与已保存 assistant 自身 `entry.data.contextBridgeManifest`，均为实际 user JSON 中同名对象的冻结快照。预览缺字段，旧缺字段不回填 Session 当前值。

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

仅在现有来源区显示五类片段和新增/沿用/更新/重载，部分范围明确标记。只显示稳定 sourceId 与短指纹；不新增路径打开，不把投影/queued 称为服务端已读。
