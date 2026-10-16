# Pi 原生会话 v1

仅应用内部私有管道；不新增前端 IPC、模型权限或任意路径接口。锁定 `@earendil-works/pi-coding-agent`、agent-core、pi-ai 均为1.3.0。

## 持有关系

- Pi：原生消息、工具调用与结果配对、分支和压缩、`SessionManager.open` 指定文件恢复。
- Rust：Markdown/学习对象、授权与版本快照、Keychain、模型配置、准确映射、运行与业务回执、落盘同步。
- React：沿用原 IPC 与来源快照；不读写 Pi 文件或密钥。

`RequestContext.nativeSession` 内部字段：`directory`（Rust已核实的私有目录）、`learningSessionId`、`runId`、`userEntryId`、`authorizedSourceIds`、`protectHistory`、`legacyRecords`。前端不能提供这些路径或扩大范围。

每个学习会话使用 `state/pi-native/sha256(sessionId)/binding.json`，绑定固定 `transcript.jsonl`。绑定不存在但目录非空、已初始化文件丢失、格式损坏或符号链接均失败。Node 独占锁只恢复已确认死亡的旧进程；不能抢活跃会话。

首次迁移先不可覆盖地保存 `legacy-backup.md/json`；原 Markdown 保持原位。旧业务记录只作为明确标注的不可信引用，不冒充真实旧 assistant/toolResult。迁移失败保留原文件与备份，旧验收包仍可打开原业务库；不得删除未知文件来“修复”。

## 私有事件

| 事件 | 处理 |
|---|---|
| `ready.nativeSession` | `engine=pi-coding-agent`、`version=1.3.0`、`persistent=true`及准确学习会话ID；Rust校验 |
| `session_sync {id}` | Rust同步当前已绑定目录中的普通文件和目录；同id `tool_result.result.synced=true`后继续；无路径参数 |
| `context_manifest {manifest}` | 实际原生材料投影的清单；写入当前Run，随后该assistant自身快照 |
| `session_lifecycle {event,reason,aborted}` | 压缩开始/结束的安全状态，无消息正文或凭据 |
| `tool_recover {id,name,args,recoveryRunId}` | 核对同学习会话原run、原操作ID和已提交对象，只读，不执行原工具；无证据返回unknown |

普通 `tool_request/tool_result/delta/done` 继续原协议。工具结果未回到 Pi 不代表业务未执行。Rust 在发送回包前持久化 `tool-sha256([runId,callId]).json`；恢复优先用原回包，必要时查询真实业务对象。新业务操作 ID 取 `sha256([saved-question-v1,sessionId,userEntryId,toolName,canonicalArgs])`，同一问题换 run/call 重试仍返回原对象；旧无 userEntryId 的记录沿原 run/call ID。原生已接受用户问题按 `userEntryId` 登记实际 native entry ID，失败重试不重复用户原话。

## 模型及资源

固定显式 ModelRuntime、内存凭据、`modelsPath=null`、禁止模型目录网络刷新。资源加载器返回空外部扩展/模板/全局指导/Agents文件，应用仅传既有学习指导。基础工具表为空，精确开放当前学习模式允许的工具；`read/bash/edit/write/grep/find/ls/powershell`均不注册。Node权限模式不允许创建子进程。

摘要与普通生成走同一受控端点和预算，不跨域、不自动换模型。容量未知保持0，不开启阈值压缩；用户填写容量后使用Pi原生阈值生命周期。正常输出用尽不盲目重发。私有状态目录路径不发给模型，凭据不进原生会话。

## 限制与恢复

原生文件有32MiB读取保护，不静默截断或删除；超过限制需保留原数据后明确处理。业务回执无法确认时会话停在unknown，不能改编号绕过。失败恢复的回滚路径是保留此私有目录、继续使用未改动的原Markdown与旧包；本轮不提供破坏性自动清库按钮。
