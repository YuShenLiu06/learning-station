# Pi 原生会话 v1

原生会话部分仅使用应用内部私有管道，不新增模型权限或任意路径接口。锁定 `@earendil-works/pi-coding-agent`、agent-core、pi-ai 均为1.3.0。

## 持有关系

- Pi：原生消息、工具调用与结果配对、分支和压缩、`SessionManager.open` 指定文件恢复。
- Rust：Markdown/学习对象、授权与版本快照、Keychain、模型配置、准确映射、运行与业务回执、落盘同步。
- React：沿用原 IPC 与来源快照；不读写 Pi 文件或密钥。

`RequestContext.nativeSession` 内部字段：`directory`（Rust已核实的私有目录）、`learningSessionId`、`runId`、`userEntryId`、`authorizedSourceIds`、`protectHistory`、可选 `historyBoundaryKey`、`legacyRecords`。前端不能提供这些路径或扩大范围。

课程项目适配另支持可选 `projectRoot` 与 `agentDir`：课程根只作为 Pi cwd，`directory` 仍为当前 session 专属存储。Rust 核实 canonical 真实目录，并显式提供允许的用户 agentDir；缺省保持旧会话资源行为。课程业务接入必须消费 `course_project_root`，不能接受前端任意绝对路径。普通开局不遍历笔记全文；Focus 仍经现有 `nativeMaterials` 投影接收全文或差量，显式引用继续走 `nativeReferences`。已收到的正文与真实工具历史保留，不因没有新的 Focus 重置基准。

Changes 接口提供首次全文、后续完整 Git diff。显式引用继续走 `nativeReferences`。`metadataOnlySourceIds` 只排除所列来源的旧自动材料投影；不会仅因材料缺少 Git 身份而清空有效历史。其余正文、会话与真实工具历史按既有投影及授权边界保留，撤权修复由独立 PR57 收口。

Changes 私有适配发送 `material_changes {id,baseCommitOid,focusBaseCommitOid}`：前者来自本 session 原请求核对后的课程变化通知，后者来自当前有效完整正文投影的 Git commit，缺失为 null。两者独立，A→B→A 不把 B 时收到的通知当作 A 正文。复用原生材料条目的 full→diff 链与压缩恢复，不增加缓存；基准缺失/不完整时恢复全文，无变化不重复。Rust 固定 OLD/NEW 比较通过已提交 Focus 课程仓路由接入，同一Frozen供generation与for_focus使用；课程变化通知与聚焦 diff 现随 Focus 条目（learning-station-focus，details.materialFrames）一并落笔，不再有独立 learning-station-changes 消息；不在Rust保存新的正文接收缓存。

项目资源使用锁版 Pi 的 `DefaultResourceLoader`：保留父级 AGENTS 与课程/用户 skills、prompt templates 等文本发现，使用显式应用 system prompt override，不另造 parser 或缓存。SettingsManager 仍为内存配置，不加载外部可执行 extensions、自动包安装或用户凭据。原生 `/skill` 展开保留原始输入哈希与实际 user entry 映射，同一问题重试复用已接受的展开文本。

cwd 与 Node 目录读取参数都不是强隔离：真实锁版 read/Bash 可读取兄弟 fixture，Node 24 的预存 symlink 也可绕过目录范围。基础原生 read/Bash 不启用，现有 Rust 工具校验不变；不新增 sandbox/插件。Node 只增加课程、父级说明文件候选与允许用户文本资源的只读范围，不增加写入或 child-process 权限。上述行为用合成 fixture 验证，不代表 GUI、真实模型或课程整合已验收。

每个学习会话使用 `state/pi-native/sha256(sessionId)/binding.json`，绑定固定 `transcript.jsonl`。绑定不存在但目录非空、已初始化文件丢失、格式损坏或符号链接均失败。Node 独占锁只恢复已确认死亡的旧进程；不能抢活跃会话。

首次迁移先不可覆盖地保存 `legacy-backup.md/json`；原 Markdown 保持原位。旧业务记录只作为明确标注的不可信引用，不冒充真实旧 assistant/toolResult。迁移失败保留原文件与备份，旧验收包仍可打开原业务库；不得删除未知文件来“修复”。

## 私有事件

| 事件 | 处理 |
|---|---|
| `ready.nativeSession` | `engine=pi-coding-agent`、`version=1.3.0`、`persistent=true`及准确学习会话ID；Rust校验 |
| `session_sync {id}` | Rust同步当前已绑定目录中的普通文件和目录；同id `tool_result.result.synced=true`后继续；无路径参数 |
| `context_manifest {manifest}` | 实际原生材料投影的清单；写入当前Run，随后该assistant自身快照 |
| `session_lifecycle {event,reason,aborted}` | 压缩开始/结束的安全状态，无消息正文或凭据 |
| `tool_recover {id,name,args,recoveryRunId}` | 核对同学习会话原run、原操作ID和已提交对象，只读，不执行原工具；无证据返回unknown，有台账但工具或参数不一致返回CONFLICT；read同时核实当前权限及原轮/本轮冻结版本 |

普通 `tool_request/tool_result/delta/done` 继续原协议。工具结果未回到 Pi 不代表业务未执行。Rust 在发送回包前持久化 `tool-sha256([runId,callId]).json`；恢复优先用原回包，必要时查询真实业务对象。新业务操作 ID 取 `sha256([saved-question-v1,sessionId,userEntryId,toolName,canonicalArgs])`，同一问题换 run/call 重试仍返回原对象；旧无 userEntryId 的记录沿原 run/call ID。原生已接受用户问题按 `userEntryId` 登记实际 native entry ID，失败重试不重复用户原话。

## 模型及资源

固定显式 ModelRuntime、内存凭据、`modelsPath=null`、禁止模型目录网络刷新。旧会话资源加载器返回空外部扩展/模板/全局指导/Agents文件，应用仅传既有学习指导；课程项目按上节复用原生文本资源。基础工具表为空，精确开放应用任务策略允许的工具；普通聊天与探索（兼容 question/explain）共用完整学习工具集合与每轮6次预算，始终包含 create_review_card/create_error，不以 Skill 加载作为开关。Rust 的策略构建与校验复用同一白名单，Pi 独立校验并注册相同集合；重开会话按当前策略重建工具表。制卡仅保存本地卡片，不能据此宣称同步 Anki；`edit/grep/find/ls/powershell`均不注册。`read/write/bash`仅在本机设置的工具授权档位（off/read/write/bash，默认off，仅question/explain模式）开启后受控注册，随toolPolicy.authorization由Rust下发；参数与结果仍由Rust校验，read只读本轮manifest已授权来源或会话工作区文件，write在课程内写当前课程materials（路径/配额/符号链接受限），无课程时保持会话工作区，bash只在会话工作区以受限环境执行（固定PATH、无HOME、超时与输出有上限），write/bash以持久回执台账幂等，恢复只读台账不重执行。Node权限模式不允许创建子进程（bash由Rust进程执行，不经Node）。

`list_learning_skills`/`load_learning_skill`是仅question/explain/practice模式注册的只读工具，不挂任何授权档位：它们只读随包 `skills/` 固定允许清单（三个稳定id），复用主动快照同一路径校验（根内canonicalize、拒符号链接、16KiB、frontmatter合法），目录仅返回id/名称/描述不含正文，正文经工具结果通道返回且受工具结果预算约束，超出按SKILL_INVALID单工具拒绝。清单外或含路径成分的 id 分别以 TOOL_SCOPE／INVALID_TOOL_INPUT 拒绝，均不触盘。同run重复load同一指导只返回简短提示（含sha256）；该指导已随模式或工具注册注入时返回"已在指导中"，不重复正文；去重台账按run维度挂Rust，新run或该run压缩完成后可重新取正文。ready事件的learningSkills版本回执语义不变，仍只认主动注入快照，按需加载不进入该回执。

摘要与普通生成走同一受控端点和预算，不跨域、不自动换模型。容量未知保持0，不开启阈值压缩；用户填写容量后使用Pi原生阈值生命周期。正常输出用尽不盲目重发。私有状态目录路径不发给模型，凭据不进原生会话。

## 限制与恢复

原生文件有32MiB读取保护，不静默截断或删除；超过限制需保留原数据后明确处理。业务回执无法确认时会话停在unknown，不能改编号绕过。失败恢复的回滚路径是保留此私有目录、继续使用未改动的原Markdown与旧包；本轮不提供破坏性自动清库按钮。

## 最终练习 JSON 的有界修正

Rust 在 practice 模式显式开启 `outputValidation`，Node 每次完成最终文本后发送只读 `validate_output {id, content}`；Rust 使用原轮冻结 manifest 校验，回复 `tool_result.result={valid,recoverable,message?}`，不保存对象、不增加权限。两端均最多接受三次校验（首次及最多两次修正），沿用原请求超时与工具预算。

可纠正错误通过 `learning-station-output-validation` 原生 custom message 回传 Agent，包含安全字段路径、原因与长度限制，不伪造新用户问题。未知来源或权限错误不可自动修正；失败原文留在私有原生会话，最终失败文本留在Run。仅校验通过才标记原生记录complete；Rust保存前再次校验并沿既有操作编号幂等写入。该私有事件不注册为模型工具或前端IPC。

## 课程会话路由（M2 / #11）

显式 course 根下允许多个业务 session，按 `activeSessionByCourse[courseId]` 恢复用户选择；旧 `sessionByCourse` 仅保留原有会话兼容映射，不再约束数量；Pi 的目录、header ID、精确重开与幂等协议仍由该业务 ID 决定。无课程的旧笔记和独立探索保持原行为。`Session.courseId` 是固定归属，`sectionId` 仅保留创建时锚点，不能代替本轮材料。

`load_session_context` 与 `start_generation` 接受可选 `documentId`；未提供Focus的课程普通聊天为空或省略时不自动附带笔记正文，探索、明确引用及练习生成另传真实来源，经保存问题、重试、生成和练习落盘保持不变。无课程旧会话省略时兼容旧锚点。Rust 检查课程归属；练习页面读取来源材料，不将含参考答案的页面正文自动注入。当前来源决定工具及练习目标，切页不改已提交请求。

首次承接旧课程时，先将原 preferences、业务会话目录和对应 Pi 目录备份到私有 `course-session-backups/<sha256(courseId)>/`，完整回执后才建立新课程对话。原对话及来源保持原样，`legacySessionIds` 供界面提示和历史查看；不选一段旧对话覆盖其他记录，不拼接 Pi 历史。备份遇到运行锁、符号链接或读写失败时不切换路由。旧练习仍按 originSessionId 作答/批改，聊天使用课程 session；引用校验同时核实原账本与课程。

Focus 切换时，native 的历史授权集合保留仍属于该课程的已见来源，兼容旧scope中的course/folder/exercise编号；它独立于仅含可读材料和附件的courseReadManifest，升级不得因目录类型减少而排除仍授权的历史/摘要。显式材料引用也核对同课程归属；移出课程、回收或真实撤权仍触发历史保护。此集合不注入其他笔记正文，不扩大 Rust ToolScope/read 工具的本轮来源列表。

read的持久回执只证明原读取，重放或恢复前（包括Node已有原生回包）必须由Rust重新核实当前授权、来源归属、可见性、原轮及当前轮冻结版本和回执版本。版本变化不重写原回执，不静默换成新正文。明确的TOOL_SCOPE/CONFLICT/TOOL_NOT_ALLOWED/TRASHED/NOT_FOUND拒绝以无正文工具错误配对，允许进入权限边界并继续会话；原轮/本轮Run缺失或损坏、回执证据缺失仍为unknown，不配对为资源读取拒绝。write/bash沿原幂等与只读恢复协议执行，不因当前档位改变而重复操作。

### 来源、移动与回收边界

练习页面是可编辑视图。来源解析核对稳定页面 ID、originSessionId、exerciseEntryId、页面摘要与真实 exercise entry；来源取自原账本，不一致返回 CONTEXT_CONFLICT。旧账本缺来源时才兼容原单篇锚点。请求接受时 Rust 把归一化来源保存到可选 Run.sourceDocumentId 与问题记录；在途改页、切页和结果保存不重新选择来源。

工具入口核对真实 run 的冻结 manifest、归一化来源和当前课程归属；原有档位、路径、版本、配额及回执约束继续执行。错题按原题账本取源，手动复习卡保存创建来源，旧单篇缺字段才回退原锚点。

材料连同练习子树跨课程移动后，原账本及 Pi 历史保持原位；只读题目/已保存作答/公开反馈核实原账本，注入当前对话时另核实目标课程。单独跨课程移动练习在修改前拒绝。其他课程仍有活动练习依赖原账本时，回收原课程返回 SOURCE_IN_USE，先处理依赖或移回来源材料。

共享会话按固定 courseId 回收；单篇材料仅收纳该材料、练习及明确来源的复习对象。整课程回收和恢复保留稳定会话 ID 与准确 Pi 绑定。课程历史保护的 historyBoundaryKey 是已排除来源集合的摘要；同一回收边界只排除旧历史一次，后续仍获授权的对话可继续保留。来源撤权或排除集合变化会重新建立权限边界，旧原始记录不删除。

恢复也按本次提交后的可见性核对练习原 session/entry：同批恢复的账本允许，仍在其他回收记录的依赖返回 DEPENDENCY_TRASHED 及明确恢复目标。失败保持原回收记录，同 requestId 可在依赖恢复后重试；不读取隐藏账本给模型，不隐式恢复原课程或改写 Pi 绑定。

## 课程多会话业务 IPC

- `course_sessions(courseId)` 返回 `{courseId, activeSessionId, sessions: DocumentSummary[]}`。
- `course_session_create(courseId, requestId)` 幂等创建无材料锚点的新会话，并选中它。
- `course_session_select(courseId, sessionId)` 核对固定 `Session.courseId` 后选中已有会话。
- `course_session_rename(courseId, sessionId, title, expectedVersion)` 核对 `Session.version` 后修改名称；返回已有 `Session` 结构。

创建与选择也返回 `Session`，仅更新会话偏好，不修改阅读文档。bootstrap 增加 `lastSessionId`、`lastCourseId`，原有字段继续保留。原 `sessionByCourse` 保留旧固定会话映射；新 `activeSessionByCourse` 记录每课程最近选择。旧会话 ID、原生私有目录及 Markdown 历史不变，新会话使用独立 Pi 绑定；无选择且存在多个候选时明确要求选择，不以最新文件猜测恢复目标。

探索额外上下文复用 `ExplorationContext`，由业务入口输出 `origin=exploration-entry` 和 `originalUserEntryId`；原问题取自与确定性会话、笔记及条目ID匹配的真实入口条目，不按模式、目录或第一条用户消息猜身份。无真实标记时不生成额外场景，普通课程会话不携带此上下文。旧持久记录新增字段默认空，保留兼容读取。
## 本轮 Focus

本轮 metadata 携带 [Focus v1](focus-v1.md) 的冻结对象，含课程、焦点文档、原来源文档、视图及固定 Git OID。它与本次 nativeMaterials 来自同次冻结；重试沿原 Focus，Pi 仍独占消息/压缩管理。本层不建立材料接收基准。

Focus 的 `metadataOnlySourceIds` 由 Rust 冻结 Context 生成并限制在本轮授权来源内。Pi 排除这些来源此前的自动材料投影，保留普通历史与用户主动引用；后续恢复文本时重新载入真实完整基准。Git binary 判定仅控制自动投影，不替代 read 工具的类型与授权校验。

## 课程工作区写入（#64）

`read.path` / `write.path` 对课程会话以当前课程 `materials/` 为根，拒绝绝对路径、上级路径、隐藏路径和符号链接组件。`documents/<id>.md` 只允许本轮授权的笔记类来源，read返回正文及业务version；write.content只替换正文，保留稳定ID、归属、类型、来源及其他元数据。创建业务笔记仍使用既有学习服务。附件目录和课程/练习元数据不开放raw写入。

普通材料文件可在根下新建，进入既有课程Git变化清单，后续可按path读取；不冒充侧栏业务笔记。按真实文件名写入UTF-8文本，不限制扩展名；路径读取也不以附件类型白名单拦截，仍校验UTF-8与容量。保存不等于支持应用内预览；不支持预览时应提示可在对应文件目录用外部程序打开，不自动执行文件。raw Markdown不得携带有效业务frontmatter；与Git捕获共用路径排除规则（含大小写别名），不纳入版本的路径在写入与intent前拒绝。覆盖必须传read返回的`expectedVersion`，新建省略；缺版本覆盖、旧版本、跨课或未授权来源均拒绝。旧无课程请求继续使用原会话工作区。Bash目录不随此次write路由改变。

回执仍在私有session账本中，完成回执重放不检查当前内容也不重写文件。课程写入前持久化intent；有intent而缺完成回执视为unknown，不盲目重放。Git沿既有versioning接口记录材料，不在模型参数中接受仓库或绝对路径。普通路径读从同一次不可变快照返回正文与版本，sourceId读取继续遵循原冻结manifest契约。

覆盖前还要求原正文能够在当前 read 字节及 JSON 回包预算内完整呈现；超限返回 TOOL_SIZE_LIMIT，保留原文件且不创建 intent，不提供隐式截断覆盖。课程路径沿 Git 材料准入（支持中文、空格、深层目录），旧无课程工作区继续使用原 ASCII/层数限制。intent 持久化后，只要完成账本仍不可核实，包含文件保存、同步、版本登记及最终回执落盘在内的失败统一报 NATIVE_TOOL_UNKNOWN。

### 冲突副本与中断恢复

macOS/Linux 课程 AI 写入（普通材料和业务笔记）共用 dirfd 发布事务。直接依赖既已锁版的 rustix 1.1.5 `fs` API；从 `/` 开始逐级 `O_DIRECTORY|O_NOFOLLOW` 打开授权目录，以已打开父目录对象执行所有文件创建和 rename，不在发布时重新解析用户路径。其他平台在保存写入意图前明确拒绝此发布路径，没有不安全降级；不引入沙盒或把 advisory lock 当隔离。

每次写入在目标父目录保留 `.learning-write-<原操作ID>/`：`manifest.json` 为持久意图，`new` 为独立的新稿副本，覆盖时 `old` 是原子移入的实际旧对象。先同步新稿/意图和目录，再以 NOREPLACE 移入旧对象，核对实际旧稿是否仍等于基线；匹配才用 NOREPLACE 发布另一份新稿，第三方文件占位时不覆盖。发布版本取自待发布字节，业务元数据仍由 ContentStore 规范化。成功后也保留副本，不自动清理。受管恢复目录经过符号链接与范围检查后不占用 4096 个材料条目配额；总目录扫描仍限制为 65536 个条目。会话 pending 与完成回执同步文件及父目录（含新建祖先目录）后才进入下一阶段；pending 目录同步失败不发布正文。旧会话的小节在移入 old 后暂时缺失时，仅使用已持久化且与当前 sessionId/sectionId 匹配的课程归属证据核对该原操作，当前显式课程变更仍拒绝访问。

版本在最后窗口改变、发布或回执失败时保留旧/新稿；若目标仍为空，尽力用 NOREPLACE 恢复旧稿副本，绝不覆盖第三份。`NATIVE_TOOL_UNKNOWN.details` 返回相对于资料库的 `recoveryPath` 和只读状态（存在性、长度、哈希）；原操作恢复只核对 `target/old/new/manifest/published`，不重做写入。中断可能短暂留下空目标；状态无法读取时明确 unverified。没有自动恢复按钮，用户可在资料库下打开 `recoveryPath`，先把 old/new 复制到资料库外比较保管；确认目标缺失时再将选定完整稿恢复至 manifest 所列原文件名。目标已存在时先保留第三份并比较，不直接覆盖；业务 Markdown 须保留完整元数据头，勿把恢复副本当新笔记导入造成重复 ID。

能力边界：dirfd 绑定的是核验时的授权目录对象；恶意同 UID 进程任意搬走目录、篡改恢复区或持续写入持有的旧文件描述符不属于强隔离保证。原子 rename 保存方式的竞争文件会保留，持续原地写旧 inode 不被描述为不可变快照。本方案提供冲突保留与可定位恢复，不宣称任意外部写者下的原子版本 CAS。

## 用户轮次与唯一 Focus（#73 / #74）

用户主动发送保存的 `userEntryId` 是用户轮身份；同轮工具续跑、程序修正和原问题重试不重新冻结。每次实际 HTTP 请求仍有独立调试编号。进入新轮或重建本轮投影时，旧时间、一次性引用和当前 Focus 声明通过 Pi context edit 排除；探索入口保留其会话生命周期，普通用户、助手和工具历史不删除。

有效 `focus` 注册仅有 0 或 1 项，携带当前位置和本次 Changes/diff。全文使用独立 `materials` 注册，始终带来源路径，不重复声明当前 Focus。旧 Focus 若有 Pi 实际接收回执，将其材料/diff与课程变化降级为明确历史材料，保留原轮身份、完整 hash 链、偏移和接收回执；未送达候选直接排除。原始持久记录保留。无 Focus 的新轮不回填旧焦点；相同正文不同文件按稳定身份分别保留材料基准。压缩丢失基准仍经原机制补齐本轮正文。

验证覆盖锁版 Pi 与 localhost 实际 payload、接收回执及隔离合成会话；不代表原生桌面或真实在线模型已经验收。
## 工具调用与容量（#72）

ToolPolicy 只携带 mode、allowedTools、authorization，不再携带 maxCalls。Rust 与 Pi 均不对学习工具调用、原回执恢复或材料变化查询设置固定次数预算，普通聊天与探索共用此规则。工具白名单、用户取消、请求超时、参数/结果字节上限和写入版本/恢复约束保持有效；参数或结果容量超限使用 TOOL_SIZE_LIMIT，不再误报调用预算。练习最终输出保留原有至多三次校验修正，独立错误码 OUTPUT_VALIDATION_LIMIT，不参与工具调用计数。
课程 `write` 的 #71 审阅状态：`proposalPersisted=true` 仅证明修改建议已保留，`persisted=false,status=pending` 不代表正文写入。生成 handler 等待用户决定；接受返回真实写入回执，拒绝返回 `persisted=false,status=rejected`。停止/超时保留提案，恢复时 pending 为未决错误，不作为完成回执注入 Pi；不自动接受或延长模型预算。无课程 scratch write/Bash 不纳入此文本审阅承诺。

#71 接受前要求初始 ledger 完整且身份匹配，缺失/损坏不发布但可拒绝后重请。`accepting`/`unknown` 查询已执行结果不依赖当前 write 授权，不能借核对重放发布。接受/拒绝均结算原 session 下同一保存问题、相同完整参数及操作 ID 的所有已登记 run/call 工具活动，不要求原生成或重试仍运行；不同问题或参数的未决调用保持隔离，旧无保存问题绑定的提案仅匹配原 run/call；终结提案归档后继续按原 ID 恢复，未决列表不返回历史终结项（旧平铺终结项首次遇到迁移）。该流程不引入新的工具数量上限。

## 练习引用来源清单

ContextBridge 每个用户轮注册 `exercise_sources`，将同轮 `Context.manifest` 的 `sourceId/path/version` 显式提供给模型；普通聊天的 `generate_exercises` 与专用 practice 共用此清单。路径仅用于识别材料，`sourceIds` 必须填写稳定 ID。清单不包含课程可读目录或历史授权集合，不声明额外正文已读取；空清单也明确提供。旧轮清单沿 turn 生命周期排除，重试及压缩恢复使用原冻结快照。Rust 仍按原冻结 manifest 严格校验，不从文件名或路径猜测 ID，不将未知来源变成可自动修正的授权错误。
