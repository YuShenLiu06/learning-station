# ContextBridge（上下文桥）

修改上下文相关代码前先读本页；行为变更同步文档、[清单契约](../contracts/context-bridge-v1.md)与[原生会话契约](../contracts/pi-native-session-v1.md)。

## 职责与实际调用

Rust 核实资料授权、版本与显式引用；桥只冻结、去重、排列和限制片段。Pi 1.3.0 的 `SessionManager`/`AgentSession` 管模型消息、工具循环、压缩和指定文件恢复。Rust 的学习记录继续供界面、复习与回执使用，不再投影最近16条/单项5000/总计20000字符的历史到新 user JSON。上下文主链：底层接口（file_metadata/versioning）→ Focus 拼接 → ContextBridge 注册 → Pi 上下文 → 最终模型请求，底层接口契约见 [文件元数据与版本接口](file-metadata-and-versioning.md)。

```mermaid
flowchart LR
  A[提交问题] --> B[Rust冻结授权与资料版本]
  B --> C[ContextBridge材料与本轮状态]
  C --> D[指定原生会话文件]
  D --> E[Pi实际上下文投影]
  E --> F[普通用户问题与原生工具循环]
  F --> G[Rust真实业务回执]
  G --> F
  F --> H[Pi持久消息与必要压缩]
```

`generation::start` 调用 `snapshot/freeze`，私有 stdin 传递 `nativeMaterials`、`nativeReferences`、本轮元数据与问题。适配器把材料/本轮状态写为明确的不可信 custom message，把本次问题写为真实 user 消息。旧模型历史由 Pi 恢复，不在此 JSON 中重建角色。批改、连接测试等原有无历史单轮任务保持隔离。

普通聊天与探索使用同一学习工具策略；工具集合与 Skill 正文、Focus/Citation/Changes/Time 的投影分开，制卡不需模式切换或先加载指导。question/explain 仍兼容已有请求与记录，不再注入“当前模式”作为聊天行为分流；场景指导沿现有单一快照入口提供，本次不新增上下文投影或会话映射。

课程内可以建立多个独立会话；`Session.courseId` 固定归属，`activeSessionByCourse` 只保存用户选择，不授予材料权限。旧固定课程会话及其 Pi 绑定保留为已有一条，不重编号、不拼接历史。创建、选择和改名不切换阅读文档；改名核对完整会话版本。

课程聊天请求的 `documentId` 为空或省略时不自动注入当前笔记或首次锚点。显式探索材料时才传真实文档编号并沿用冻结快照；附加引用也须属于本课程，独立探索保留已有显式引用语义。普通课聊没有 Focus 时不因切换笔记或会话增加来源；有效当前文本的自动 Focus 提供同次冻结完整正文。无明确材料来源的练习在模型调用和工具写入前返回 `SOURCE_REQUIRED`，不伪造练习父节点。

## 生命周期

| 内容 | 行为 |
|---|---|
| 固定规则与学习指导 | 显式应用规则；课程cwd沿Pi发现父级AGENTS/skills等文本，不加载外部可执行扩展或默认编码工具 |
| 时间、问题、探索目标 | 每个用户回合冻结；本轮工具续跑不重新读取用户状态 |
| Focus与明确提供的材料正文 | 开局不自动遍历；版本与差量经 versioning 接口取得（见[文件元数据与版本接口](file-metadata-and-versioning.md)）：file_stat 按未变/A/M/D·binary 定档，首见全文、有效完整基准后 file_diff 差量，无变化不重复，压缩或新session缺基准时恢复全文 |
| 手动、AI回答、练习分区引用 | 沿既有真实问题保存回执消费；模型失败重试使用原快照；旧 turn custom 通过原生 context edit 排除，不自动再注入 |

Core 锁覆盖应用写入并复核外部文件版本，不是多文件系统事务。冻结后新编辑属于下一回合。

## 基准与压缩

Rust 生成有界 full/diff 候选，并保留当前已授权的完整范围文本供重载。适配器检查 Pi `buildSessionProjection()` 中真实可见的 full→diff 哈希链：未变不新增；小改只在完整链仍在时追加；链缺失、部分范围、分支或压缩丢失 full 时重载正文。不能仅根据本机缓存存在就宣称模型仍有基准。Focus 部件即此形态：元数据＋版本hash＋diff（首见或丢基准时全文）＋课程级 stat 已并入同一部件（【Focus：当前文件】），diff 基准链随 Focus 条目保存；envelope 不再携带 paragraphs 正文与坐标投影，生命周期机制不变。

Pi 在已配置上下文容量时执行原生阈值压缩，通过同一受控模型/协议/凭据与输出预算产生摘要；摘要请求零工具、无自动换模型或服务。未知容量保持未知，不猜规格，不启用阈值压缩。新摘要完成后重新核验并补齐本轮必要材料。范围不完整时明确只含前缀；从未展示的尾部不算已读。

资料授权集合减少或回收历史保护触发时，使用明确的原生应用权限边界排除先前消息/摘要，再加入本轮授权资料。旧原始记录仍保留，未删除用户数据。主动 `query_related_knowledge` 仍返回授权范围内真实片段，不使用缓存标记代替读取。

## 同源清单

`Run.courseReadManifest` 单独保存课程可读来源与提交版本，不能渲染成已发送或已读来源；`Context.manifest` / `Run.contextManifest` 仍只列实际正文。Rust read 的 `delivery=tool-result` 与 `range` 描述本次实际回包，不表示远端已接收；Pi各session的工具消息、receipt及真实投影决定正文基准，不新增缓存。

Pi历史授权集合独立沿用课程可见来源，兼容旧course/folder/exercise编号，不以可读正文目录替换；仍授权的历史/摘要在升级时保留，真实移出课程、回收或撤权才建立权限边界。

Pi 确定实际材料投影后，把同一个 `contextBridgeManifest` 放进 turn custom message，并经私有管道交 Rust 写入 Run；答案保存时复制到该 assistant 自身。queued 初始值不冒充实际清单。界面不从最新 Session 回填旧回复，不新增路径打开。

此清单说明桥提供的资料/引用/状态，不是 Pi 全部原生历史消息目录，也不表示服务端已读或保证总 token 下降。预览或旧记录缺字段明确未记录。

## 持久化与失败恢复

稳定学习会话 ID 映射到私有 `pi-native/<hash>/transcript.jsonl`，只打开该文件，不用 `continueRecent`。首次保留旧 Markdown 与结构化备份；旧记录以明确引用导入，不补造缺失工具调用。

Node 继续使用权限模式，只能读运行组件及读写当前私有会话目录，无 child-process 权限。Node 24 在该模式下禁止 fsync，因此关键检查点经私有 `session_sync` 交 Rust 同步文件和目录，收到确认后才继续模型请求、工具执行或完成回执。

工具调用归属先持久化；Rust 业务提交后先保存完整回包日志再回复 Node。中断恢复先读原回包，缺失时只查询真实业务记录；无证据保持 unknown，绝不重做写操作。新业务操作 ID 绑定真实已保存问题、工具名与精确参数，因此同一问题恢复后即使换 run/call 编号，相同写入仍返回原对象；旧无问题绑定的记录保留原编号语义。read 的旧回执不能证明当前授权：每次重放或恢复（含已有原生回包）重新核实当前来源、课程、回收状态、原轮及本轮冻结版本与回执版本；不一致仅返回无正文错误，旧回执保持原样。明确读取拒绝配对为工具错误后允许继续，结果未知仍停止；write/bash 的持久幂等与只读恢复不变。映射/原生文件缺失、损坏、部分 JSON 行或符号链接均报错，旧 Markdown 不被覆盖。失败或取消不推进 Rust 资料基准。

## 验证边界

[README](../README.md)区分实际锁版 SDK、Rust→受限 Node→Pi localhost、React/mock IPC 和包资源验证。没有新增原生 GUI、真实在线模型、Anki、飞书或鼠标/IME 验收。

课程内自动 Focus 已按 [Focus v1](../contracts/focus-v1.md) 冻结身份、视图与固定 Git 材料，沿原请求用于重试与来源显示；无课程 legacy 会话保留兼容路径。通用网络权限未新增。HTML 预览只改变阅读视图，不增加模型来源授权；嵌入原文仍作为普通不可信笔记内容，远程网页/视频不抓取或自动加入上下文。原版本投影参考保留 [pi-readcache MIT许可](../THIRD_PARTY_NOTICES/pi-readcache.txt)，未安装插件。

最终练习 JSON 的修正沿同一原生会话和冻结来源进行，程序校验消息不作为用户新问题，也不重新冻结引用或增加来源授权；最多两次修正，来源越界不自动修正。详见原生会话契约的 `validate_output`。

Citation 沿用本页的保存回执与一次性生命周期，三类精确选区的位置/版本见 [契约](../contracts/citation-v1.md)。显式引用的必要邻段、同题已保存作答和状态按 [补充契约](../contracts/citation-context-v1.md) 单独冻结，保留真实范围、版本及截断标记；不会把补充当作用户原选区。引用选择本身不创建 Git；未交卷练习仍只读取经分区校验公开的原文。

## 课程归属与本轮材料

课程会话的稳定 ID 和本轮 `documentId` 分开：同课允许多个独立 Pi 对话；切换笔记/练习保持当前聊天，本轮材料、选区及练习落盘使用提交时的文档。旧练习页面沿原账本读取题目和作答，不把参考答案自动带入上下文。课程旧对话备份、历史入口与兼容行为见原生会话契约“课程会话路由”。统一 Focus 身份、视图及固定 Git 版本见 Focus v1；同一课程各对话分别维护原生实际接收基准。

练习页的本轮来源须通过原始账本映射校验；页面来源、origin 或 entry 被改写时在模型请求前报 CONTEXT_CONFLICT。请求接受后使用 Rust 保存的归一化 Run.sourceDocumentId；在途页面编辑不能改变练习落盘目标。跨课程移动的练习可读取原账本，但分区注入另按当前课程校验，不拼接原课程聊天。共享会话按 courseId 回收，单篇回收的历史排除边界按来源集合摘要记住，后续合法回合不会每次清空。

## 请求调试（M2 / #52）

唯一开关与查看入口为「设置 → 请求调试」，默认关闭，不写入偏好。Pi 的
`createControlledFetch` 在最终 `Request` 送入底层 fetch 时记录其真实 JSON
payload；不是 ContextBridge 预览，也不重新生成上下文。OpenAI / Anthropic、
工具后续、压缩与校正均复用同一边界，不增加模型调用。请求编号关联课程、
会话、request/run、独立 attempt UUID、阶段、模型与时间；HTTP 状态和运行结果
分别显示。失败后重试仍是新的实际发送记录，未发送的校验失败不生成条目。

Rust 每次发送前提供易失采集许可；关闭后清空并拒绝旧许可的迟到事件。
离开会话清除对应记录，旧会话在途请求不继续采集；重新选择后从新记录开始。
仅保留最近 40 条且总计最多 4 MiB，单请求体最多 256 KiB；超限整个 body 省略，
明确标为不完整，不将前缀冒充完整。diff 只比较同一 session 的上一实际请求，
基线淘汰或不完整则说明无法比较。工具名、参数和请求消息均来自该 payload。
正文视图将消息内 JSON 展开为路径和原文，便于核对 Focus、引用补充和 Changes。

认证头及未知头默认隐藏；结构化敏感字段、请求内存中已有 API key、常见认证
文本与重复凭证值在 sidecar 发出 IPC 前脱敏。没有原值显示、复制按钮、导出、
持久化日志或上传通道。用户可选中已脱敏文字；课程正文仍是私人内容。

### Debug 来源与注册键

「按来源 / 注册键」直接显示 `ContextBridge::add` 接受的 key、lifetime、position、
value；生产当前为 stable/materials/references/turn/question 五键。注册值是冻结输入，
不能当作最终发送投影。值超过 8 KiB 明示省略；关闭调试时不生成该内容快照，
调试晚于冻结开启或内部消息预算不足时可能缺少注册快照，界面明确说明。

nativeMaterials/nativeReferences/manifest、Pi 材料与 Changes 投影、harness 的系统说明
不因此被冒充为命名注册。来源定位从原生 projection 的真实 customType/消息角色及
Pi system.sections 取得，绑定到该次最终 Request 的字段/片段；不按正文关键词分类。
未匹配或同内容来源冲突标为未知，用户消息、工具返回、模型回复单列。

正文只呈现 messages/system，工具定义单独折叠，原始请求及完整 payload diff 保留。
桥内 JSON 仅按正常 JSON 解析展开，显示字符串真实换行，不全局替换代码字面量 \\n。
本批使用可折叠可读 log 和文字来源标签；未新增 JSON 高亮依赖或更改模型提示。
