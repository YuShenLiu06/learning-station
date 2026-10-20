# 本轮接口（2026-10-16，前端/Rust共同锁定）

前端独占 src 与前端 tests；主核独占 Rust、Pi、contracts、构建与 Git；飞书独立任务仅提供 tools/feishu-bot 补丁。禁止 computer-use；本轮自动层不能冒充原生/在线验收。

## 练习与主观批改

保留当前练习按钮与位置。start_generation(mode=practice,prompt=用户自然语言要求) 不限定英语；空要求使用“根据当前材料和学习疑点生成针对练习”。按钮和 Pi generate_exercises 共用 exercises::validate/create：Pi根据真实材料生成结构化题目后调用工具保存，不嵌套第二次模型请求；回执 persisted=true、documentId/exerciseEntryId 才算保存。新工具对象 kind=exercise，id 为真实练习子文档ID。

题目保留 format=exact|self，self表示主观题、必须AI批改，不代表让用户自评。可选 rubric={criteria:[{id,label,description,maxPoints}]}；1–8项，id唯一，maxPoints为1–100整数。量表随题目持久化，提交后不可改变原量表；旧主观题没有量表时只能有结构化评语、score=null，不能伪精确。每题答案/完整参考默认隐藏，提交后参考仍默认折叠。

- get_grading_settings() → {model:null|string,effectiveModel:string,configured:boolean,usesCurrentProvider:true}。
- set_grading_model({model?:string}) → 同上；空/null沿用聊天模型，只改本机批改模型ID，不改供应商/地址/凭据，不发网络。
- grade_attempt({sessionId,attemptId,requestId}) → GradeRun；仅已保存的self题attempt；同编号同提交不重发，异参CONFLICT。
- get_grade_run({runId}) → GradeRun；cancel_grade_run({runId}) → GradeRun。
- GradeRun={id,sessionId,attemptId,status:"queued"|"running"|"succeeded"|"failed"|"cancelled"|"interrupted",result:null|GradeResult,error:null|{code,message},model,updatedAt}。稳定id=grade_+SHA256(requestId)前32。未知响应先按该IDget，不盲换编号；刷新最新Session读取真实feedback。
- GradeResult={attemptId,exerciseId,overallFeedback,score:null|{earned,max},criteria:[{criterionId,earned,feedback,evidence:string[]}],annotations:[{quote,occurrence:number,issue,reason,suggestion,criterionId?:string,anchor:null|{start,end}}],missingPoints:[{criterionId?:string,description,suggestion}],weakPoints:string[],rubricVersion,answerVersion}。
- anchor是原答案Unicode码点半开范围（前端 Array.from），只由Core按quote第occurrence次精确匹配派生；找不到时anchor=null，只显示未定位评语，不画红线。遗漏要点不得伪造引用。criteria覆盖固定量表全部项且不重复，总分由程序加总；无量表criteria=[]/score=null。答案仍原样，AI输出不能改答案/量表。评分无工具、一次有界请求，失败不冒充通过/不复用旧attempt反馈。feedback.data.aiGrade=GradeResult，method=ai；pending仅提示待AI批改，不能提示自评。

## 显式上下文注入

- ContextInjection={documentId,expectedVersion,selection?:string}；每次最多8份，selection最多8000字符且须精确存在于该版本正文；只能section/knowledge/兼容旧exploration，禁止目录/练习参考/系统记录/路径。
- load_session_context({sessionId,selection?,injections?:ContextInjection[]}) 与 start_generation(...,injections?:ContextInjection[]) 接受相同参数。未传兼容[]。右键显式加入/移除，列表显示来源；不自动持久化为全局上下文。
- Core执行前重读校验版本，冻结到该Run已有contextManifest；重复documentId去重，选段与全文不重复加载，所有材料仍共用60000字符/32份预算，超限可见warning，版本变化CONTEXT_CONFLICT不发送。
- 独立会话不继承active文档/选区；只有用户明确注入才可有来源。预览不是执行快照；重试同requestId包含同injections，变更参数CONFLICT。

## 复习来源

list_review_items({query?,courseId?}) 返回原卡结构并增加 courseId:null|string,courseTitle:null|string,sectionTitle:string,sourceDocumentId:null|string。sourceDocumentId/sectionId是稳定来源；无小节旧独立会话sourceDocumentId=null，打开真实session。来源已回收不猜替代课程。课程可按courseId查关联卡。source标题只展示，跳转必须用ID。五维算法仅保留未来扩展边界，本轮不定义维度/排期。
课程会话的卡片按真实持久化 Session.courseId 归属与筛选，空材料来源保持空；旧无 courseId 会话仍由原材料解析课程，卡片 payload 中的 courseId 不能改变归属。此查询不迁移或重写旧卡片。

## Anki（真实API，CSV仅降级）

已确认 /Applications/Anki.app 26.5 +2055492159为AnkiConnect；当前127.0.0.1:8765拒绝连接。固定本机地址，不安装/不自动打开Anki、不调用requestPermission、不读取插件配置/用户牌库DB/现有API key。

- check_anki_connection() → {connected,code,message,version?:number,decks:[{id:number,name:string}]}。显式重试；offline/auth错误清楚展示，不称同步。需要用户自行打开已有Anki；如插件要求API key，本轮明确ANKI_AUTH_REQUIRED，不绕权限。
- get_anki_due({deckName?:string}) → {cards:AnkiCard[],total:number,mode:"anki"}；最多25张，固定is:due，deckName只能来自真实deckNamesAndIds。AnkiCard={cardId,deckName,question,reviewToken,nextReviews:string[],source:null|ReviewSource}，不给背面。
- reveal_anki_card({cardId,reviewToken}) → {cardId,answer}，实际同一张卡才显示。
- answer_anki_card({cardId,reviewToken,ease:1|2|3|4,requestId}) → AnkiOperation；须先显示答案，复核卡片未变化；真实answerCards成功才succeeded，写请求发送前持久预占，超时/断线unknown不自动重发。
- get_anki_operation({requestId}) → AnkiOperation={requestId,status:"succeeded"|"failed"|"unknown",cardId?:number,noteIds?:number[],error?:{code,message}}；相同ID不会重复写。
- add_review_items_to_anki({ids:string[],deckName:string,requestId}) → AnkiOperation；1–100条真实本机卡，用户显式发送才创建LearningStationBasicV1模型/所选牌组（牌组须已有），字段Front/Back/Source，稳定标签去重；记录实际noteId和源课程/小节/会话/卡ID，返回成功才能说写入。未知同ID仅核对，不重写。Anki原有卡 source=null 明确“Anki原有卡，未关联学习材料”，不伪造课程。
- 复习主入口真实Anki连接+到期卡+显示答案+四档实际复习；所有牌组/本机卡管理是次级入口。本机来源卡可以显式送入Anki，CSV在未连接时为降级导出。外部HTML须净化，禁脚本/远程媒体；不执行Anki模板JS/CSS。

ReviewSource={reviewItemId,sessionId,sectionId:null|string,sectionTitle:string,courseId:null|string,courseTitle:null|string}；AnkiCard.source仅对本应用真实写入并记录映射的卡提供该对象。source不可用显示来源缺失并保留稳定ID，不猜跳转。

## 飞书安全配置与显式生命周期（独立Node适配器集成）

无CLI回退，复用已存在Node24运行时。仅独立自建应用、可用范围本人、唯一目标聊天；长连接只收事件，发消息走HTTP。默认接收/发送均关闭；保存配置0网络，不自动连接/授权。

- get_feishu_bot_settings() → {configured,appId,ownerOpenId,chatId,receiveEnabled,deliveryEnabled,secretStored,runtimeAvailable,runtimeReason}。不含secret。
- configure_feishu_bot({appId,ownerOpenId,chatId,receiveEnabled,deliveryEnabled,appSecret?:string}) → 同上。首次/变更绑定须新secret，现binding空secret可保留；仅用户本机输入，保存独立Keychain，提交即清空。进程尚在运行时BUSY，先停再改。
- start_feishu_bot() / stop_feishu_bot() / get_feishu_bot_snapshot() → {state,reason,counts,processExited,outputClosed}，state=unconfigured|stopped|starting|blocked|connecting|connected|disconnected|reconnecting|failed|receive_disabled|stopping；只有connected且存活是接收已联通。runtime缺失为blocked，不伪造按钮成功。
- check_feishu_status/test_feishu_notification 保持原IdentityStatus/DeliveryOutcome与同requestId去重；test仅用户显式发送固定文字，成功须messageId。收取和发送分别启用，接收内容仅被动持久通知，不执行资料指令/自动回复/自动模型请求。
- 原设置页通知分类下加已有飞书配置/启停/安全状态即可，不加新的主导航。标明应用退出/休眠不能保证及时提醒。配置精确步骤见contracts/feishu-bot-node-v1.md（适配器随后合并）。

飞书deliveryEnabled的用户可见含义为“发送新到期提醒及手动测试”。tick仍仅入站内库，独立协调器只有机器人已显式启动、发送已启用时才处理启用后新建的到期通知；先持久unknown预占，真实messageId才sent。旧历史不补发，unknown/failed不自动重发；用户关闭提醒/完成todo后不发尚未发送项。应用退出不保证发送，不另建后台服务。草稿feishu-test可携带原测试requestId（同通用草稿先读/版本校验）。

新生成题由Core标记answerMatch=trim，仅去首尾空白后比较，保留大小写/符号/小数点；避免将m/M或.5/5误当相同。需要接受的有限变体列入answers，复杂等价解法用self。旧题无标记时保留旧核对规则，原数据不迁移。

## 练习字段与错误恢复（Issue #3/#4）

最终练习 JSON 仅含 `exercises` 和 `summary`；`generate_exercises` 工具额外要求非空 `requirements`（最多8000个 Unicode scalar value），不可混为同一协议。1–5 题；summary 最多10000个 Unicode scalar value，题干/解析最多20000，单个答案最多10000，标签最多64；32KiB工具参数和256KiB最终响应字节总限额保持不变。

`explanation` 可省略；存在时必须是字符串。缺省仍以缺省保存，阅读时显示“尚未提供解析”，不从答案或 rubric 推造解析。题干、参考答案、格式、标签和来源数组仍按原规则必需。sourceIds 必须来自本轮冻结材料 manifest 且不重复；评分标准结构与分值校验不放宽。

模型字段错误使用 `INVALID_MODEL_OUTPUT` 和安全字段路径（题目/答案序号从1开始）、原因/长度，不回显字段原值或来源ID。Rust校验诊断经工具错误回包送回Pi和活动卡；原问题、引用及私有原生记录保持原生命周期。对已注册练习工具的结构错误，允许SDK返回错误供模型在原预算内修正；权限、未知工具、超预算、未知写入结果仍停止。已保存问题与精确参数绑定的幂等ID不改变。

练习按钮的最终 JSON 也由 Rust 先只读校验，安全字段错误回传 Agent 最多修正两次；来源越界不可自动修正。此过程不写学习对象、不改变原问题/快照/操作编号，成功后才经原保存路径提交。原始中间输出保留在私有 Pi 会话，最终失败输出仍可从现有恢复入口查看。
