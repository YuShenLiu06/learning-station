#[cfg(test)]
mod cases {
    use super::super::*;
    use crate::{app::Engine, content, drafts, exercises as exercise_store, sessions};
    use std::sync::Arc;

    fn locator(text: &str, quote: &str) -> Locator {
        let at = text.rfind(quote).unwrap();
        let start = text[..at].chars().count();
        Locator {
            start_char: start,
            end_char: start + quote.chars().count(),
        }
    }
    fn plain(text: &str, quote: &str) -> Snapshot {
        snapshot(
            source(
                "doc_fixture",
                "notes/doc_fixture.md".into(),
                &hash(text),
                text.into(),
            ),
            quote,
            Some(&locator(text, quote)),
            vec![],
            None,
        )
    }
    fn empty() -> Context {
        serde_json::from_value(json!({"text":"","manifest":[],"warnings":[],"truncated":false}))
            .unwrap()
    }
    fn with_snapshot(snapshot: Snapshot) -> Context {
        let mut c = empty();
        c.citation_context = Some(CitationContext {
            version: 1,
            references: vec![snapshot],
        });
        c
    }
    fn value(snapshot: &Snapshot) -> Value {
        json!({"type":"quoted_material","documentId":snapshot.source.id,"version":snapshot.source.version,"locator":snapshot.locator,"quotedText":snapshot.quoted_text})
    }
    fn entry(content: &str, data: Value) -> Entry {
        Entry {
            id: String::new(),
            kind: "user".into(),
            content: content.into(),
            created_at: String::new(),
            selection: None,
            related_ids: vec![],
            data: Some(data),
        }
    }
    fn fixture() -> (
        tempfile::TempDir,
        Arc<Engine>,
        crate::exercises::ExerciseDocument,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let x = {
            let mut c = engine.lock().unwrap();
            let doc = c
                .create_section("合成材料", "MATERIAL", vec![], vec![])
                .unwrap();
            let (s, _) = c.start_session(&doc.id, false).unwrap();
            exercise_store::create_manual(
                &mut c.content,
                &s.id,
                &doc.id,
                exercise_store::ManualExercise {
                    question: "完整题干的开头。\n\n为什么使用这个句子？\n\n题干的最后条件。".into(),
                    format: "exact".into(),
                    answers: vec!["HIDDEN_REFERENCE_SENTINEL".into()],
                    explanation: "HIDDEN_EXPLANATION_SENTINEL".into(),
                },
                "citation-supplement-fixture",
            )
            .unwrap()
        };
        let q = x.exercises["exercises"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        (dir, engine, x, q)
    }
    fn reference(p: &exercises::ExerciseProjection, quote: &str) -> exercises::ExerciseReference {
        exercises::ExerciseReference {
            session_id: p.session_id.clone(),
            document_id: p.document_id.clone(),
            exercise_entry_id: p.exercise_entry_id.clone(),
            question_id: p.question_id.clone(),
            part: p.part.clone(),
            entry_id: p.entry_id.clone(),
            text_hash: p.text_hash.clone(),
            selection: quote.into(),
            locator: Some(locator(&p.text, quote)),
        }
    }
    fn attempt(
        engine: &Engine,
        x: &exercise_store::ExerciseDocument,
        q: &str,
        text: &str,
        id: &str,
    ) -> Session {
        let mut c = engine.lock().unwrap();
        let mut e = entry(
            text,
            json!({"exerciseId":q,"exerciseEntryId":x.document.metadata.extra["exerciseEntryId"]}),
        );
        e.kind = "attempt".into();
        sessions::append(&mut c.content, &x.session.id, e, None, id).unwrap()
    }
    #[test]
    fn real_assistant_supplement_uses_the_bound_message_version_and_paragraphs() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let material = c
            .create_section("材料", "UNRELATED_MATERIAL", vec![], vec![])
            .unwrap();
        let (session, _) = c.start_session(&material.id, false).unwrap();
        let body = "FAR_BEFORE\n\nNear before\n\nprefix chosen suffix\n\nNear after\n\nFAR_AFTER";
        let mut answer = entry(body, json!({"runId":"run_citation_answer"}));
        answer.kind = "assistant".into();
        let session = sessions::append(
            &mut c.content,
            &session.id,
            answer,
            None,
            "citation-message-answer",
        )
        .unwrap();
        let answer = session.entries.last().unwrap();
        let r = context::AssistantReference {
            session_id: session.id.clone(),
            entry_id: answer.id.clone(),
            run_id: Some("run_citation_answer".into()),
            text_hash: hash(body),
            selection: "chosen".into(),
            locator: Some(locator(body, "chosen")),
        };
        let frozen = freeze(&mut c.content, &session, &[], &[r.clone()], &[]).unwrap();
        let quote = &frozen.references[0];
        assert_eq!(quote.quoted_text, "chosen");
        assert_eq!(quote.source.id, answer.id);
        assert_eq!(
            quote.source.path,
            format!("ai-answer/{}/{}", session.id, answer.id)
        );
        assert_eq!(quote.source.version, hash(body));
        let auxiliary = quote
            .supplemental_context
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>();
        assert!(auxiliary.contains("Near before"));
        assert!(auxiliary.contains("Near after"));
        assert!(!auxiliary.contains("FAR_"));
        assert!(!auxiliary.contains("UNRELATED_MATERIAL"));
        let mut forged = r.clone();
        forged.session_id = "another_session".into();
        assert!(freeze(&mut c.content, &session, &[], &[forged], &[]).is_err());
        let document = c.content.read(&answer.id).unwrap();
        c.content
            .save(
                &answer.id,
                &content::serialize(&document.metadata, "NEW_MESSAGE_VERSION").unwrap(),
                &document.version,
            )
            .unwrap();
        let updated = sessions::get(&mut c.content, &session.id).unwrap();
        assert!(freeze(&mut c.content, &updated, &[], &[r], &[]).is_err());
        assert!(!serde_json::to_string(&frozen)
            .unwrap()
            .contains("NEW_MESSAGE_VERSION"));
    }
    #[test]
    fn unordered_list_space_and_tab_separators_stop_each_supported_layout() {
        for marker in ["-", "*", "+"] {
            for separator in [" ", "\t"] {
                for layout in ["\n", "\n\n", "\n\nneighbor paragraph\n"] {
                    for newline in ["\n", "\r\n"] {
                        let text = format!("Near before🙂\n\nprefix😀 chosen suffix{layout}{marker}{separator}OUTSIDE_LIST\n\nFAR_AFTER").replace('\n', newline);
                        let frozen = plain(&text, "chosen");
                        assert_eq!(frozen.quoted_text, "chosen");
                        assert_eq!(frozen.source.version, hash(&text));
                        assert_eq!(
                            serde_json::to_value(&frozen.locator).unwrap(),
                            json!(locator(&text, "chosen"))
                        );
                        let auxiliary = frozen
                            .supplemental_context
                            .iter()
                            .map(|p| p.text.as_str())
                            .collect::<String>();
                        assert!(
                            !auxiliary.contains("OUTSIDE_LIST"),
                            "crossed {marker:?}{separator:?} with {layout:?}"
                        );
                        assert!(!auxiliary.contains("FAR_AFTER"));
                        for part in &frozen.supplemental_context {
                            assert_eq!(
                                part.text,
                                text.chars()
                                    .skip(part.range.start_char)
                                    .take(part.range.end_char - part.range.start_char)
                                    .collect::<String>()
                            );
                            assert_eq!(part.source.included_chars, part.text.chars().count());
                            assert!(!part.truncated);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn existing_numeric_boundaries_and_fenced_tab_markers_remain_unchanged() {
        for marker in ["1. ", "1.\t", "1) ", "1)\t", "10.\t", "0) "] {
            for gap in ["\n", "\n\n"] {
                let frozen = plain(
                    &format!("prefix😀 chosen suffix{gap}{marker}OUTSIDE_LIST\n\nFAR_AFTER"),
                    "chosen",
                );
                assert_eq!(frozen.quoted_text, "chosen");
                let auxiliary = serde_json::to_string(&frozen.supplemental_context).unwrap();
                assert!(!auxiliary.contains("OUTSIDE_LIST"));
                assert!(!auxiliary.contains("FAR_AFTER"));
            }
        }
        let frozen = plain("OUTSIDE_BEFORE\n\n```text\n-\tLITERAL_TAB_MARKER\nprefix😀 chosen suffix\n===\n```\n\nOUTSIDE_AFTER", "chosen");
        let auxiliary = frozen
            .supplemental_context
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>();
        assert!(auxiliary.contains("-\tLITERAL_TAB_MARKER"));
        assert!(auxiliary.contains("==="));
        assert!(!auxiliary.contains("OUTSIDE_BEFORE"));
        assert!(!auxiliary.contains("OUTSIDE_AFTER"));
    }
    #[test]
    fn adjacent_paragraph_lists_also_stop_supplement_expansion() {
        for marker in ["- ", "* ", "+ ", "1. ", "  - "] {
            let text = format!("prefix chosen suffix\n\nneighbor paragraph\n{marker}OUTSIDE_LIST\n\nFAR_AFTER_LIST");
            let frozen = plain(&text, "chosen");
            assert_eq!(frozen.quoted_text, "chosen");
            let auxiliary = frozen
                .supplemental_context
                .iter()
                .map(|p| p.text.as_str())
                .collect::<String>();
            assert!(auxiliary.contains("neighbor paragraph"));
            assert!(
                !auxiliary.contains("OUTSIDE_LIST"),
                "crossed adjacent {marker:?}"
            );
            assert!(!auxiliary.contains("FAR_AFTER_LIST"));
        }
    }
    #[test]
    fn later_structural_lines_and_setext_headings_block_neighbor_expansion() {
        for boundary in [
            "| OUTSIDE_BLOCK |\n| --- |",
            "<div>OUTSIDE_BLOCK</div>",
            "> OUTSIDE_BLOCK",
        ] {
            let text =
                format!("Near before\n\nprefix chosen suffix\n{boundary}\n\nFAR_AFTER_BLOCK");
            let frozen = plain(&text, "chosen");
            let auxiliary = frozen
                .supplemental_context
                .iter()
                .map(|p| p.text.as_str())
                .collect::<String>();
            assert!(!auxiliary.contains("OUTSIDE_BLOCK"));
            assert!(!auxiliary.contains("FAR_AFTER_BLOCK"));
        }
        for underline in ["===", "---"] {
            let frozen = plain(
                &format!(
                    "FAR_BEFORE_HEADING\n\nprefix chosen suffix\n{underline}\n\nFAR_AFTER_HEADING"
                ),
                "chosen",
            );
            let auxiliary = frozen
                .supplemental_context
                .iter()
                .map(|p| p.text.as_str())
                .collect::<String>();
            assert!(!auxiliary.contains("FAR_BEFORE_HEADING"));
            assert!(!auxiliary.contains("FAR_AFTER_HEADING"));
        }
    }
    #[test]
    fn paragraph_neighbors_stop_at_lists_without_an_intervening_blank_line() {
        for marker in ["- ", "* ", "+ ", "1. ", "  - "] {
            let text = format!(
                "Near before\n\nprefix chosen suffix\n{marker}LIST_BOUNDARY\n\nFAR_AFTER_LIST"
            );
            let frozen = plain(&text, "chosen");
            assert_eq!(frozen.quoted_text, "chosen");
            let auxiliary = frozen
                .supplemental_context
                .iter()
                .map(|p| p.text.as_str())
                .collect::<String>();
            assert!(auxiliary.contains("Near before"));
            assert!(auxiliary.contains("prefix ") && auxiliary.contains(" suffix"));
            assert!(!auxiliary.contains("LIST_BOUNDARY"), "crossed {marker:?}");
            assert!(
                !auxiliary.contains("FAR_AFTER_LIST"),
                "crossed {marker:?} into a farther paragraph"
            );
        }
    }
    #[test]
    fn source_paragraph_neighbors_preserve_unicode_crlf_fences_and_heading_boundaries() {
        let text="FAR_BEFORE\r\n\r\nNear before\r\n\r\nprefix🙂 chosen suffix\r\n\r\nNear after\r\n\r\nFAR_AFTER";
        let s = plain(text, "chosen");
        let auxiliary = s
            .supplemental_context
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>();
        assert_eq!(s.quoted_text, "chosen");
        assert!(auxiliary.contains("Near before"));
        assert!(auxiliary.contains("Near after"));
        assert!(auxiliary.contains("prefix🙂"));
        assert!(!auxiliary.contains("FAR_"));
        assert!(!auxiliary.contains("chosen"));
        for p in &s.supplemental_context {
            assert_eq!(
                p.text,
                text.chars()
                    .skip(p.range.start_char)
                    .take(p.range.end_char - p.range.start_char)
                    .collect::<String>()
            );
            assert!(!p.truncated);
        }
        let heading = plain(
            "hidden before\n# title\nselected text\n# next\nhidden after",
            "selected",
        );
        let output = serde_json::to_string(&heading.supplemental_context).unwrap();
        assert!(!output.contains("hidden"));
        assert!(!output.contains("title"));
        let fence = plain(
            "outside before\n\n```txt\ninside\n\nselected\n```\n\noutside after",
            "selected",
        );
        let output = serde_json::to_string(&fence.supplemental_context).unwrap();
        assert!(output.contains("inside"));
        assert!(output.contains("```"));
        assert!(!output.contains("outside"));
        let repeated = "same\n\n🙂 second same tail";
        assert_eq!(
            plain(repeated, "same").locator.unwrap().start_char,
            locator(repeated, "same").start_char
        );
        assert!(snapshot(
            source(
                "legacy",
                "notes/legacy.md".into(),
                "version",
                repeated.into()
            ),
            "same",
            None,
            vec![],
            None
        )
        .supplemental_context
        .is_empty());
    }
    #[test]
    fn per_reference_and_actual_projection_budgets_never_rewrite_the_quote() {
        let text = format!("{}QUOTE{}", "🙂".repeat(9000), "后".repeat(9000));
        let snapshot = plain(&text, "QUOTE");
        assert!(snapshot.supplemental_context.iter().any(|p| p.truncated));
        assert!(
            snapshot.quoted_text.len()
                + snapshot
                    .supplemental_context
                    .iter()
                    .map(|p| p.text.len())
                    .sum::<usize>()
                <= 30000
        );
        assert!(
            snapshot.quoted_text.chars().count()
                + snapshot
                    .supplemental_context
                    .iter()
                    .map(|p| p.text.chars().count())
                    .sum::<usize>()
                <= 8000
        );
        let original = serde_json::to_value(&snapshot).unwrap();
        let mut ctx = with_snapshot(snapshot.clone());
        let wire = quote(&mut ctx, &snapshot.source.id, value(&snapshot), 1200).unwrap();
        let v: Value = serde_json::from_str(&wire).unwrap();
        assert!(wire.chars().count() <= 1200);
        assert_eq!(v["quotedText"], "QUOTE");
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), original);
        assert!(ctx.truncated);
        assert!(v["supplementalContextTruncated"].as_bool().unwrap());
        assert_eq!(
            quote(
                &mut with_snapshot(snapshot.clone()),
                &snapshot.source.id,
                value(&snapshot),
                1
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }
    #[test]
    fn full_partial_and_mismatched_material_coverage_only_deduplicate_real_text() {
        let text = "before text\n\nchosen sentinel\n\nafter text";
        let snapshot = plain(text, "chosen sentinel");
        for (version, prefix, expect_pointer) in [
            (snapshot.source.version.clone(), text.to_string(), true),
            (
                snapshot.source.version.clone(),
                text.chars()
                    .take(snapshot.locator.as_ref().unwrap().start_char + 3)
                    .collect(),
                true,
            ),
            (hash("other version"), text.to_string(), false),
        ] {
            let mut ctx = with_snapshot(snapshot.clone());
            ctx.text = prefix.clone();
            ctx.automatic_materials
                .push(crate::automatic_context::MaterialSnapshot {
                    id: snapshot.source.id.clone(),
                    path: snapshot.source.path.clone(),
                    version,
                    text: prefix.clone(),
                    complete: prefix == text,
                    total_bytes: text.len(),
                    start: 0,
                    end: prefix.len(),
                    body_start_char: 0,
                    asset_course: None,
                });
            let v: Value = serde_json::from_str(
                &quote(&mut ctx, &snapshot.source.id, value(&snapshot), 60000).unwrap(),
            )
            .unwrap();
            assert_eq!(v.get("quotedTextProjection").is_some(), expect_pointer);
            if expect_pointer {
                let pieces = v["quotedTextProjection"].as_array().unwrap();
                let reconstructed = pieces
                    .iter()
                    .map(|p| {
                        p["text"].as_str().map(str::to_owned).unwrap_or_else(|| {
                            let a = p["range"]["startChar"].as_u64().unwrap() as usize;
                            let b = p["range"]["endChar"].as_u64().unwrap() as usize;
                            prefix.chars().skip(a).take(b - a).collect()
                        })
                    })
                    .collect::<String>();
                assert_eq!(reconstructed, snapshot.quoted_text);
                assert_eq!(
                    ctx.citation_context.as_ref().unwrap().references[0].quoted_text,
                    snapshot.quoted_text
                );
            }
        }
        let mut metadata = with_snapshot(snapshot.clone());
        metadata.text = "summary metadata".into();
        metadata.manifest.push(snapshot.source.clone());
        let v: Value = serde_json::from_str(
            &quote(&mut metadata, &snapshot.source.id, value(&snapshot), 60000).unwrap(),
        )
        .unwrap();
        assert_eq!(v["quotedText"], "chosen sentinel");
        assert!(v.get("quotedTextProjection").is_none());
    }
    #[test]
    fn source_markers_and_supplement_inventory_preserve_exact_ranges_and_truncation() {
        let text = "left paragraph\n\nchosen\n\nright paragraph";
        let snapshot = plain(text, "chosen");
        let mut ctx = with_snapshot(snapshot.clone());
        ctx.text = text.into();
        ctx.automatic_materials
            .push(crate::automatic_context::MaterialSnapshot {
                id: snapshot.source.id.clone(),
                path: snapshot.source.path.clone(),
                version: snapshot.source.version.clone(),
                text: text.into(),
                complete: true,
                total_bytes: text.len(),
                start: 0,
                end: text.len(),
                body_start_char: 0,
                asset_course: None,
            });
        let _ = quote(&mut ctx, &snapshot.source.id, value(&snapshot), 60000).unwrap();
        let items = inventory(&ctx);
        assert_eq!(items[0]["sourceMarkers"], json!(["citation", "material"]));
        assert_eq!(
            items[0]["locator"],
            serde_json::to_value(snapshot.locator).unwrap()
        );
        assert!(items
            .iter()
            .skip(1)
            .all(|p| p["reason"] == "explicit_supplemental_context" && p["truncated"] == false));
    }
    #[test]
    fn real_user_receipt_keeps_original_auxiliary_snapshot_and_rejects_client_forgery() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let doc = c
            .create_section("合成原文", "left\n\nselected\n\nright", vec![], vec![])
            .unwrap();
        let (s, _) = c.start_session(&doc.id, false).unwrap();
        let input = context::ContextInjection {
            document_id: doc.id.clone(),
            expected_version: doc.version.clone(),
            selection: Some("selected".into()),
            locator: Some(locator(&doc.body, "selected")),
        };
        let e = entry(
            "解释选段",
            json!({"mode":"explain","requestedContext":{"documentId":doc.id,"selection":"","injections":[input]}}),
        );
        let saved = sessions::append(
            &mut c.content,
            &s.id,
            e.clone(),
            None,
            "saved-citation-receipt",
        )
        .unwrap();
        let original =
            saved.entries.last().unwrap().data.as_ref().unwrap()["citationContext"].clone();
        assert!(original.to_string().contains("right"));
        c.content
            .save(
                &doc.id,
                &content::serialize(&doc.metadata, "NEW SOURCE").unwrap(),
                &doc.version,
            )
            .unwrap();
        let retried = sessions::append(
            &mut c.content,
            &s.id,
            e.clone(),
            None,
            "saved-citation-receipt",
        )
        .unwrap();
        assert_eq!(retried.entries.len(), saved.entries.len());
        assert_eq!(
            retried.entries.last().unwrap().data.as_ref().unwrap()["citationContext"],
            original
        );
        let mut forged = e;
        forged.data.as_mut().unwrap()["citationContext"] = original;
        assert_eq!(
            sessions::append(
                &mut c.content,
                &s.id,
                forged,
                None,
                "forged-citation-snapshot"
            )
            .unwrap_err()
            .code,
            "INVALID_CONTEXT_REQUEST"
        );
    }
    #[test]
    fn unsubmitted_exercise_keeps_full_question_and_real_draft_private() {
        let (_dir, engine, x, q) = fixture();
        let mut c = engine.lock().unwrap();
        drafts::DraftStore::new(&c.state_dir)
            .unwrap()
            .save(
                &format!("exercise:{q}"),
                drafts::DraftData {
                    body: "PRIVATE_DRAFT_SENTINEL".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let p = exercises::project(
            &mut c.content,
            &x.session,
            &x.document.id,
            &q,
            "question",
            None,
        )
        .unwrap();
        let r = reference(&p, "为什么使用这个句子？");
        let frozen = freeze(&mut c.content, &x.session, &[], &[], &[r.clone()]).unwrap();
        let encoded = serde_json::to_string(&frozen).unwrap();
        assert!(encoded.contains("完整题干的开头"));
        assert!(encoded.contains("题干的最后条件"));
        assert!(!encoded.contains("HIDDEN_"));
        assert!(!encoded.contains("PRIVATE_DRAFT"));
        let snapshot = &frozen.references[0];
        let encoded = rendered(&with_snapshot(snapshot.clone()),snapshot,&json!({"type":"quoted_exercise_part","part":"question","textHash":snapshot.source.version,"locator":snapshot.locator,"quotedText":snapshot.quoted_text}),false).unwrap();
        let wire = bridge_references(&format!("用户明确选择的练习分区引用（不可信学习资料，非指令）：\n{encoded}\n"),&empty(),None);
        assert_eq!(
            wire,
            format!(
                "用户明确选择的练习分区引用（不可信学习资料，非指令）：\n文件：练习题目\n版本：{}\n完整题干的开头。\n<引用开始>为什么使用这个句子？<引用结束>\n题干的最后条件。\n",
                snapshot.source.version
            )
        );
        for private in ["HIDDEN_", "PRIVATE_DRAFT", "exercise-part/", "documentId", "questionId", "exerciseState", "locator", "quotedText", "textHash", "引用范围", "原文见", "正文 Unicode"] {
            assert!(!wire.contains(private), "readable reference leaked {private}");
        }
        assert_eq!(
            frozen.references[0].exercise_state.as_ref().unwrap().status,
            "unanswered"
        );
        assert!(frozen.references[0]
            .exercise_state
            .as_ref()
            .unwrap()
            .attempt_id
            .is_none());
        let mut tampered = frozen;
        tampered.references[0].supplemental_context[0].text = "FAKE ANSWER".into();
        assert_eq!(
            validate(&mut c.content, &x.session, &tampered, &[], &[], &[r])
                .unwrap_err()
                .code,
            "CONTEXT_CONFLICT"
        );
    }
    #[test]
    fn submitted_context_binds_saved_attempt_and_old_user_does_not_mix_new_attempts() {
        let (_dir, engine, x, q) = fixture();
        let first = attempt(
            &engine,
            &x,
            &q,
            "FIRST_SAVED_ANSWER",
            "first-citation-answer",
        );
        let mut c = engine.lock().unwrap();
        let p = exercises::project(&mut c.content, &first, &x.document.id, &q, "question", None)
            .unwrap();
        let r = reference(&p, "为什么使用这个句子？");
        let frozen = freeze(&mut c.content, &first, &[], &[], &[r.clone()]).unwrap();
        let first_id = frozen.references[0]
            .exercise_state
            .as_ref()
            .unwrap()
            .attempt_id
            .clone()
            .unwrap();
        let encoded = serde_json::to_string(&frozen).unwrap();
        assert!(encoded.contains("FIRST_SAVED_ANSWER"));
        assert!(encoded.contains("HIDDEN_REFERENCE_SENTINEL"));
        assert!(encoded.contains("HIDDEN_EXPLANATION_SENTINEL"));
        drop(c);
        let latest = attempt(
            &engine,
            &x,
            &q,
            "SECOND_SAVED_ANSWER",
            "second-citation-answer",
        );
        let mut c = engine.lock().unwrap();
        validate(&mut c.content, &latest, &frozen, &[], &[], &[r.clone()]).unwrap();
        let fresh = freeze(&mut c.content, &latest, &[], &[], &[r]).unwrap();
        assert_ne!(
            fresh.references[0]
                .exercise_state
                .as_ref()
                .unwrap()
                .attempt_id
                .as_ref(),
            Some(&first_id)
        );
        assert!(serde_json::to_string(&fresh)
            .unwrap()
            .contains("SECOND_SAVED_ANSWER"));
        assert!(!serde_json::to_string(&frozen)
            .unwrap()
            .contains("SECOND_SAVED_ANSWER"));
    }
    #[test]
    fn serialized_material_records_body_origin_and_real_bridge_pointers_do_not_repeat_text() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let doc = c
            .create_section(
                "真实合成元数据",
                "before\n\n🙂selected。 tail\n\nafter",
                vec![],
                vec![],
            )
            .unwrap();
        let (s, mut ctx) = c.start_session(&doc.id, false).unwrap();
        let input = context::ContextInjection {
            document_id: doc.id.clone(),
            expected_version: doc.version.clone(),
            selection: Some("selected。".into()),
            locator: Some(locator(&doc.body, "selected。")),
        };
        let body_start = ctx.automatic_materials[0].body_start_char;
        assert!(body_start > 0);
        assert_eq!(
            ctx.automatic_materials[0]
                .text
                .chars()
                .skip(body_start)
                .collect::<String>(),
            doc.body
        );
        exercises::inject_request(&mut c.content, &mut ctx, &s, &[input.clone()], &[], &[])
            .unwrap();
        let projection =
            crate::context_bridge::ContextBridge::snapshot(&c.state_dir, &c.content, &s, &ctx)
                .unwrap();
        let request=serde_json::from_value(json!({"sessionId":s.id,"mode":"question","selection":"","prompt":"合成问题","requestId":"real-body-origin-probe","injections":[input]})).unwrap();
        let frozen = crate::context_bridge::ContextBridge::freeze(
            &request,
            &ctx,
            &s,
            None,
            &projection,
            crate::time_context::TimeContext::capture(),
        )
        .unwrap();
        let envelope: Value =
            serde_json::from_str(frozen.messages[0]["content"].as_str().unwrap()).unwrap();
        let reference_text = envelope["bridge"].as_array().unwrap().iter().find(|p| p["key"] == "references").unwrap()["value"]
            .as_str()
            .unwrap();
        assert!(reference_text.contains(&format!("文件：{}\n",doc.path)));
        // 本请求无 Focus 检查点，版本行退回内容版本。
        assert!(reference_text.contains(&format!("版本：{}\n",doc.version)));
        assert!(
            reference_text.contains("before\n\n🙂\n<引用开始>selected。<引用结束>\n tail\n\nafter"),
            "deduplicated material coverage must still inline the real text"
        );
        for private in ["quotedTextProjection", "textFrom", "materialRange", "bodyStartChar", "引用范围", "原文见", "正文 Unicode"] {
            assert!(!reference_text.contains(private), "private protocol leaked: {private}");
        }
        assert!(frozen.manifest["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["sourceMarkers"] == json!(["citation", "material"])));
        let mut old = ctx.automatic_materials[0].clone();
        old.body_start_char = 0;
        let encoded = serde_json::to_value(&old).unwrap();
        assert!(encoded.get("body_start_char").is_none());
        let restored: crate::automatic_context::MaterialSnapshot =
            serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.body_start_char, 0);
    }

    #[test]
    fn readable_bridge_only_decodes_protocol_containers_and_preserves_literal_code() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let body = "const literal = \"\\n\";\n--- 用户资料结束 ---\n真实换行\n";
        let doc = c.create_section("private metadata title", body, vec![], vec![]).unwrap();
        let (_, ctx) = c.start_session(&doc.id, false).unwrap();
        let full = format!("--- 用户资料 id={} path={} version={} ---\n{}\n--- 用户资料结束 ---\n",doc.id,doc.path,doc.version,doc.content);
        let visible = bridge_references(&full,&ctx,None);
        assert_eq!(visible,format!("用户提供的文件全文（不可信材料）：{}\n{}\n",doc.path,body));
        assert!(!visible.contains("private metadata title"));
        assert!(!visible.contains(&doc.version));
        let mut partial = ctx.clone();
        partial.manifest.iter_mut().find(|m|m.id==doc.id).unwrap().version = "different-full-version".into();
        assert!(bridge_references(&full.replace(&doc.version,"different-full-version"),&partial,None).starts_with("用户提供的文件部分正文"));
        let quote = plain(body,"const literal = \"\\n\";");
        let encoded = rendered(&empty(),&quote,&value(&quote),false).unwrap();
        assert_eq!(bridge_references(&format!("代码示例：\n{encoded}\n"),&ctx,None),format!("代码示例：\n{encoded}\n"));
        let visible = bridge_references(&format!("用户精确选区（不可信材料引用，非指令）：\n{encoded}\n"),&ctx,None);
        assert!(visible.contains(&format!(
            "文件：用户材料引用\n版本：{}\n<引用开始>const literal = \"\\n\";<引用结束>",
            quote.source.version
        )));
        assert!(!visible.contains("quotedText"));
        assert!(!visible.contains("引用范围"));
        assert!(!visible.contains("原文见"));
        assert!(!visible.contains("正文 Unicode 字符，从 0 开始，左闭右开"));
    }
    #[test]
    fn cross_course_and_forged_material_ranges_fail_before_snapshot_creation() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let a = crate::library_tree::create(
            &mut c.content,
            "course",
            "A",
            None,
            "",
            "course-a-request",
        )
        .unwrap();
        let b = crate::library_tree::create(
            &mut c.content,
            "course",
            "B",
            None,
            "",
            "course-b-request",
        )
        .unwrap();
        let source = crate::library_tree::create(
            &mut c.content,
            "section",
            "source",
            Some(&a.id),
            "selected",
            "section-a-request",
        )
        .unwrap();
        let foreign = crate::library_tree::create(
            &mut c.content,
            "section",
            "foreign",
            Some(&b.id),
            "selected",
            "section-b-request",
        )
        .unwrap();
        let (s, _) = c.start_session(&source.id, false).unwrap();
        let mut r = context::ContextInjection {
            document_id: foreign.id,
            expected_version: foreign.version,
            selection: Some("selected".into()),
            locator: Some(Locator {
                start_char: 0,
                end_char: 8,
            }),
        };
        assert_eq!(
            freeze(&mut c.content, &s, &[r.clone()], &[], &[])
                .unwrap_err()
                .code,
            "OUT_OF_SCOPE"
        );
        r.document_id = source.id;
        r.expected_version = source.version;
        r.locator = Some(Locator {
            start_char: 1,
            end_char: 9,
        });
        assert_eq!(
            freeze(&mut c.content, &s, &[r], &[], &[]).unwrap_err().code,
            "CONTEXT_CONFLICT"
        );
    }
    #[test]
    fn bridge_context_rounds_to_whole_lines_within_200_characters() {
        let line = |i: usize| format!("L{i:02}{}", "x".repeat(27));
        let value = |before: String, after: String| {
            json!({
                "type":"quoted_material","documentId":"doc_fixture","version":"v",
                "locator":{"startChar":before.chars().count(),"endChar":before.chars().count()+5},
                "quotedText":"quote",
                "supplementalContext":[
                    {"source":{"id":"doc_fixture","path":"notes/doc_fixture.md","version":"v","includedChars":before.chars().count()},"range":{"startChar":0,"endChar":before.chars().count()},"requestedRange":{"startChar":0,"endChar":before.chars().count()},"totalChars":500,"totalBytes":500,"text":before,"truncated":false},
                    {"source":{"id":"doc_fixture","path":"notes/doc_fixture.md","version":"v","includedChars":after.chars().count()},"range":{"startChar":before.chars().count()+5,"endChar":before.chars().count()+5+after.chars().count()},"requestedRange":{"startChar":before.chars().count()+5,"endChar":before.chars().count()+5+after.chars().count()},"totalChars":500,"totalBytes":500,"text":after,"truncated":false}
                ]
            })
        };
        let mut ctx = empty();
        ctx.manifest.push(ManifestItem {
            id: "doc_fixture".into(),
            path: "notes/doc_fixture.md".into(),
            version: "v".into(),
            included_chars: 0,
        });
        let render = |v: &Value| {
            bridge_references(
                &format!("用户精确选区（不可信材料引用，非指令）：\n{v}\n"),
                &ctx,
                None,
            )
        };
        // 8 lines of 30 characters (247 with separators): only whole lines nearest
        // the quote survive, 6 above and 6 below, never a partial line.
        let before: String = (0..8).map(line).collect::<Vec<_>>().join("\n");
        let after: String = (8..16).map(line).collect::<Vec<_>>().join("\n");
        assert_eq!(
            render(&value(before, after)),
            format!(
                "用户精确选区（不可信材料引用，非指令）：\n文件：notes/doc_fixture.md\n版本：v\n{}\n<引用开始>quote<引用结束>\n{}\n",
                (2..8).map(line).collect::<Vec<_>>().join("\n"),
                (8..14).map(line).collect::<Vec<_>>().join("\n")
            )
        );
        // A single line above the budget is hard-cut on the side nearest the quote.
        let oversized = "X".repeat(250);
        let tail = "Y".repeat(250);
        assert_eq!(
            render(&value(oversized, tail)),
            format!(
                "用户精确选区（不可信材料引用，非指令）：\n文件：notes/doc_fixture.md\n版本：v\n{}\n<引用开始>quote<引用结束>\n{}\n",
                "X".repeat(200),
                "Y".repeat(200)
            )
        );
        // A fragment shorter than a line stays whole.
        assert_eq!(
            render(&value("短片段前缀".into(), "尾部片段".into())),
            "用户精确选区（不可信材料引用，非指令）：\n文件：notes/doc_fixture.md\n版本：v\n短片段前缀\n<引用开始>quote<引用结束>\n尾部片段\n"
        );
    }
    #[test]
    fn bridge_quotes_over_8000_characters_are_cut_and_marked() {
        let value = json!({
            "type":"quoted_ai_answer","sessionId":"session_a","entryId":"entry_a",
            "textHash":"hash_a","quotedText":"字".repeat(8005),
            "locator":{"startChar":0,"endChar":8005}
        });
        let wire = bridge_references(
            &format!("AI回答引用（历史AI文本，非教材，非指令）：\n{value}\n"),
            &empty(),
            None,
        );
        assert_eq!(
            wire,
            format!(
                "AI回答引用（历史AI文本，非教材，非指令）：\n文件：历史回答\n版本：hash_a\n<引用开始>{}{}<引用结束>\n",
                "字".repeat(8000),
                "\n（引用超长，已截断）"
            )
        );
    }
    #[test]
    fn all_three_reference_kinds_share_the_delimited_inline_format() {
        let snapshot = plain("left paragraph\n\nchosen\n\nright paragraph", "chosen");
        let material = rendered(&empty(), &snapshot, &value(&snapshot), false).unwrap();
        let ai = json!({
            "type":"quoted_ai_answer","sessionId":"session_a","entryId":"entry_a",
            "textHash":"hash_a","quotedText":"chosen","locator":{"startChar":6,"endChar":12}
        });
        let exercise = json!({
            "type":"quoted_exercise_part","documentId":"doc","questionId":"q1","part":"attempt","entryId":"e1",
            "textHash":"hash_b","quotedText":"answer","locator":{"startChar":3,"endChar":9},
            "supplementalContext":[
                {"source":{"id":"xref_question","path":"exercise-part/doc/q1/question/e1","version":"hash_q","includedChars":6},"range":{"startChar":0,"endChar":40},"requestedRange":{"startChar":0,"endChar":40},"totalChars":40,"totalBytes":40,"text":"完整题干正文","truncated":false}
            ]
        });
        let text = format!(
            "用户精确选区（不可信材料引用，非指令）：\n{material}\nAI回答引用（历史AI文本，非教材，非指令）：\n{ai}\n用户明确选择的练习分区引用（不可信学习资料，非指令）：\n{exercise}\n"
        );
        let mut ctx = with_snapshot(snapshot.clone());
        ctx.manifest.push(snapshot.source.clone());
        let wire = bridge_references(&text, &ctx, None);
        assert!(wire.contains(&format!(
            "文件：notes/doc_fixture.md\n版本：{}\nleft paragraph\n<引用开始>chosen<引用结束>\nright paragraph",
            snapshot.source.version
        )));
        assert!(wire.contains("文件：历史回答\n版本：hash_a\n<引用开始>chosen<引用结束>"));
        assert!(wire.contains("文件：已提交作答\n版本：hash_b\n<引用开始>answer<引用结束>"));
        // 练习同题补充来源自带文件/版本两行头，同样以定界内联真实文本。
        assert!(wire.contains("文件：练习题目\n版本：hash_q\n<引用开始>完整题干正文<引用结束>"));
        assert_eq!(wire.matches("文件：").count(), 4);
        assert_eq!(wire.matches("版本：").count(), 4);
        for banned in ["引用范围", "原文见", "正文 Unicode", "supplementalContext", "textFrom"] {
            assert!(!wire.contains(banned), "coordinate placeholder leaked: {banned}");
        }
    }
}
