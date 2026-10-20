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
- `configure_model({provider:"custom"|"glm-pi",protocol?:"openai-completions",baseUrl,model,apiKey})` → ModelSettings；密钥仅本机Keychain，不返回前端/日志
- `get_model_settings()` → ModelSettings
- `test_model()` → `{ok,message}`；按已保存的 provider/协议/地址/模型测试。Coding Plan 专用地址（含 provider=custom）在读取凭据前返回 `PLAN_USAGE_UNCONFIRMED`，details 为 `{category:"local_policy",reason:"coding_plan_endpoint",stage:"before_credentials",requestSent:false}`；属于本地用途限制，不能显示为服务器连接失败。普通 custom 地址重新加载后按实际地址重算 gate，不继承旧限制。界面已知 gate 时仅本地解释，编辑和保存仍可用。ModelSettings见[Pi契约](pi-decision.md)。
- `create_todo({title,dueAt?:string|null,sessionId?,requestId})` → Todo
- `list_todos()` → Todo[]；`set_todo_done({id,done})` → Todo
- `list_notifications()` → Notification[]；`mark_notification_read({id})` → Notification
- `export_anki_csv({sessionId})` → `{path,count,synced:false}`（仅导出）

## 事件（listen）

- `generation` → `{runId,sessionId,status,delta?,content?,error?,tool?}`；渲染合并每帧，不在每字流入时重建编辑器。成功产物由Rust持久化到会话。
- `library_changed` → `{documents}`；刷新后发
- `notification` → Notification；站内提醒，飞书未配置必须显示未连接

空库由UI提供“新建小节”和“使用示例”（示例标注）；Coding Plan 用途未确认时保留该专用接入限制；自定义协议/地址配置不被此门槛禁用。聊天未发送草稿由UI保存；输入失败不清空。新建学习记录不要污染课程正文。

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

## 本轮增量契约 v2（2026-10-16）

- `ModelSettings.protocol="openai-completions"`；此处为上一轮协议约定；当前两协议以文末v3为准。provider支持custom、glm-pi。新默认custom，baseUrl/model为空，不推测服务或计费。custom模型ID手输；请求地址为baseUrl后接`/chat/completions`。保存不发请求，测试仅使用已保存配置；修改目标地址/协议/provider须新密钥，改模型可保留同通道密钥。
- URL只允许HTTPS或本机HTTP，拒绝账号密码、查询参数、片段、空白；请求不重定向、不跨域转发认证、不回退其他模型。专用CodingPlan地址即使标为custom也保留用途门槛。密钥只存本应用目标origin+协议+运行目录命名空间的Keychain项。
- `get_ui_preferences()`与`save_ui_preferences(...)`：独立本机偏好文件，不接收密钥字段；合并字段与范围见后表及 full-loop-v1。
- `start_exploration({sectionId?:string,associateCurrentSection:boolean,fresh?:boolean})` → `{session,context,lastRunId?:string}`。true复用指定小节会话；false复用单个独立探索会话，`sectionId=""`、links=[]、associateCurrentSection=false、contextManifest=[]。不读取当前小节、选句或其历史，也不改当前阅读位置。
- `load_session_context({sessionId,selection?:string})` → Context。与生成共用真实上下文计算；独立会话收到非空selection会报错，不静默带入。生成时把实际manifest写回session；UI不能把首次启动的旧清单当作当前来源。
- `exploration:<sessionId>`草稿key准许，字段仍仅body/question/importTitle/importBody；与其他草稿同样要求先读成功、按version保存。独立提问应放question，不含key。
- Pi新增`create_todo({title,dueAt?:RFC3339,relativeMinutes?,reminderEnabled?})`。实际session归属由Rust固定，不接收sessionId；reminderEnabled可选boolean，默认false，仅当本轮用户明确要求提醒时可传true，启用时必须提供提醒时间且晚于本轮referenceTime（缺失、非法或过去时间均拒绝），按固定run时钟核验。成功返回真实persisted/todoId/sessionId/sectionId/todo（todo.reminderEnabled反映实际状态），同run+call幂等，变参拒绝。question/explain可用，practice/feedback不可用。UI在生成终态从真实list_todos/list_notifications刷新，不解析模型“已保存”。
- Markdown顶部JSON新增createdAt/updatedAt和timestampSource。新建用真实时间；已有值保留；旧无创建时间为null/unknown，旧修改时间可用明确file-mtime来源；读/无变化保存不写盘，真实正文或用户元数据变化才更新updatedAt。

## 本轮增量契约 v3（以此节为准）

| 接口 | 当前行为 |
| --- | --- |
| `configure_model` / `get_model_settings` | custom支持`openai-completions`、`anthropic-messages`；glm-pi仅OpenAI。ModelSettings增加`requestUrl`，按保存配置派生，不表示服务器已验证。OpenAI为base+`/chat/completions`，不会盲加`/v1`；Anthropic为base+`/v1/messages?beta=true`（本版Pi SDK固定query），基础地址不含末尾`/v1`或`/messages`。仅API key、无OAuth或身份伪装；模型与地址不自动替换。 |
| `set_model({model})` | 只改当前已保存binding模型ID，0网络、0Keychain读写。地址/协议/provider不变；改名取消verified，未改名no-op。生成queued/running时BUSY；无配置NOT_CONFIGURED；无效模型INVALID_MODEL_SETTINGS。新run捕获独立配置快照。 |
| `start_generation({...userEntryId?})` | 可复用同会话已保存、kind=user且正文/selection一致的entry，不再次append。Run额外`userEntryId`、`toolCalls`（旧文件默认空）。同requestId+同内容返回原run，变内容CONFLICT；未知结果先get_run，同ID不自动另发。确认终态失败后新重试用新operationId、同userEntryId。 |
| `append_entry` | 发送原问题data可含`{clientMessageId:sendId,mode}`；同requestId幂等，expectedVersion防覆盖。未配置/gate前置失败不假送达；本地append成功与模型失败分开显示，失败不覆盖后来输入的新草稿。 |
| `get_ui_preferences` | `{fontSize,assistantWidth,sidebarWidth,sidebarCollapsed,expandedNodeIds,toolAuthorizationTier}`。字号12–24默认16；右宽280–960默认346；左宽180–480默认240；折叠默认false、展开ID默认[]；工具授权档位off/read/write/bash默认off，旧文件缺字段补默认off。 |
| `save_ui_preferences({fontSize?,assistantWidth?,sidebarWidth?,sidebarCollapsed?,expandedNodeIds?,toolAuthorizationTier?})` | 同一锁内原子合并，缺省字段保持，全部缺省只读；旧文件缺字段时补默认。非法值不写盘。toolAuthorizationTier仅接受四档枚举，变更档位时如有queued/running的Run返回BUSY（先核对未决工具回执），同值重存不拦截。小窗口只夹紧实际渲染宽度，折叠不覆盖宽度偏好。 |
| `list_tool_receipts({sessionId})` | 只读列出该会话工作区已落账的 write/bash 回执摘要（requestId、tool、status、createdAt、write 的相对路径），不含命令原文、参数或文件内容；会话不存在或编号无效拒绝，无台账返回空列表。按 createdAt 降序最多返回 200 条，超出时 `truncated:true`（另有损坏/符号链接/超限台账文件跳过不展示）。 |
| `check_feishu_status()` | `{ready,reason}`，只读已验证本人通道，不登录、不发送、不返回凭据或收件人参数。 |
| `test_feishu_notification({requestId})` | `{status:sent|blocked|failed|unknown,messageId:string|null,reason}`。ID为8–128字母数字/下划线/连字符；正文固定测试通知，用户点击才发。发前私有原子Unknown占位；同ID重开/超时均不重发；仅真实成功+messageId才sent。不会启用全部提醒。 |

ToolActivity=`{id,toolCallId,runId,sessionId,name,status:running|succeeded|failed,title,summary,startedAt,updatedAt,persisted,object?,errorCode?}`。id/toolCallId由Core对run+模型opaque ID哈希，不返回rawargs或内部路径。object=`{kind:todo|learning-entry|review-card,id,sessionId,sectionId}`；仅真实保存回执提供。查询成功persisted=false，不能显示“已保存”。

工具开始/回执后更新`get_run.toolCalls`并发`generation.tool`事件。历史复用Session.entries：kind=note，data=`{origin:"pi-tool-activity",toolActivity}`；按toolCallId取最新快照，不当普通学习正文、不回灌模型历史。历史只有running而无终态显示结果未确认，不自动重跑；打开仅按稳定ID调用已有待办/学习记录/复习接口。

响应诊断只含固定枚举、状态码、字符/token计数和完成标志，不含正文、reasoning原文、headers或凭据。最近一次在私有状态目录`model-response-diagnostic.json`，生成失败也保留到Run.error.details.response。MODEL_ENDPOINT_PAGE说明返回网页且提示核对/v1；MODEL_FORMAT、MODEL_LENGTH、MODEL_INCOMPLETE、MODEL_THINKING_ONLY、MODEL_EMPTY区分格式、截断和无正文；均不能冒充完整答案。原Pi流解析未替换，预算不自动提高，真实测试由用户在本机触发。

本轮明确取消旧全展开，改边缘拖动调宽。探索左移/作业结构的半截需求、课程资源目录和课件功能只记录后续，不做迁移。

### 后续学习 Skill 的边界（尚未实现）

保留现有 `app::session_context`、`generation::messages` 与 `task_tool_policy(mode)` 作为流程级组合入口；未来可按 mode 声明 required Skill/context，并将实际加载版本记入上下文清单。必需加载失败必须在模型请求前报错；可选学习资料继续沿用现有可见 warning。当前无 Skill loader/安装/自动扫描能力，不加载用户 coding skills。

生成状态恢复仅把真正不存在的记录返回 `NOT_FOUND`；损坏、不可读或不安全文件分别报原错误并保留，不应视作首次发送。

### M2 课程会话兼容字段

- `Session.courseId?: string`：共享会话固定课程归属；`legacySessionIds?: string[]`：已备份并保留的旧对话索引。
- `start_session(sectionId, newSession?)`：课程内文档返回同一 session；显式新建不绕过课程固定路由。
- `load_session_context`、`start_generation` 新增可选 `documentId`，由本轮 activeDocumentId 冻结；未传兼容旧 sectionId 锚点。保存问题的 requestedContext.documentId 与生成重试必须一致。
- 练习 originSessionId 是原作答/批改账本；与当前聊天 session 可以不同，课程和原始来源均由 Rust 校验。

### Focus 冻结

`freeze_focus {request:{sessionId,input:{documentId,expectedBody,expectedVersion?,view,ranges,questionId?,assetId?},selection,injections,assistantReferences,exerciseReferences}}` 返回 [FocusSnapshot](focus-v1.md)。这是一次本机私有材料冻结，不发模型、不改变权限。`start_generation` 增可选 `focus`；既有调用兼容，带 Focus 的请求严格绑定原冻结记录。
