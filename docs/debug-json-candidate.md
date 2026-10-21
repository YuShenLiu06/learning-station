# Debug JSON 候选

从 e71bcb00dde4865a289cc333cbc1a5d9ae60b1db 独立实现；Debug 布局先独立提交，随后在本地验收分支正常合并 164cfe8 上下文架构重构。

## 使用与导出

设置 → 请求调试（唯一开关，默认关闭）→ 返回已有会话发送问题 → 设置从左侧列表选择实际请求 → 阅读上下文或切 JSON / 来源 / 原始 / 差异 → 顶部导出解码 JSON。

- CodeMirror 只读视图提供颜色、缩进、行号、折叠、查找、复制完整 JSON；原始请求另有入口，保留线上字符串转义。
- Rust 解码接口与导出复用同一生成函数，只读取已脱敏采集记录；不改变 Request 或增加模型请求。
- 文件写到当前 state/exports/context-bridge-<序号>-<UUID>.json，权限 0600。界面显示实际保存路径；导出文件由用户自行管理，关闭调试不删除。
- registrationSnapshot 是内部冻结输入，projectionSnapshot 是 Pi 可见注册值；finalProjection 是实际已绑定桥片段及本轮已定位问题，按消息编号、内容片段编号和 UTF-16 偏移排序，不按类别或键字母重排。
- 内容保留嵌套对象/数组，导出不受视图折叠、搜索、屏幕虚拟渲染影响。只解桥传输容器和 context/nativeReferences 中对象或数组 JSON；材料、代码和问题不递归解码，字面反斜线 n 保留。
- 导出含来源 key、已有 description、本轮身份、实际内容、最终位置。description 有则原样保留、没有不补造；逐键实发位置没有证据时明确未知。后续拼装标为未注册。

布局按已批准的静态 HTML 原型接入：紧凑请求列表、宽正文、分段视图和独立注册快照。来源片段逐条保留，同一 key 可对应多条最终位置。\n\n## 当前边界

- 注册来自实际提供器，展示其真实 key / description / position，不硬套旧五键。同一 focus 可对应位置、全文、差量多个片段；保留各片段自身回合标识。
- 保持 40 条 / 4 MiB 易失记录、256 KiB 请求体上限；注册快照保留完整值，单记录合计上限 3 MiB，超过时明确省略快照。缺失会令 complete=false，不从最终请求反推补造。
- 未归属片段只列位置，不混入桥正文；attributionComplete=false、complete=false。最终内容范围是可定位的桥片段，不是整个 Pi 历史、系统提示或工具定义。
- 请求体不完整、无最终来源定位或无桥片段时拒绝导出并说明原因；不会把部分前缀冒充完整文件。
- 不读取真实 Key，不新增全局开关、自动导出、日志或上传通道；不更改 AGENTS 或契约。

## 复用选择

先核对现有 request_debug_get / 发送边界采集 / 来源追踪；沿用脱敏和内存上限。项目 Editor 已用 CodeMirror 6，因此采用其只读宿主，未引入第二个编辑器或自制树。

- [官方只读 API](https://codemirror.net/examples/readonly/)、[JSON 语言包](https://github.com/codemirror/lang-json)、[搜索包](https://github.com/codemirror/search)：MIT。新增 lang-json 6.0.2、search 6.5.11，以及传递依赖 @lezer/json 1.0.3；其余复用原有依赖（@lezer/highlight 仅补直接依赖声明）；查看器按需加载，许可随包保留。
- 对比 [@uiw/react-json-view](https://github.com/uiwjs/react-json-view)：MIT，可树形查看，但将引入另一套组件及 Babel runtime peer；本轮已有 CodeMirror，更适合接近 VSCode 的 JSON 阅读。
- 旧 Debug handoff patch 仅只读比对；没有引入其架构注册接口或放宽采集预算。

## 后续启动方案（尚未执行）

候选包应复用 /private/tmp/learning-station-full-qa-m2-debug-e71bcb0/library 和同级 state 的现有 QA 隔离标记，保持已有 API 配置。由主线程协调关闭旧进程后启动独立命名的候选包；不覆盖运行 App，不复制/新建空 state，不提取或重放凭据。

固定持久路径及具体凭据来源迁移仍待用户授权。本轮不迁移，不更改设置；真实服务与本人验收尚未完成。合成 fixture、浏览器 mock、编译和原生候选的证据分别记录，不互相替代。
