# Windows 模型与图片输入验证

2026-10-29；基线 `99236cbf40fa4c2fb62393efe48a99dddd11b164`。下列结果记录拆分提交前的组合验证，不冒充后续 SHA 的验证结果；各 PR 自证另行记录对应 head 的复测、CI 和审查。没有使用真实在线模型凭据。

## 实现范围

- Windows 使用系统凭据管理器、对应平台 Node 文件与构建配置；修复目录同步、Node 权限路径、必要系统环境和成功退出崩溃。
- Markdown 编辑器与课程聊天复用附件 IPC 粘贴图片，阅读/对话直接预览，异步导入不串会话、不覆盖期间输入。
- Rust 冻结显式图片的课程、版本及首次引用顺序，Pi 原生图片输入负责规范化与模型消息。未知或纯文本模型跳过当前/历史图片。
- 自定义模型可声明图片能力；每轮最多4张、每张5MiB。接近原生会话容量上限时在加入图片前拒绝，保留原对话。

## 已运行

环境：Windows x64，Node 24.18.0；工作区内 Rust 1.97.1 GNU 与已有 GCC 13.2。均使用隔离合成数据，没有读取或重放用户模型密钥。

| 检查 | 实际结果 |
| --- | --- |
| `npm.cmd run build` | 通过；保留既有 chunk 大小提示 |
| 图片粘贴、前端安全、模型设置 | 27项通过；React/CodeMirror及模拟IPC |
| workspace-flow、ui-flow、course-conversations | 3项通过；React及模拟IPC |
| Pi harness | 18项通过 |
| Pi images、context-architecture、native-session | 52项通过；包含Node权限模式、OpenAI/Anthropic localhost图片请求、多图顺序、纯文本过滤、重试、容量拒绝后重开和全部sidecar本地模块打包清单 |
| Rust native_session 单元测试 | 5项通过；Windows真实文件/目录同步 |
| Rust native_project_scope | 4项通过；凭据后端类型、drive/UNC路径转换、中文路径权限与兄弟目录拒绝 |
| Rust image_context、图片引用语法、model_limit | 2＋2＋6项通过；跨课拒绝、版本、去重与顺序、模型能力保存/重置 |
| Rust model::native_session_tests | 4项通过；真实Rust→受限Node→Pi→localhost，含重开、多会话、工具回执及输出纠正 |
| `cargo check --locked --offline` 默认desktop | 通过；保留2项既有平台条件编译警告 |
| Tauri实际资源复制 | `target/debug/pi-sidecar/image-input.mjs` 与源码哈希一致，`runtime/node.exe` 与官方锁定校验值一致 |

## 失败与修复记录

- Windows Node 正常结束时强制退出触发 libuv 断言；改为关闭stdin并自然退出后 harness 通过。
- Windows canonical路径直接交Node权限模式被拒绝；只在Node边界转换已核验路径，权限测试通过。
- Rust清空子进程环境使Node加密初始化崩溃；保留必要的 `SystemRoot` 后原生调用4项通过。
- 全量前端测试遇到既有 `URL.pathname` Windows路径错误；本次影响到的4个测试文件改用 `fileURLToPath`，不将其他未修测试记为通过。
- 首轮全量Pi测试159项中149通过、10失败。新增的缺省 `model.input` 回归已修并通过架构测试；其余2项为既有Skill样例CRLF匹配，7项为本机无文件符号链接权限。未把这9项失败当作通过。
- 审查发现多图排序和规范化后大小膨胀问题，均已修复并增加回归。4MiB回合余量不构成原有任意工具历史增长的总量保证。
- 审查发现新增图片模块遗漏打包清单，已加入资源映射，并用模块导入清单回归防止遗漏。

## 本人最短验收

1. Windows设置中重新保存模型连接及密钥，执行连接测试，再发送普通问题并重开同一对话。
2. 在课程笔记编辑处粘贴截图，确认预览、保存后重开；导入中继续输入，确认文本保留。
3. 聊天粘贴两张可区分图片，询问第一张和第二张；视觉模型应按顺序处理。
4. 将自定义模型图片能力设为纯文本，再发含图问题；文字可继续，实际请求不含图片。

尚未执行真实在线模型、原生GUI/系统剪贴板、安装包或Windows ARM64验收；没有原生界面截图。Windows旧版本没有落到系统凭据管理器的密钥需本人重新保存，不迁移其他应用凭据。Windows课程AI文件写入仍按原契约明确报不支持，本次未扩大该功能。
