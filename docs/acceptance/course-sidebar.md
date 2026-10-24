# 课程侧栏验收

本次实现把全局学习入口保留在窄导航中，课程侧栏使用原有宽度、折叠与展开偏好；课程选择器可返回“全部课程与文件”，旧根目录材料仍可打开。

- `list_course_files(courseId)` 只枚举已登记且未回收课程的 `materials`，不触发旧课程迁移。非隐藏目录保留原路径；注册业务文件用原文档标题与操作，未出现在材料目录的业务练习等仍显示。旧课程沿用登记目录。
- 隐藏项目、缓存、常见凭据名称、私钥扩展名、符号链接和非普通文件不进入目录；最大 10000 项、24 层，越界明确报错。文件夹展开标识使用课程和路径摘要，复用 Rust 导航偏好，不在浏览器另存。
- `reveal_course_file(courseId,path)` 重新核对课程、回收状态、相对路径及每级符号链接，只调用固定 `/usr/bin/open -R <absolute-path>`；未提供任意路径、shell、模型工具或系统默认打开能力。当前实现为 macOS Finder 定位。
- 未登记材料只显示文件信息和 Finder 定位入口，不预览、不执行 HTML/脚本、不进入业务审阅。刷新按钮与窗口重新聚焦可重读文件目录。

本地验证：`npm run build`；`node --experimental-strip-types --test tests/course-sidebar.test.mjs tests/workspace-flow.test.mjs`；`cargo test --manifest-path src-tauri/Cargo.toml --no-default-features course_files::tests`。分别覆盖编译、模拟 IPC 的目录/定位与既有学习流程，以及真实隔离临时目录的 HTML/TXT、过滤和路径/链接拒绝。

桌面 Rust 编译使用 `TAURI_CONFIG='{"bundle":{"resources":[]}}' cargo check --manifest-path src-tauri/Cargo.toml`；当前工作树缺少打包资源 `runtime/node`，此覆盖仅用于检查源码编译，不代表打包成功。浏览器开发预览检查了实际 React 展开/折叠布局，数据仅在内存。未执行真实 Finder 定位、原生完整流程、跨平台验收、CI 或用户本人验收。

本人最短验收：打开含 Markdown 笔记、练习、子目录 HTML/TXT 的课程；确认材料无重复、练习仍可打开；查找 HTML 并从信息页在 Finder 定位；折叠侧栏后切换探索/待办；展开并切课，返回“全部课程与文件”；调整宽度和分组折叠后重开应用，确认草稿和偏好保留。

## 首轮代码审查修复验证

四项有效发现均修复：新建菜单继承定位导致被裁切；物理文件名匹配后再次按标题过滤；刷新外部新增业务文档未同步节点；宽侧栏未为窄导航预留视口空间。

浏览器隔离验收入口：开发服务下 `/tests/fixtures/sidebar-layout.html`。该页面直接使用实际 `WorkspaceSidebar`、`CourseFiles` 与应用样式，IPC 全为合成数据，不调用原生服务，也不进入生产构建入口。Edge 中实际检查：1110px 宽且保存侧栏 480px 时，面板测量为 351px，助手右侧控件完整；新建菜单可见且点击笔记动作成功；搜索 `note.md` 可打开标题为“课堂笔记”的注册文档；模拟新增文档后刷新可见并打开；800px 展开与折叠均保留输入和全局导航，保存宽度仍为480px。以上为真实浏览器组件交互证据，不替代原生包验收。
