# UI / Rust IPC 契约 v1（2026-10-16）

Rust命令均为异步IPC；失败返回 `{code,message,retryable,details?}`。参数名 camelCase。启动不扫库、不连网；编辑本地即时，不逐键IPC。

## 数据

`Document = { id:string, path:string(库内相对路径), metadata:{id,title,kind,goals:string[],tags:string[],links:string[],version:number}, body:string, content:string(完整Markdown), version:string(SHA256), modifiedAt:string }`

Markdown 开头是 ```json 围栏（JSON对象），后面正文。metadata.id稳定，links为稳定ID，不是文件路径。kind: section|knowledge|session；tags由AI建议可编辑。

`LibraryState = {root:string, documents:DocumentSummary[], activeDocumentId?:string, activeSessionId?:string, model:ModelSettings, indexing:boolean}`；摘要不含正文。

`Session = {id,sectionId,title,path,version,createdAt,updatedAt,entries:Entry[],contextManifest:ManifestItem[]}`。
`Entry = {id,kind:user|assistant|exercise|attempt|feedback|error|note,content:string,createdAt,selection?:string,relatedIds?:string[],data?:object}`。

`Context = {text:string,manifest:[{id,path,version,includedChars}],warnings:string[],truncated:boolean}`。库内容始终作为用户资料，不作为system。

## 命令（invoke）

- `get_app_state()` → LibraryState（立即返回缓存/初始状态）
- `refresh_library()` → DocumentSummary[]（异步扫描；不在启动阻塞）
- `create_section({title,body,goals?,tags?})` → Document
- `import_markdown({content,filename?})` → Document；复制内容进学习库，不读取任意外部路径
- `open_document({id})` → Document
- `save_document({id,content,expectedVersion})` → Document；冲突code=CONFLICT，保留本地输入，先刷新差异
- `start_session({sectionId,newSession?:boolean})` → `{session,context}`；同小节未关闭会话直接恢复，关联ID自动加载
- `get_session({id})` → Session
- `append_entry({sessionId,entry,expectedVersion,requestId})` → Session；requestId去重
- `load_context({sectionId,selection?})` → Context
- `start_generation({sessionId,mode:explain|question|practice|feedback,selection,prompt,requestId})` → `{runId}`（立即返回）
- `cancel_generation({runId})` → `{status}`；停止未完成生成，不删除记录/不撤回外部发送
- `get_run({runId})` → `{id,status:queued|running|succeeded|failed|cancelled|interrupted,content,error?,sessionId}`
- `configure_model({provider:"glm-pi",baseUrl,model,apiKey})` → ModelSettings；密钥仅本机Keychain，不返回前端/日志
- `get_model_settings()` → ModelSettings
- `test_model()` → `{ok,message}`；当前 Coding Plan 用途门槛返回 PLAN_USAGE_UNCONFIRMED，不发真实请求。ModelSettings见[Pi契约](pi-decision.md)。
- `create_todo({title,dueAt?:string|null,sessionId?,requestId})` → Todo
- `list_todos()` → Todo[]；`set_todo_done({id,done})` → Todo
- `list_notifications()` → Notification[]；`mark_notification_read({id})` → Notification
- `export_anki_csv({sessionId})` → `{path,count,synced:false}`（仅导出）

## 事件（listen）

- `generation` → `{runId,sessionId,status,delta?,content?,error?}`；渲染合并每帧，不在每字流入时重建编辑器。成功产物由Rust持久化到会话。
- `library_changed` → `{documents}`；刷新后发
- `notification` → Notification；站内提醒，飞书未配置必须显示未连接

空库由UI提供“新建小节”和“使用示例”（示例标注）；Coding Plan 用途未确认时禁用AI执行与密钥配置。聊天未发送草稿由UI保存；输入失败不清空。新建学习记录不要污染课程正文。

实现过程中如字段变动在本文件更新并通知对方；前端mock必须最终切换真实IPC。

## 补充：练习与恢复（Core已定）

Session对应 `sessions/<id>/session.md` 与该目录 `entries/*.md`，每条正文仅存一次；顶部JSON只放entryKind/selection/relatedIds/data等元数据，课程正文不被学习聊天改写。

practice成功产物为Entry(kind=exercise)，data=`{exercises:[{id,question,format:"exact"|"self",answers:string[],explanation,tags:string[],sourceIds:string[]}],summary:string}`；内容正文是可读题目。AI输出解析失败显示失败并保留partial，不当成功练习。

UI提交答案调用append_entry，kind=attempt，content为答案，data=`{exerciseId,exerciseEntryId,answer}`；Rust对exact规范化比较，追加feedback与error记录（错因待本人确认），self型标记待反馈，可再调用start_generation(mode=feedback,prompt描述题目与作答)。Rust返回的Session包含新增反馈。错因修正追加kind=error的更正记录，relatedIds关联旧错；保留历史。

通知Todo={id,title,dueAt,sessionId?:string,done,createdAt}；Notification={id,todoId,title,dueAt,read,deliveryStatus}。飞书未启用时站内提醒可用，绝不实际发送；未来只发已核验本人。

Tauri devUrl约定 `http://127.0.0.1:1420`，release嵌入 `dist/`。命令snake_case，invoke参数camelCase。Rust core已拆 content/context/sessions/model/reminders/app/desktop；前端无需统一dispatcher。事件 generation.delta 为新增片段，content为当前完整快照；run.status终态可用get_run核对。无API显示未连接，mock只限浏览器开发并明确标注。

## 复习召回补充（已确认）

- `list_review_items({query?:string})` → `[{id,front,back,sectionId,source,sessionId,tags,createdAt}]`。空库返回[]，由Rust检索历史entry，前端不扫库。source为"错题"或"手动复习"。
- 手工留作复习：`append_entry kind=note,data={front,back,tags:[],reviewCard:true}`，不是错因。
- `export_anki_csv`同时接受error及上述note，返回synced:false。
- `get_app_state`额外提供lastRunId?:string，用于get_run恢复当前学习最近生成（重启后的running返回interrupted）。

纯待办已接受 dueAt=null 或空串；不设时间不会生成提醒。有效非空时间为RFC3339。

## 提醒显式开关（用户最终决定，覆盖前述空串兼容说明）

- `create_todo({title,dueAt?:string|null,reminderEnabled?:boolean,sessionId?,requestId})` → Todo。
- Todo.dueAt为string|null，新增reminderEnabled:boolean默认false；空串只兼容输入，核心规范None；不是是否提醒的隐式开关。
- 无时间允许纯待办；有时间但未启用不提醒；启用却无有效时间报INVALID_INPUT。仅启用+到期+未完成才入提醒箱。
- `set_todo_reminder({id,reminderEnabled:boolean})` → Todo；关闭未来排队，不能撤回已发送消息。

## 原生草稿与关闭窗口

- `get_ui_draft({key})` → UiDraft|null。
- `save_ui_draft({key,data:{body?:string,question?:string,importTitle?:string,importBody?:string},expectedVersion?:string})` → UiDraft={key,data,version,updatedAt}。
- key建议section:<稳定ID>或workspace，限字母数字:_-；每次保存完整该key快照，省略字段视为空字符串；密钥字段拒绝，禁止传入apiKey/baseUrl/model设置。
- 课程正文成功保存后草稿可保留相同内容，重开时仅当body不同于课程正文提示恢复；question/import按key恢复。异步保存按操作序列避免过期回调覆盖。冲突保留当前输入。
- capability已加 core:window:allow-close。

## 性能实测入口（仅记录耗时，不含学习内容/密钥）

`record_ui_metric({name,valueMs})` → `{recorded:boolean}`，name白名单：shell-interactive/section-ready/editor-ready/page-switch/ime-commit/stream-input。Rust在私有运行目录performance.jsonl加上mainToMetricMs（从Rust main进入Tauri到上报），用于release实测，不是冷启动完整OS耗时保证。

请前端在首次可操作布局后requestAnimationFrame上报shell-interactive=performance.now()；CodeMirror构建并requestAnimationFrame可输入时editor-ready记该次加载耗时；切页可见后page-switch记动作到下一帧。只每次动作一条，禁止逐键IPC。可用实际中文IME提交测ime-commit；无真实模型时stream-input保持未测，不用fake证明在线性能。

草稿保存加强：expectedVersion省略仅允许新建；已有草稿必须先get_ui_draft取得版本，否则CONFLICT，保护读取失败后的旧草稿。
