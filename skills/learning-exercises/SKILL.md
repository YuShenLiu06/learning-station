---
name: learning-exercises
description: 根据给定材料和自然语言目标设计跨学科练习，按学习站现有结构固定题目、参考与评分量表。
---

依据本次材料、目标、疑问和已有错误出题，不预设学科；题型与难度应能检验所学理解，而非复制原文凑题。

- 用明确题面检验一个或相连的少量目标，难度适度；必要时使用小案例、迁移或对比。
- 短且有限答案才用 exact；开放作答用 self。接受成立的替代解法，避免把措辞差异当成知识错误。
- 主观题在用户作答前固定合理的 rubric，各项说明可判断、分值符合现有约束；不要事后改变标准或冒充官方考试评分。
- 严格按本次提供的结构输出题目、参考答案、解析、来源和量表，不增造字段或来源ID。参考交由程序隐藏，题面和摘要不提前泄露答案。
- 练习按钮与 generate_exercises 工具遵循同一指导；工具参数仍须通过现有校验与持久化回执，文字不代表保存成功。

修改已有练习或解释本地 Markdown 格式时，按需加载 `learning-exercise-markdown`。已有题目的修订结构不同于下面的新建参数，不要直接重写页面正文来改变评分答案。

## 完整样例

下面两组样例字段与两侧真实校验完全一致：exact（客观题）与 self（主观自评）各一组；每组把「generate_exercises 工具入参」与「practice 模式最终输出」分成两份 JSON 成对展示，不可混用或合并。

- 工具入参 JSON：在 question/explain 等模式经 generate_exercises 工具保存练习时作为参数提交，按工具 schema 校验；顶层只有 requirements（本轮用户要求）、summary、exercises 三项。
- 最终输出 JSON：practice 模式直接输出的练习产物，交由 Rust 校验后保存；顶层只有 exercises、summary 两项，没有 requirements，也不要写 id、answerMatch 等程序生成字段（校验通过后由程序补齐）。
- 样例中的 sourceIds（src_demo_material、src_demo_note）仅为示意；实际取值只能来自本轮授权引用清单（context manifest）中的稳定 ID（使用“本轮练习可引用来源”的 sourceId，文件路径不是 sourceId），不得重复、不得虚构，AI 回复里的编号不算来源。

### 样例一：exact（客观题）

generate_exercises 工具入参：

```json
{
  "requirements": "依据本次材料出一道单位换算客观题，只检验一个目标，答案短且有限。",
  "summary": "考查千米与米的换算，并接受千分位等价写法。",
  "exercises": [
    {
      "question": "3.5 千米等于多少米？只填写数值。",
      "format": "exact",
      "answers": ["3500", "3,500"],
      "explanation": "1 千米 = 1000 米，3.5 × 1000 = 3500；3,500 是千分位写法，同为正确。",
      "tags": ["单位换算"],
      "sourceIds": ["src_demo_material"]
    }
  ]
}
```

practice 模式最终输出：

```json
{
  "exercises": [
    {
      "question": "3.5 千米等于多少米？只填写数值。",
      "format": "exact",
      "answers": ["3500", "3,500"],
      "explanation": "1 千米 = 1000 米，3.5 × 1000 = 3500；3,500 是千分位写法，同为正确。",
      "tags": ["单位换算"],
      "sourceIds": ["src_demo_material"]
    }
  ],
  "summary": "针对单位换算疑点：等价写法全部列入参考答案，按去首尾空白后的文本精确核对。"
}
```

### 样例二：self（主观自评）

generate_exercises 工具入参：

```json
{
  "requirements": "出一道开放解释题，用户用自己的话说明现象与依据，作答前先固定评分量表。",
  "summary": "考查比热容概念与昼夜温差解释，附固定评分量表。",
  "exercises": [
    {
      "question": "用自己的话解释：为什么海边的昼夜温差通常比内陆小？说明判断依据。",
      "format": "self",
      "answers": ["海水比热容比沙石大，吸收或放出同样热量时温度变化更慢，从而调节了海边气温。"],
      "explanation": "开放作答按量表评分，参考答案只是一种表述，同义或更完整的解释同样成立。",
      "tags": ["比热容", "现象解释"],
      "sourceIds": ["src_demo_material", "src_demo_note"],
      "rubric": {
        "criteria": [
          {
            "id": "heat_capacity",
            "label": "比热容差异",
            "description": "指出海水比热容大于内陆沙石，同等热量下温度变化更小。",
            "maxPoints": 4
          },
          {
            "id": "regulation",
            "label": "调节过程",
            "description": "说明白天升温慢、夜间降温慢，两者共同缩小昼夜温差。",
            "maxPoints": 4
          },
          {
            "id": "evidence",
            "label": "依据引用",
            "description": "引用本轮材料的原文或数据支持结论，不虚构来源。",
            "maxPoints": 2
          }
        ]
      }
    }
  ]
}
```

practice 模式最终输出：

```json
{
  "exercises": [
    {
      "question": "用自己的话解释：为什么海边的昼夜温差通常比内陆小？说明判断依据。",
      "format": "self",
      "answers": ["海水比热容比沙石大，吸收或放出同样热量时温度变化更慢，从而调节了海边气温。"],
      "explanation": "开放作答按量表评分，参考答案只是一种表述，同义或更完整的解释同样成立。",
      "tags": ["比热容", "现象解释"],
      "sourceIds": ["src_demo_material", "src_demo_note"],
      "rubric": {
        "criteria": [
          {
            "id": "heat_capacity",
            "label": "比热容差异",
            "description": "指出海水比热容大于内陆沙石，同等热量下温度变化更小。",
            "maxPoints": 4
          },
          {
            "id": "regulation",
            "label": "调节过程",
            "description": "说明白天升温慢、夜间降温慢，两者共同缩小昼夜温差。",
            "maxPoints": 4
          },
          {
            "id": "evidence",
            "label": "依据引用",
            "description": "引用本轮材料的原文或数据支持结论，不虚构来源。",
            "maxPoints": 2
          }
        ]
      }
    }
  ],
  "summary": "针对比热容疑点：无唯一标准答案，按固定量表评分并接受同义表述。"
}
```
