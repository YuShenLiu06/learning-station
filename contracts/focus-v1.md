# Focus v1（M2 #12）

Focus 是本次提交的视图状态，不是学习模式，也不改变课程会话归属。前端传 `documentId`、预期正文/版本、`view`、UTF-16 可见原文范围、可选 `questionId/assetId`；身份、正文与状态由 Rust 校验。阅读按 Markdown 原文块、编辑按 CodeMirror 可见范围冻结；最多16段/8000字符。练习只提供当前题号及已作答/已批改状态，不自动注入参考答案；图片/PDF 只提供元数据。现有导入器不支持 PDF，本层不新增 PDF 正文读取。

## 唯一冻结入口与 Changes 消费

`freeze_focus { request: FreezeRequest }` → Rust `focus::freeze(core, request)`，在同一 Core 锁内刷新索引、核实 session/document/引用、形成 Context，执行一次 `FrozenMaterials::capture` 和 `MaterialGit::commit`，再核对清单所有材料与固定 Git 对象一致。冻结失败不保存问题、不发送模型。返回：

```ts
type FocusSnapshot = {
  id: string; // 私有冻结记录的 SHA-256，不是任意路径
  courseId: string; documentId: string; sourceDocumentId: string;
  path: string; type: 'markdown'|'text'|'exercise'|'image'|'pdf';
  view: 'read'|'edit'|'exercise'|'asset';
  commitHash: string; materialTreeOid: string; documentVersion: string;
  excludedFiles: Array<{assetId:string;fileName:string;reason:string}>;
  paragraphs: Array<{start:number;end:number;text:string}>;
  exercise?: {questionId:string;number:number;answered:boolean;graded:boolean};
  asset?: {id:string;mime:string;size:number};
};
```

`commitHash/materialTreeOid/excludedFiles` 来自同一次 Checkpoint；`sourceDocumentId` 是练习所依附的原材料，`documentId` 是实际焦点。相同正文、不同文档身份仍是不同 Focus。路径是课程材料树中的相对路径，不是任意主机文件入口。

同次 Context、automatic materials、引用及附件路径表保存在私有 `state/focus/<id>.json`，不进入源码 Git。`focus::load(core, session, focus)` 返回同一 `Frozen`（公开 `focus/context`），校验记录指纹、课程/当前可见性、引用和固定 Git 检查点。**Changes 应消费此 Frozen 与固定 OID，不得另行 capture。** `focus::open_materials(core)` 只重开同一私有材料 Git，供 `compare/files/read` 使用；不接受前端路径，也不初始化第二个仓库。 此层没有 Changes、Pi 接收基线或完整 Citation 去重。

## 请求、运行与恢复

普通发送与快捷练习先冻结，再保存 `RequestedContext.focus`；`GenerationRequest.focus`、Run、Pi 本轮 metadata、assistant 自身来源清单保存同一对象。真实材料读取和 query/read 工具按本 Run 的固定 OID；权限档位、回收可见性和课程归属仍独立校验，不因历史快照扩大授权。

切页后的进度只更新该 operation 保存的文档来源显示，不能把最新页面当作旧请求状态。取消不删除快照；重试沿原 Focus/引用，工作区编辑不会混入旧请求。未知结果沿原 operation 核对。旧记录缺 Focus 明确显示未记录；既有无课程 legacy 会话保留兼容发送路径，不伪造 Git Focus。

Git 投影同时支持应用实际生成的 `exercises/<id>.md`，仅该明确 kind/path 组合映射为 `documents/<id>.md`；其他非约定路径仍拒绝，课程祖先链校验不变。

验证使用临时合成库、真实 Git、React/mock IPC、锁版 Pi 与 localhost fixture，分别报告。当前不做 GUI；重开界面回归使用 jsdom，不能称实际桌面验收。删除本层提交即可停用新入口；保留私有 Focus/Git 数据，不自动清库或改权限。
