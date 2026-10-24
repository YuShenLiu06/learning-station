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

## 课程可读范围与实际正文

`Run.courseReadManifest?: ManifestItem[]` 是独立的课程授权目录及精确版本快照，不含正文，不发送给模型。目录项 `includedChars=0` 不表示已接收；旧 Run 缺省时沿用原 `contextManifest` 授权。目录包含本课程可见笔记和附件，内部工具范围上限为4096份；单次显式引用仍遵守原有数量限制。

普通课程对话不自动遍历当前笔记正文；显式选区、练习来源和 Citation 继续加入真实正文。`Context.manifest` / `Run.contextManifest` 仅列实际正文来源，`contextBridgeManifest` 仍以 Pi 实际投影为准，目录不能混入三者。

受控 `read` 回包提供 `includedInInitialContext`、`authorizationScope`、`delivery=tool-result`、`range` 和 `truncated`。这些字段说明初始正文快照及本次返回范围，不证明远端已收到；Pi 的真实工具消息、投影与持久回执才是 session 正文事实。读取及旧回执重放/恢复均核实当前来源、课程归属、回收状态和冻结版本；回执版本还须等于本轮冻结版本，跨run同问题同参数不放宽检查。已有原生read回包也须经Rust重新核实；不改写旧回执。附件复用原有校验，仅返回纯文本，图片明确拒绝。

Focus provider 后续仍接既有 `nativeMaterials` 投影接口：首次全文、有效完整基准才使用 Git 差量、未变不重复，新 session 或压缩丢失基准恢复全文。本切片保留该接口，不声称已完成 Focus 与 UI 的组合验收。

## 内嵌内容

`learning-embed` 块是现有笔记正文的一部分，不增加 bridge 的授权、网络抓取或来源编号。网页/视频链接不自动变为知识来源；AI 回答引用的编号不能冒充 `manifest` 中的材料编号。

显式材料选区采用 [Citation](citation-v1.md) 的 quoted_material 块；若同一来源已在自动材料中，保留一个材料 manifest 项，并以 citation:<id> 的 reference 项标记选区及位置。两类生命周期仍分开；Focus 冻结与固定 Git 投影复用同一来源身份；实际接收基准仍由各 Pi 会话确认。

内部运行快照可含 Run.sourceDocumentId：它由 Rust 在请求接受时从真实材料/练习账本归一化，区别于可编辑页面 ID。结果持久化与工具派发沿用该来源和冻结 manifest，不依靠页面元数据或课程会话的首次 sectionId 重新选源。旧运行字段可缺省；不新增前端必填参数或读取权限。

课程会话尚未提供Focus时，`documentId` 为空或省略表示本轮无自动材料；`selection` 此时必须为空。显式 `injections` 逐项核对版本、选区与本课程归属，且不改变本轮的 `sourceDocumentId`。真实探索/练习传材料编号；Run 与已保存问题冻结各自来源，恢复不回落当前阅读文档。

课程目录与已读事实分别记账：`Run.courseReadManifest` 是提交时可读来源及版本的授权元数据快照，不含正文，不是已读基准或第二缓存；`Run.contextManifest`/`Context.manifest` 保留实际发送正文语义，界面“实际来源”只取该实际快照。课程可读清单与实际来源分开展示；`includedChars=0` 不构成已收到/已读的证据。`contextBridgeManifest` 记录实际发出的正文范围，工具read回执说明实际版本、范围及截断；每会话实际收到的正文事实继续由Pi消息/投影负责，不重打包历史。

Focus由独立冻结接口传入，仍自动加载聚焦正文；本层空来源只取消无Focus开局的自动笔记加载，不取消Focus正文或显式Citation补充。Focus首次或Pi实际上下文缺基准时完整提供；有效全文基准存在时可沿固定Git OID提交聚焦diff，无变化不重复，接收基准按会话独立并由Pi实际投影/回执确认，Git提交不算模型已收。
## Focus

`GenerationRequest.focus`、`Run.focus` 与 assistant 自身 `entry.data.focus` 使用 [Focus v1](focus-v1.md) 的同一个冻结对象；审计快照保留完整Focus；目标完整正文已由材料投影供给时，Pi turn metadata仅保留身份、位置和范围，不重复段落text，字符数按实际投影统计。Focus 不从最新 Session 或页面回填。queued Focus 表示已冻结请求，不冒充 Pi 已接收材料基线。focus 注册内容的目标形态（Layer 2，见 [文件元数据与版本接口](../docs/file-metadata-and-versioning.md)）：元数据＋版本hash＋diff/全文＋课程级 stat，移除 paragraphs 正文与坐标投影；Layer 1 现状：envelope 结构与注册内容暂不变，底层版本/差量出口经 versioning。

Focus 使用同 checkpoint 的 Git binary 判定移除自动材料正文，清单保留0字符元数据；`metadataOnlySourceIds` 同步排除 Pi 旧自动材料投影。用户明确选区/引用及经授权的主动文本 read 沿各自原协议，不能把元数据身份当作图片正文读取权限。

历史授权与read目录独立：`nativeSession.authorizedSourceIds` 继续使用课程可见历史来源集合，兼容旧course/folder/exercise编号，不因此授予read权限或注入正文。仍授权的历史与摘要在升级时保留；真实撤权仍触发原生权限边界。

具有Focus的请求按原固定OID读取，后续工作区编辑不会混入该请求；新请求冻结为新版本后，不得重放旧版本回执。回收和课程归属仍核对当前可见性，旧Git对象存在不授予读取权限。交出课程cwd前也复用同一ContentStore回收门禁。

## 请求调试的两个上下文视图（#63）

「本轮新增上下文」从捕获请求的 `sources` 取 `origin=bridge`、明确的
`registrationKey`，并以 `userEntryId`（优先）或 `runId` 核实当前用户轮。
只显示这些来源绑定的实际 payload 片段，不能用冻结候选或完整投影注册列表代替。
同用户轮的工具续跑与重试保留该轮注册归属；“新增”不表示相对上一 HTTP 请求的差异。
缺少注册键、轮次或来源映射时不猜测，不把原生问题、历史部件或压缩摘要当成本轮注册。

「完整上下文」按捕获 payload 的 system/messages 顺序显示完整消息对象，保留
历史、压缩摘要、工具调用和回包的结构字段；缺少来源映射不隐藏正文。
JSON 模式使用同一视图范围：本轮为已定位注册片段，完整为原捕获请求体。
超限缺失明确显示不完整，不从当前会话回填。两视图均仅解释脱敏实发记录，
不新增模型请求、持久化历史或权限。

## 用户轮次与唯一 Focus（#73 / #74）

用户主动发送保存的 `userEntryId` 是用户轮身份；同轮工具续跑、程序修正和原问题重试不重新冻结。每次实际 HTTP 请求仍有独立调试编号。进入新轮或重建本轮投影时，旧时间、一次性引用和当前 Focus 声明通过 Pi context edit 排除；探索入口保留其会话生命周期，普通用户、助手和工具历史不删除。

有效 `focus` 注册仅有 0 或 1 项，携带当前位置和本次 Changes/diff。全文使用独立 `materials` 注册，始终带来源路径，不重复声明当前 Focus。旧 Focus 若有 Pi 实际接收回执，将其材料/diff与课程变化降级为明确历史材料，保留原轮身份、完整 hash 链、偏移和接收回执；未送达候选直接排除。原始持久记录保留。无 Focus 的新轮不回填旧焦点；相同正文不同文件按稳定身份分别保留材料基准。压缩丢失基准仍经原机制补齐本轮正文。

验证覆盖锁版 Pi 与 localhost 实际 payload、接收回执及隔离合成会话；不代表原生桌面或真实在线模型已经验收。

## 练习引用来源清单

ContextBridge 每个用户轮注册 `exercise_sources`，将同轮 `Context.manifest` 的 `sourceId/path/version` 显式提供给模型；普通聊天的 `generate_exercises` 与专用 practice 共用此清单。路径仅用于识别材料，`sourceIds` 必须填写稳定 ID。清单不包含课程可读目录或历史授权集合，不声明额外正文已读取；空清单也明确提供。旧轮清单沿 turn 生命周期排除，重试及压缩恢复使用原冻结快照。Rust 仍按原冻结 manifest 严格校验，不从文件名或路径猜测 ID，不将未知来源变成可自动修正的授权错误。

练习编辑例外：仅冻结 Focus 的 `type=exercise,view=edit` 将该documentId及版本加入 `courseReadManifest`，不加入实际正文manifest、不自动提供参考答案。普通做题Focus仍不授该ID完整读取权。结构化题集只能经后续显式授权read取得。
