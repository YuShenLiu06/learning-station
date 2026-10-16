# 完整学习闭环 v1（本轮锁定）

目标：课程资料 → 持久探索正文 → 练习子内容 → 作答/解析 → 本人错因 → 复习回执/CSV → 待办。旧文件不搬迁；正文、层级、练习、作答都用稳定ID。新增操作可重试，未知结果先读取同ID。

## 层级与探索

所有学习节点仍是JSON顶部信息+Markdown。kind为course/folder/section/knowledge/exploration/exercise；层级放在metadata.parentId、order，物理路径不因重排改变。course仅根，exercise为叶，最多12层。旧无parent的section/knowledge作为根兼容显示，不自动迁移。父级仅目录归属，不自动成为AI上下文；探索中心正文是可保存/编辑的真实文档，右侧仅助教。

- `get_library_tree()` → `{nodes:[{id,parentId:null|string,courseId,kind,title,version,order,path}],warnings:string[]}`，内部先刷新索引；无效父/循环在清单可见，原文件不改。
- `create_library_node({kind,title,parentId?:string,body?:string,requestId})` → `Document`。UI可建course/folder/section/exploration；稳定id=`node_`+SHA256(requestId)前32；同ID异参CONFLICT。courseId为最顶层祖先ID，旧根小节也可拥有独立资料区。
- `set_document_links({id,links:string[],expectedVersion})` → `Document`，最多16份显式选择的学习资料；目录归属不自动关联，练习/目录/系统记录不可作为自动资料。跨课程必须由用户显式选定，不扫描全课；附件仍严格同课程。解绑保存空links后实际上下文移除该资料。
- `move_library_node({id,parentId?:string,order,expectedVersion})` → `Document`，无环、版本校验与原文历史备份；不改节点/题目ID。
- `import_markdown({content,filename?,parentId?,requestId?})`保留原ID/links，新增可选父级与防重复请求号，同一次Markdown写入完成；旧参数保持兼容。
- `open_document/save_document/start_session`继续使用。exercise打开后用下节get_exercise_document返回origin session，不新建作答session。新exploration session指向自身文档；旧独立会话保留，可从历史访问，不暗带当前正文。

## 练习子文档

AI仍只输出严格JSON：`{exercises:[{question,format:"exact"|"self",answers:string[],explanation,tags:string[],sourceIds:string[]}],summary}`。Core验证数量/长度/来源，生成稳定q ID；Core渲染Markdown的题干、参考答案、解析、来源区域。评分数据以真实origin entry为准，不解析自由编辑正文评分。

- practice生成成功：先幂等保存exercise entry，再建立当前文档的exercise子文档；中途失败可按原requestId恢复，禁止报告假成功。子项metadata包含parentId、originSessionId、exerciseEntryId。
- `get_exercise_document({id})` → `{document:Document,session:Session,exercises:<完整exercises结构>}`。
- `create_manual_exercise({sessionId,sourceDocumentId,input:{question,format,answers,explanation},requestId})` → 同上。
- `materialize_legacy_exercise({sessionId,exerciseEntryId})` → `Document`。仅用户显式操作将旧entry映射为稳定子项，不复制答案状态。
- 作答继续`append_entry` attempt `{exerciseId,exerciseEntryId,answer}`，以题目ID关联；反馈必须匹配最新attemptId；新作答不能套旧反馈。本人错因优先恢复真实error，未提交草稿优先；参考答案与本人错因/AI建议分开。

## 课程资料区

- `import_course_asset({courseId,fileName,bytes:number[],requestId})` → `Asset`。
- `list_course_assets({courseId})` → `Asset[]`。
- `read_course_asset({courseId,assetId})` → `{asset,text?:string,dataUrl?:string,truncated:boolean}`。
- `Asset={id,courseId,fileName,mime,size,sha256,reference:"asset:<id>"}`，只接受合法根courseId；存储于库内assets/<courseId>/<assetId>/，不接受系统路径。
- UTF8文本/代码白名单最多1MiB，PNG/JPEG/GIF/WebP最多5MiB且magic匹配；每课程最多25MiB/100件。禁HTML、SVG、可执行文件、压缩包；代码只作为文本存储/预览，不执行。文本预览最多128KiB，图片仅本机data URL。上传Markdown不进入学习文档索引。
- 引用语法`[名称](asset:<id>)`或`![名称](asset:<id>)`。前端专用引用解析与IPC预览；不放开任意img/a/远程URL。AI上下文仅在正文中明确引用时加载同课程文本，受总预算限制并显示真实清单；图片可本机预览，本轮不声称已送视觉模型。

## 复习修正（已确认缺陷）

- `list_review_items({query})`延续原返回并增加`reviewState:{status:"new"|"again"|"understood",reviewedAt:null|string}`。本人显式回执，无新增排期算法。
- `set_review_state({id,status:"again"|"understood",requestId})` → `{id,status,reviewedAt}`，保存origin session的note回执，重复请求不重复写。note data.origin=`review-state`，不是普通聊天/复习卡。
- `export_review_items({ids:string[]})` → `{path,count}`，1至200个可见真实复习ID，按所选集合精确CSV，不用active小节或last session兜底。空集合禁用；找不到任何指定ID则报错，不静默少导。
- 独立探索留作复习必须使用其实际sessionId/sectionId，不能传当前阅读activeId；空section来源打开旧探索会话。真实参考答案、本人错因和AI待确认来源保持区分。

## 界面偏好与设置

`get_ui_preferences`增加`sidebarCollapsed:boolean=false,expandedNodeIds:string[]=[]`；save命令两字段都可选，缺省保持原值，与fontSize/assistantWidth在同一锁下合并。expandedNodeIds最多4096个合法稳定ID。旧偏好自动默认，不重置字号/宽度。

设置左分类右内容，仅已有供应商/外观/通知；切换保留未提交配置。主左栏可图标rail/完整树切换，选中/层级保留、tooltip+aria、reduced-motion；右侧拖宽保留，不恢复全展开。

## 上下文快照与预算

材料`manifest`保持真实文档/同课程显式文本附件，另有`Context.historyManifest`记录实际历史投影的稳定ID、JSON签名与字符数；Session返回`contextHistoryManifest/contextWarnings/contextTruncated`，旧文件默认空/false。生成前重新读取实际版本。Run也持久保存`contextManifest/historyManifest/contextWarnings/contextTruncated/contextSnapshotVersion=1`，当前/重试运行以其自身快照显示，不能拿Session后来一次生成的来源替代旧运行。旧Run缺版本默认为0，历史来源未记录须明确提示。Session字段用于恢复当前会话；历史assistant仍用自身entry data快照。

历史限定同session、排除本次已保存问题/复习和工具回执；最多最近16项、单项5000字符、总JSON20000字符，练习历史只题干/稳定ID/来源，不带未作答答案解析。当前问题与选区独立，正文60000字符/32份/4层预算；截断明确警告，未实现摘要。

JSON元数据可声明`requiredContextIds`（最多32个稳定ID）；缺失、不可读或预算不能完整载入必需项时`REQUIRED_CONTEXT_UNAVAILABLE`阻断请求。普通links仍可见warning。此为既有上下文入口的受限声明，不是Skill发现/安装平台。

## 统一验收（全部完成后一次打包）

1. 旧库只读兼容/原文历史备份；新建课程→嵌套文件夹→材料→探索，关窗重开层级、正文、选中与草稿保留；移动无环，题目ID不变。
2. 手动题与localhost真实SDK生成分别形成子文档→答错→匹配本次反馈→本人错因→复习→本人回执→筛选集合CSV；重开仍一致。模型结构错误不落假题/不报成功。
3. PNG+UTF8代码上传→同课程引用→安全本机预览；路径/类型/大小/跨课程拒绝，代码不执行；图片不冒充已送模型。
4. 回归协议精确目标、保存0请求、发送unknown核对、重试不重复、后续草稿、工具回执、模型切换、右栏宽度、主栏折叠、设置分类、飞书显式测试。
5. 统一最终源码/产物哈希后隔离库实际native关键流程截图；不占用户窗口。若同bundle绑定有风险，先解决隔离入口再操作，不能以mock升级为native通过。真实在线模型/飞书仅经用户应用内合法操作；未测保持明确，绝不重放凭据。

本轮不加课件、不加学习Skill/安装平台、不部署、不推送，不再交零散中间候选。
