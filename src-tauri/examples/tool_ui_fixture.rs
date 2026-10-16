//! Isolated native UI acceptance fixture: invokes the real Rust learning tools,
//! without keys, a model request, or the default user's library.
use learning_station_core::{
    app::Engine,
    learning_tools::{execute, ToolScope},
};
use serde_json::json;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root =
        std::env::temp_dir().join(format!("learning-station-tool-ui-{}", uuid::Uuid::new_v4()));
    let library = root.join("library");
    let state = root.join("state");
    let engine = Engine::new(library.clone(), state.clone())?;
    let scope = {
        let mut core = engine.lock()?;
        let section = core.create_section("Pi学习工具 · 隔离验收示例", "> 本文仅用于工具与界面验收，不是用户真实学习记录或在线AI答案。\n\nInstead of giving up, she changed the plan.\n\nCompare instead of with because of.", vec![], vec!["工具验收示例".into()])?;
        let (session, context) = core.start_session(&section.id, false)?;
        ToolScope {
            session_id: session.id,
            section_id: section.id,
            run_id: "run_native_tool_fixture".into(),
            allowed_ids: context.manifest.into_iter().map(|item| item.id).collect(),
        }
    };
    let question = execute(
        &engine,
        &scope,
        "native-question",
        "record_question",
        json!({"question":"为什么 instead of 在这里表示替代而不是原因？","selection":"Instead of giving up"}),
    )?;
    let error = execute(
        &engine,
        &scope,
        "native-error",
        "create_error",
        json!({"front":"instead of 与 because of 的关系区别是什么？","back":"前者表示替代选择，后者表示原因。此处是验收用参考解释。","cause":"可能混淆替代与因果；仅建议，待本人核对。","tags":["工具验收示例"]}),
    )?;
    let card = execute(
        &engine,
        &scope,
        "native-card",
        "create_review_card",
        json!({"front":"instead of 表达什么关系？","back":"表达替代选择。此卡由真实Rust工具创建，仅供验收。","tags":["工具验收示例"]}),
    )?;
    let note = execute(
        &engine,
        &scope,
        "native-note",
        "create_note",
        json!({"content":"验收笔记：先找原句证据，再区分替代与因果。","selection":"Instead of giving up"}),
    )?;
    let knowledge = execute(
        &engine,
        &scope,
        "native-query",
        "query_related_knowledge",
        json!({"query":"instead of"}),
    )?;
    println!(
        "{}",
        json!({"fixture":true,"realModel":false,"library":library,"state":state,"question":question,"error":error,"card":card,"note":note,"knowledge":knowledge})
    );
    Ok(())
}
