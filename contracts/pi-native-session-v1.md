# Pi 原生会话 v1

原生会话部分仅使用应用内部私有管道，不新增模型权限或任意路径接口。锁定 `@earendil-works/pi-coding-agent`、agent-core、pi-ai 均为1.3.0。

## 持有关系

- Pi：原生消息、工具调用与结果配对、分支和压缩、`SessionManager.open` 指定文件恢复。
- Rust：Markdown/学习对象、授权与版本快照、Keychain、模型配置、准确映射、运行与业务回执、落盘同步。
- React：沿用原 IPC 与来源快照；不读写 Pi 文件或密钥。

`RequestContext.nativeSession` 内部字段：`directory`（Rust已核实的私有目录）、`learningSessionId`、`runId`、`userEntryId`、`authorizedSourceIds`、`protectHistory`、可选 `historyBoundaryKey`、`legacyRecords`。前端不能提供这些路径或扩大范围。

每个学习会话使用 `state/pi-native/sha256(sessionId)/binding.json`，绑定固定 `transcript.jsonl`。绑定不存在但目录非空、已初始化文件丢失、格式损坏或符号链接均失败。Node 独占锁只恢复已确认死亡的旧进程；不能抢活跃会话。

首次迁移先不可覆盖地保存 `legacy-backup.md/json`；原 Markdown 保持原位。旧业务记录只作为明确标注的不可信引用，不冒充真实旧 assistant/toolResult。迁移失败保留原文件与备份，旧验收包仍可打开原业务库；不得删除未知文件来“修复”。

## 私有事件

| 事件 | 处理 |
|---|---|
| `ready.nativeSession` | `engine=pi-coding-agent`、`version=1.3.0`、`persistent=true`及准确学习会话ID；Rust校验 |
| `session_sync {id}` | Rust同步当前已绑定目录中的普通文件和目录；同id `tool_result.result.synced=true`后继续；无路径参数 |
| `context_manifest {manifest}` | 实际原生材料投影的清单；写入当前Run，随后该assistant自身快照 |
| `session_lifecycle {event,reason,aborted}` | 压缩开始/结束的安全状态，无消息正文或凭据 |
| `tool_recover {id,name,args,recoveryRunId}` | 核对同学习会话原run、原操作ID和已提交对象，只读，不执行原工具；无证据返回unknown，有台账但工具或参数不一致返回CONFLICT |

普通 `tool_request/tool_result/delta/done` 继续原协议。工具结果未回到 Pi 不代表业务未执行。Rust 在发送回包前持久化 `tool-sha256([runId,callId]).json`；恢复优先用原回包，必要时查询真实业务对象。新业务操作 ID 取 `sha256([saved-question-v1,sessionId,userEntryId,toolName,canonicalArgs])`，同一问题换 run/call 重试仍返回原对象；旧无 userEntryId 的记录沿原 run/call ID。原生已接受用户问题按 `userEntryId` 登记实际 native entry ID，失败重试不重复用户原话。

## 模型及资源

固定显式 ModelRuntime、内存凭据、`modelsPath=null`、禁止模型目录网络刷新。资源加载器返回空外部扩展/模板/全局指导/Agents文件，应用仅传既有学习指导。基础工具表为空，精确开放当前学习模式允许的工具；`edit/grep/find/ls/powershell`均不注册。`read/write/bash`仅在本机设置的工具授权档位（off/read/write/bash，默认off，仅question/explain模式）开启后受控注册，随toolPolicy.authorization由Rust下发；参数与结果仍由Rust校验，read只读本轮manifest已授权来源或会话工作区文件，write只写会话工作区（路径/配额/符号链接受限），bash只在会话工作区以受限环境执行（固定PATH、无HOME、超时与输出有上限），write/bash以持久回执台账幂等，恢复只读台账不重执行。Node权限模式不允许创建子进程（bash由Rust进程执行，不经Node）。

`list_learning_skills`/`load_learning_skill`是仅question/explain/practice模式注册的只读工具，不挂任何授权档位：它们只读随包 `skills/` 固定允许清单（三个稳定id），复用主动快照同一路径校验（根内canonicalize、拒符号链接、16KiB、frontmatter合法），目录仅返回id/名称/描述不含正文，正文经工具结果通道返回且受工具结果预算约束，超出按SKILL_INVALID单工具拒绝。清单外或含路径成分的 id 分别以 TOOL_SCOPE／INVALID_TOOL_INPUT 拒绝，均不触盘。同run重复load同一指导只返回简短提示（含sha256）；该指导已随模式或工具注册注入时返回"已在指导中"，不重复正文；去重台账按run维度挂Rust，新run或该run压缩完成后可重新取正文。ready事件的learningSkills版本回执语义不变，仍只认主动注入快照，按需加载不进入该回执。

摘要与普通生成走同一受控端点和预算，不跨域、不自动换模型。容量未知保持0，不开启阈值压缩；用户填写容量后使用Pi原生阈值生命周期。正常输出用尽不盲目重发。私有状态目录路径不发给模型，凭据不进原生会话。

## 限制与恢复

原生文件有32MiB读取保护，不静默截断或删除；超过限制需保留原数据后明确处理。业务回执无法确认时会话停在unknown，不能改编号绕过。失败恢复的回滚路径是保留此私有目录、继续使用未改动的原Markdown与旧包；本轮不提供破坏性自动清库按钮。

## 最终练习 JSON 的有界修正

Rust 在 practice 模式显式开启 `outputValidation`，Node 每次完成最终文本后发送只读 `validate_output {id, content}`；Rust 使用原轮冻结 manifest 校验，回复 `tool_result.result={valid,recoverable,message?}`，不保存对象、不增加权限。两端均最多接受三次校验（首次及最多两次修正），沿用原请求超时与工具预算。

可纠正错误通过 `learning-station-output-validation` 原生 custom message 回传 Agent，包含安全字段路径、原因与长度限制，不伪造新用户问题。未知来源或权限错误不可自动修正；失败原文留在私有原生会话，最终失败文本留在Run。仅校验通过才标记原生记录complete；Rust保存前再次校验并沿既有操作编号幂等写入。该私有事件不注册为模型工具或前端IPC。

## 课程会话路由（M2 / #11）

显式 course 根下的笔记与练习经 `sessionByCourse[courseId]` 恢复同一业务 session；Pi 的目录、header ID、精确重开与幂等协议仍由该业务 ID 决定。无课程的旧笔记和独立探索保持原行为。`Session.courseId` 是固定归属，`sectionId` 仅保留创建时锚点，不能代替本轮材料。

`load_session_context` 与 `start_generation` 接受可选 `documentId`；前端提交时从 activeDocumentId 冻结，经保存问题、重试、生成和练习落盘保持不变。省略时兼容旧锚点。Rust 检查课程归属；练习页面读取来源材料，不将含参考答案的页面正文自动注入。当前来源决定工具及练习目标，切页不改已提交请求。

首次承接旧课程时，先将原 preferences、业务会话目录和对应 Pi 目录备份到私有 `course-session-backups/<sha256(courseId)>/`，完整回执后才建立新课程对话。原对话及来源保持原样，`legacySessionIds` 供界面提示和历史查看；不选一段旧对话覆盖其他记录，不拼接 Pi 历史。备份遇到运行锁、符号链接或读写失败时不切换路由。旧练习仍按 originSessionId 作答/批改，聊天使用课程 session；引用校验同时核实原账本与课程。

Focus 切换时，native 的历史授权集合保留仍属于该课程的已见来源，课程外来源仍须本轮明确提供；回收历史保护仍生效。此集合不注入其他笔记正文，不扩大 Rust ToolScope/read 工具的本轮来源列表。

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
