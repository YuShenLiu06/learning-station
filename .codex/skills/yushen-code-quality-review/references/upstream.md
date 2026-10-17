# 来源及适配

核验日期：2026-10-17。以下是外部方法借鉴，不标作用户原创。

- [Addy Osmani：code-review-and-quality](https://github.com/addyosmani/agent-skills/blob/main/skills/code-review-and-quality/SKILL.md)：借鉴复用现有能力、多维检查、区分必要修改和建议、核对测试证据。未采用自动变异修改步骤、固定第三次才抽象及一概强制清理历史债务的规则。
- [Thomas Ricouard：review-and-simplify-changes](https://github.com/Dimillian/Skills/blob/main/review-and-simplify-changes/SKILL.md)：借鉴复用/质量/效率/清晰度四个视角、只读子审查与去重输出。固定使用 review-only；未采用 safe-fixes 或 fix-and-validate 模式，不强制四代理并发。

用户要求：高复用、检查死代码与重复造轮子、给出可核实量化、网上替代只在 PR 建议。计数公式、初试阈值与具体执行细则由助手补充，需真实试验校准。

本版是中文精简适配，不宣称完整移植上游，不依赖运行上游脚本。源码主分支链接可变；再次更新先比较来源变化，不自动拉入新行为。两项上游均为 MIT；版权与许可保留于本目录上层 THIRD_PARTY_NOTICES.txt。
