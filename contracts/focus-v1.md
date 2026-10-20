# Focus v1（M2 #12）

Focus 是本次提交的视图状态，不是学习模式，也不改变课程会话归属。前端传 `documentId`、预期正文/版本、`view`、UTF-16 可见原文范围、可选 `questionId/assetId`；身份、正文与状态由 Rust 校验。阅读按 Markdown 原文块、编辑按 CodeMirror 可见范围冻结；最多16段/8000字符。练习只提供当前题号及已作答/已批改状态，不自动注入参考答案；图片/PDF 只提供元数据。现有导入器不支持 PDF，本层不新增 PDF 正文读取。

## 唯一冻结入口与 Changes 消费

`freeze_focus { request: FreezeRequest }` → Rust `focus::freeze(core, request)`，在同一 Core 锁内刷新索引、核实 session/document/引用、形成 Context，执行一次 `FrozenMaterials::capture_project` 和 `MaterialGit::commit`，再核对清单所有材料与固定 Git 对象一致，并用该 checkpoint 的真实 compare 判定二进制；不另行 capture 或猜测编码。冻结失败不保存问题、不发送模型。返回：

```ts
type FocusSnapshot = {
  id: string; // 私有冻结记录的 SHA-256，不是任意路径
  courseId: string; sessionId?: string; documentId: string; sourceDocumentId: string;
  path: string; type: 'markdown'|'text'|'exercise'|'image'|'pdf';
  view: 'read'|'edit'|'exercise'|'asset'; binary?: boolean; // 同一 checkpoint 的 Git 判定，旧记录可缺省
  commitHash: string; materialTreeOid: string; repositoryVersion?: 2; documentVersion: string;
  excludedFiles: Array<{assetId:string;fileName:string;reason:string}>;
  paragraphs: Array<{start:number;end:number;text:string;startChar?:number;endChar?:number;totalChars?:number;totalBytes?:number}>;
  exercise?: {questionId:string;number:number;answered:boolean;graded:boolean};
  asset?: {id:string;mime:string;size:number};
};
```

`commitHash/materialTreeOid/excludedFiles` 来自同一次 Checkpoint；`sourceDocumentId` 是练习所依附的原材料，`documentId` 是实际焦点。相同正文、不同文档身份仍是不同 Focus。路径是课程材料树中的相对路径，不是任意主机文件入口。段落 start/end 是原始 UTF-16 坐标；新增 startChar/endChar/totalChars 为原始正文的 Unicode 字符坐标，totalBytes 为正文 UTF-8 字节数，旧记录可缺省。Markdown 沿精确 CRLF/CR→LF 边界映射，解析一次完整 tokens，保留 reference definitions；不能映射时不猜偏移。

同次 Context、automatic materials、课程可读目录、引用及附件路径表保存在私有 `state/focus/<id>.json`，不进入源码 Git。`focus::load(core, session, focus)` 返回同一 `Frozen`（公开 `focus/context`），校验记录指纹、课程/当前可见性、引用和固定 Git 检查点。**Changes 应消费此 Frozen 与固定 OID，不得另行 capture。** `focus::open_materials(core, focus)` 按新记录 `repositoryVersion=2` 重开该课程项目 `.git`；旧记录缺字段仅只读重开旧私有材料 Git，供 `compare/files/read` 使用；不接受前端路径，也不初始化第二个仓库。 本层复用既有 Context/Pi 投影，不另建正文缓存；Changes 消费同一冻结对象与固定 OID，Pi 继续负责每会话实际接收基准。

## 请求、运行与恢复

普通发送与快捷练习先冻结，再保存 `RequestedContext.focus`；`GenerationRequest.focus`、Run、Pi 本轮 metadata、assistant 自身来源清单保存同一对象。聚焦附件在同次 Context.manifest 登记真实身份和实际字符数；图片/PDF 登记0字符元数据。ContextBridge 的 focus turn 条目记录真实来源、版本及段落范围，与 Pi 实际清单、Run 和 assistant 同源。Git 判为 binary 的自动材料从 Context 正文及 nativeMaterials 一并移除，清单保留身份/版本并记0字符；Focus.paragraphs 为空。ContextBridge 的 metadataOnlySourceIds 只使 Pi 排除同来源的旧自动材料投影，不擦除普通会话、主动工具结果或用户明确引用。支持的 UTF-8 Markdown 即使 Git 判 binary，已授权主动 read 仍按原固定 OID 返回；自动投影规则不扩大或取消 read 档位。附件 read 依据已验证 MIME 仅支持文本/Markdown/CSV/JSON，图片字节即使可解码为 UTF-8 仍拒绝；PDF 导入未支持。真实材料读取和 query/read 工具按本 Run 的固定 OID；权限档位、回收可见性和课程归属仍独立校验，不因历史快照扩大授权。

切页后的进度只更新该 operation 保存的文档来源显示，不能把最新页面当作旧请求状态。取消不删除快照；重试沿原 Focus/引用，工作区编辑不会混入旧请求。未知结果沿原 operation 核对。旧记录缺 Focus 明确显示未记录；既有无课程 legacy 会话保留兼容发送路径，不伪造 Git Focus。

Git 投影同时支持应用实际生成的 `exercises/<id>.md`，仅该明确 kind/path 组合映射为 `documents/<id>.md`；其他非约定路径仍拒绝，课程祖先链校验不变。

验证使用临时合成库、真实 Git、React/mock IPC、锁版 Pi 与 localhost fixture，分别报告。当前不做 GUI；重开界面回归使用 jsdom，不能称实际桌面验收。删除本层提交即可停用新入口；保留私有 Focus/Git 数据，不自动清库或改权限。

课程允许多个独立会话；新冻结记录写入 sessionId 并校验相等，同课程也不得跨会话重用。旧记录缺省字段不改写指纹；缺 sessionId 的旧快照只有原会话已保存问题或回答中存在同一 Focus 指纹才可恢复，否则保留草稿并要求重冻，其课程/引用/权限校验继续生效。本字段不把正文接收基准提升为课程级。

新冻结只使用 `library/courses/<courseId>/materials/` 与同课程 `.git/`；不创建第二份材料或共享课程间 Git。未迁移的旧平铺课程按目录接口返回 `COURSE_PROJECT_REQUIRED`，不复制旧材料。`Frozen.course_read_manifest` 是同一检查点验证过的授权目录，`Context.manifest` 是本轮实际正文/明确元数据，禁止互换。检查点排除的文件不进入读取授权。当前文本自动提供完整固定原文，paragraphs 保留可见位置；不沿链接或 requiredContextIds 自动展开其他笔记。显式全文引用当前材料只保留一份正文，精确引用仍保留定位。图片/PDF、练习和 Git binary 不自动提供正文；支持的主动读取继续执行原类型与授权边界。冻结后生成与重试直接使用该目录，不能随后读取当前目录更新版本。

主动关联查询的候选来自同次 Frozen.course_read_manifest，不能用实际正文清单排除未自动发送的授权资料，也不把目录的0字符当读取预算。查询复核当前课程归属、可见性和普通资料类型，再按该Focus固定OID及版本读取；每次最多扫描32份/60000码点，最多返回5项、每项2000字节，截断如实标记。新增或已撤权来源不随旧请求进入授权，查询不反向加入初始正文。
