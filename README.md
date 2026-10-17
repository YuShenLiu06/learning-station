# 学习站 Learning Station

以本机 Markdown 为中心的学习工作区，串联阅读、提问、练习、错因和复习。

**当前为源码预览。** 桌面目标为 macOS arm64；本版尚未完成原生界面和真实在线服务的端到端验收，没有正式下载发行包，开发构建未进行 Developer ID 签名或公证。

- 课程目录、普通探索笔记、受控图片/代码资料，以及版本保存、草稿恢复和可恢复回收。
- 按问题附加资料、AI 回答或练习分区引用；失败重试保留原问题和来源，工具保存以实际回执为准。
- 结构化练习、题下作答、反馈和本人错因；主观题批改需要自行配置模型。
- Pi 原生会话负责消息、工具结果、准确重开和已知容量下的压缩；默认编码工具未开放。
- 本机 Anki/AnkiConnect 复习与 CSV 备用导出；本人飞书机器人的显式启停、通知和手动测试。

模型端点与凭据由本人配置，密钥写入本机 Keychain。飞书与 Anki 需要已有程序/授权，应用不自动安装、授权或发送；Coding Plan 用途限制仍保留，不自动绕过。

## 本机开发

准备 Rust、Xcode Command Line Tools 和 **Node v24.18.0（darwin arm64）**。运行时版本及校验值锁定在 `runtime/manifest.json`；构建只复制现有 Node，不自动下载运行时。

```sh
npm ci --ignore-scripts
npm --prefix pi-sidecar ci --ignore-scripts
npm --prefix tools/feishu-bot ci --ignore-scripts
node scripts/prepare-runtime.mjs
npm run tauri -- dev
```

开发环境使用项目内 `runtime/node`。旧 CLI 兼容适配器不带个人配置，固定路径仅为开发占位；日常飞书通知使用独立 Bot 适配器。

## 验证状态与范围

公开前已有本机 Core、Pi/localhost 和 React/mock IPC 回归；这些不等于本版原生界面或真实账户验收。本次发布没有另跑产品测试。

支持 HTML 代码块静态隔离预览，以及笔记中的 HTML/网页/视频链接；HTML 脚本与外部资源禁用，远程站点限制不会绕过。设置中可自选模型工具授权档位（受控读取/读写/命令，默认关闭；命令仅在会话工作区内受限执行）。自动识别当前文档/题目的 focus 与通用网络权限尚未完成。不保证应用退出或休眠期间及时派发提醒。

实现边界见 [架构](docs/architecture.md)、[ContextBridge](docs/context-bridge.md)和[飞书配置](docs/feishu-setup.md)。

## 许可

本项目采用 [MIT](LICENSE)，Copyright © 2026 YuShen。第三方代码、运行时和依赖保留各自许可；见 [第三方声明](THIRD_PARTY_NOTICES/pi-readcache.txt)、[Node 许可](runtime/LICENSE)及依赖锁文件。
