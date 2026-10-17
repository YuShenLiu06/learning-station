# 隔离内容 v1

笔记使用 fenced `learning-embed` JSON 保存 `{kind, source, content?}`；kind 是 html/web/video，只有 html 含原文 content。本地文件由用户通过文件选择器读取，原名和完整原文进入现有 Markdown 保存/版本/草稿流程，不授予任意文件读取权限。HTML 最多200000 UTF-16单元；文件选择额外限制200KB。非法块保留为代码，不执行。

AI 回复顶层的 html fenced code 使用同一预览组件；原始回复不改写。预览须点击打开，可就近重新加载、收起与查看原文。嵌套引用中的代码仍作为引用文本，避免自动执行语义。

本地 HTML 是静态预览：DOMPurify清理活动标签、跳转、表单和事件；空 sandbox 为不透明源并禁用脚本/弹窗/表单/顶层导航；srcdoc CSP 默认拒绝，仅允许内联样式与data图片。无本机文件、IPC、命令、远程请求或原生桥权限。JavaScript交互演示不在本版支持范围。

远程内容仅接收公共域名的无凭据 HTTPS 标准端口 URL，保留原链接；点击才设置 iframe src。sandbox只允许scripts，不允许same-origin/forms/popups/top-navigation/downloads，拒绝相机、麦克风、位置、剪贴板等能力；referrer-policy为no-referrer。主窗口CSP允许https frame，不放开主窗口脚本/连接策略；Tauri锁定版本的初始化脚本仅注入主frame，capability未配置remote授权。远程站点会收到网络请求，服务端限制/登录/第三方策略可能导致不可嵌入。

跨源iframe的load事件不能证实成功，因此界面始终提示空白、离线、禁止嵌入的可能与原链接后备入口；不代理下载、不绕过CSP/X-Frame-Options。原链接仅经用户按钮送Rust再次验证，用固定 `/usr/bin/open` 和独立参数打开；不执行shell。此入口仅针对仓库当前macOS目标。
