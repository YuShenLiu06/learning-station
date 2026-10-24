//! Canonical Markdown regions and durable per-region decisions. No client text is accepted.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Review {
    version: Option<String>,
    hunks: Vec<Hunk>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hunk {
    id: String,
    start: usize,
    end: usize,
    after: String,
    status: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    receipt: Option<Value>,
}
// Blank-line blocks keep paragraphs, lists, tables and fenced code together.
// Exact bytes (including CRLF and the final newline) survive every decision.
fn blocks(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied().unwrap_or(0);
        let count = trimmed.bytes().take_while(|b| *b == marker).count();
        if matches!(marker, b'`' | b'~') && count >= 3 {
            match fence {
                None => fence = Some((marker, count)),
                Some((m, n)) if m == marker && count >= n && trimmed[count..].trim().is_empty() => {
                    fence = None
                }
                _ => (),
            }
        }
        offset += line.len();
        if fence.is_none() && line.trim().is_empty() {
            result.push(&text[start..offset]);
            start = offset;
        }
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}
impl Review {
    fn new(p: &Proposal) -> Self {
        let a = blocks(&p.original);
        let b = blocks(&p.content);
        let mut prefix = 0;
        let mut suffix = 0;
        while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
            prefix += 1;
        }
        while suffix < a.len().min(b.len()) - prefix
            && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
        {
            suffix += 1;
        }
        let x = &a[prefix..a.len() - suffix];
        let y = &b[prefix..b.len() - suffix];
        let width = y.len() + 1;
        let mut equal = Vec::new();
        if (x.len() + 1).saturating_mul(width) <= 2_000_000 {
            let mut table = vec![0u32; (x.len() + 1) * width];
            for i in (0..x.len()).rev() {
                for j in (0..y.len()).rev() {
                    table[i * width + j] = if x[i] == y[j] {
                        1 + table[(i + 1) * width + j + 1]
                    } else {
                        table[(i + 1) * width + j].max(table[i * width + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < x.len() && j < y.len() {
                if x[i] == y[j] {
                    equal.push((i, j));
                    i += 1;
                    j += 1;
                } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        equal.push((x.len(), y.len()));
        let (mut ai, mut bi) = (0, 0);
        let mut offset = a[..prefix].iter().map(|s| s.len()).sum::<usize>();
        let mut hunks = Vec::new();
        for (i, j) in equal {
            let end = offset + x[ai..i].iter().map(|s| s.len()).sum::<usize>();
            if i > ai || j > bi {
                hunks.push(Hunk {
                    id: format!("h{}", hunks.len()),
                    start: offset,
                    end,
                    after: y[bi..j].concat(),
                    status: "pending".into(),
                    operation: None,
                    receipt: None,
                });
            }
            offset = end + x.get(i).map_or(0, |s| s.len());
            ai = i + 1;
            bi = j + 1;
        }
        Self {
            version: p.expected_version.clone(),
            hunks,
        }
    }
    fn body(&self, original: &str, accepting: Option<usize>) -> String {
        let mut result = String::new();
        let mut offset = 0;
        for (i, h) in self.hunks.iter().enumerate() {
            result.push_str(&original[offset..h.start]);
            result.push_str(if h.status == "accepted" || accepting == Some(i) {
                &h.after
            } else {
                &original[h.start..h.end]
            });
            offset = h.end;
        }
        result.push_str(&original[offset..]);
        result
    }
    pub(super) fn progress(&self, p: &Proposal) -> Value {
        let accepted = self.hunks.iter().filter(|h| h.status == "accepted").count();
        let rejected = self.hunks.iter().filter(|h| h.status == "rejected").count();
        json!({"persisted":false,"partialPersisted":accepted > 0,"proposalPersisted":true,"proposalId":p.id,"courseId":p.course_id,"sessionId":p.session_id,"path":p.path,"status":p.status,"version":self.version,"acceptedCount":accepted,"rejectedCount":rejected,"pendingCount":self.hunks.len()-accepted-rejected,"message":"建议仍待审阅，仅已接受的部分已保存；不得将剩余建议视为已生效。"})
    }
    pub(super) fn view(&self, original: &str) -> Value {
        let mut offset = 0;
        let hunks:Vec<_> = self.hunks.iter().map(|h| { let value=json!({"id":h.id,"prefix":&original[offset..h.start],"before":&original[h.start..h.end],"after":h.after,"status":h.status}); offset=h.end; value }).collect();
        json!({"version":self.version,"hunks":hunks,"tail":&original[offset..]})
    }
}
pub(super) fn prepare(core: &Core, p: &mut Proposal) -> Result<()> {
    if p.exercise.is_none() && p.text_review.is_none() && p.status == "pending" {
        // A legacy accepting record normalized for a pre-intent retry must
        // retain the original recovery protocol, not be migrated on read.
        let stored: Value = serde_json::from_slice(&crate::assets::bounded_read(
            &proposal_path(core, &p.course_id, &p.id)?,
            256 * 1024,
        )?)?;
        if stored["status"] != "pending" {
            return Ok(());
        }
        let review = Review::new(p);
        if review.hunks.is_empty() {
            return Ok(());
        }
        p.text_review = Some(review);
        crate::course_write_transaction::durable_json(
            &proposal_path(core, &p.course_id, &p.id)?,
            p,
        )?;
    }
    Ok(())
}
pub(crate) fn text_proposal_decide(
    engine: &crate::app::Engine,
    course_id: &str,
    id: &str,
    hunk_id: &str,
    accept: bool,
) -> Result<Value> {
    let authorization = crate::app::UI_PREFERENCE_WRITE
        .lock()
        .map_err(|_| invalid())?;
    let tier = engine.ui_preferences()?.tool_authorization_tier;
    let mut core = engine.lock()?;
    check_course(&mut core, course_id)?;
    let mut p = read_proposal(&core, course_id, id)?;
    prepare(&core, &mut p)?;
    let mut review = p.text_review.clone().ok_or_else(invalid)?;
    let index = review
        .hunks
        .iter()
        .position(|h| h.id == hunk_id)
        .ok_or_else(invalid)?;
    let path = proposal_path(&core, course_id, id)?;
    let original_ledger = workspace_root(&core.state_dir, &p.session_id)
        .join("receipts")
        .join(format!("{id}.json"));
    let status = review.hunks[index].status.clone();
    if matches!(status.as_str(), "accepted" | "rejected") {
        // A repeated decision returns the exact original receipt, never another publication.
        let receipt = review.hunks[index].receipt.clone().ok_or_else(invalid)?;
        if matches!(p.status.as_str(), "accepted" | "rejected") {
            let mut entry = read_ledger(&original_ledger)?;
            entry.result = proposal_receipt(&p);
            crate::course_write_transaction::durable_json(&original_ledger, &entry)?;
        }
        drop(core);
        drop(authorization);
        if matches!(p.status.as_str(), "accepted" | "rejected") {
            settle_decision(engine, &p)?;
        }
        return Ok(receipt);
    }
    if review
        .hunks
        .iter()
        .enumerate()
        .any(|(i, h)| i != index && h.status == "accepting")
    {
        return Err(CoreError::new(
            "NATIVE_TOOL_UNKNOWN",
            "请先核对此前修改的保存结果。",
        ));
    }
    let scope = ToolScope {
        session_id: p.session_id.clone(),
        section_id: p.section_id.clone(),
        run_id: p.run_id.clone(),
        allowed_ids: p.allowed_ids.clone(),
    };
    let mut published = None;
    if status == "accepting" {
        let operation = review.hunks[index].operation.as_ref().ok_or_else(invalid)?;
        let ledger = original_ledger.with_file_name(format!("{operation}.json"));
        let args = json!({"path":p.path,"content":review.body(&p.original,Some(index)),"expectedVersion":review.version});
        if let Ok(entry) = read_ledger(&ledger) {
            if entry.name != "write"
                || entry.arguments_hash != hash(&serde_json::to_string(&args)?)
                || entry.result["persisted"] != true
            {
                return Err(conflict());
            }
            published = Some(entry.result);
        } else if !ledger.with_extension("pending").exists()
            && matches_pending_ledger(&p, &read_ledger(&original_ledger)?)?
        {
            review.hunks[index].status = "pending".into();
            p.text_review = Some(review);
            crate::course_write_transaction::durable_json(&path, &p)?;
            return Err(CoreError::new(
                "NOT_PUBLISHED",
                "已核实此处尚未发布，可以重新选择；未自动重放。",
            ));
        } else {
            // A transaction's durable publication marker plus exact current bytes
            // can close the publication -> receipt interruption without replaying.
            let doc = document(&core, &scope, course_id, &p.path)?.ok_or_else(out_of_scope)?;
            let state = crate::course_write_transaction::inspect(
                core.content.root(),
                &doc.path,
                operation,
            )?;
            if state["published.json"]["exists"] == true
                && state["new"]["hash"] == state["target"]["hash"]
                && doc.body == review.body(&p.original, Some(index))
            {
                let result = json!({"persisted":true,"sessionId":p.session_id,"courseId":course_id,"path":p.path,"version":doc.version});
                let entry = LedgerEntry {
                    name: "write".into(),
                    arguments_hash: hash(&serde_json::to_string(&args)?),
                    result: result.clone(),
                    created_at: timestamp(),
                };
                crate::course_write_transaction::durable_json(&ledger, &entry)?;
                published = Some(result);
            } else {
                drop(core);
                drop(authorization);
                return Err(course_write_unknown(engine, &p.session_id, &ledger));
            }
        }
    } else if accept {
        if tier_rank(&tier) < 2 {
            return Err(not_allowed());
        }
        if !matches_pending_ledger(&p, &read_ledger(&original_ledger)?)? {
            return Err(conflict());
        }
        if course(&mut core, &scope)?.as_deref() != Some(course_id) {
            return Err(out_of_scope());
        }
        if let Some(doc) = document(&core, &scope, course_id, &p.path)? {
            if engine
                .drafts
                .get(&format!("section:{}", doc.id))?
                .is_some_and(|d| d.data.body != doc.body)
            {
                return Err(CoreError::new(
                    "CONFLICT",
                    "当前文件有未保存草稿，请先处理；建议仍保留。",
                ));
            }
        }
        let content = review.body(&p.original, Some(index));
        let args = WriteArgs {
            path: p.path.clone(),
            content,
            expected_version: review.version.clone(),
        };
        let arguments_hash = hash(&serde_json::to_string(
            &json!({"path":args.path,"content":args.content,"expectedVersion":args.expected_version}),
        )?);
        let operation = format!(
            "textedit_{}",
            &hash(&format!("{id}:{hunk_id}:{arguments_hash}"))[..32]
        );
        let ledger = original_ledger.with_file_name(format!("{operation}.json"));
        review.hunks[index].operation = Some(operation);
        review.hunks[index].status = "accepting".into();
        p.text_review = Some(review.clone());
        crate::course_write_transaction::durable_json(&path, &p)?;
        match course_write_locked(&mut core, &scope, &args, &ledger, None) {
            Ok(Some(result)) if result["persisted"] == true => {
                let entry = LedgerEntry {
                    name: "write".into(),
                    arguments_hash,
                    result: result.clone(),
                    created_at: timestamp(),
                };
                crate::course_write_transaction::durable_json(&ledger, &entry)?;
                published = Some(result);
            }
            outcome => {
                if !ledger.with_extension("pending").exists() {
                    review.hunks[index].status = "pending".into();
                    p.text_review = Some(review);
                    crate::course_write_transaction::durable_json(&path, &p)?;
                }
                return Err(outcome.err().unwrap_or_else(conflict));
            }
        }
    }
    let persisted = published.is_some();
    if let Some(result) = published {
        review.version = result["version"].as_str().map(str::to_owned);
    }
    review.hunks[index].status = if persisted { "accepted" } else { "rejected" }.into();
    let complete = review
        .hunks
        .iter()
        .all(|h| matches!(h.status.as_str(), "accepted" | "rejected"));
    let receipt = json!({"persisted":persisted,"status":review.hunks[index].status,"complete":complete,"version":review.version});
    review.hunks[index].receipt = Some(receipt.clone());
    p.text_review = Some(review.clone());
    if complete {
        let any = review.hunks.iter().any(|h| h.status == "accepted");
        p.status = if any { "accepted" } else { "rejected" }.into();
        p.receipt = Some(
            json!({"persisted":any,"status":p.status,"proposalId":id,"courseId":course_id,"sessionId":p.session_id,"path":p.path,"version":review.version,"acceptedCount":review.hunks.iter().filter(|h|h.status=="accepted").count(),"rejectedCount":review.hunks.iter().filter(|h|h.status=="rejected").count(),"message":"审阅已完成，仅接受的修改已保存；拒绝的修改未应用。","hunkDecisions":review.hunks.iter().map(|h|json!({"id":h.id,"status":h.status})).collect::<Vec<_>>()}),
        );
    }
    // Persist decisions before settling the original ledger, including pure rejection.
    crate::course_write_transaction::durable_json(&path, &p)?;
    if complete {
        let mut entry = read_ledger(&original_ledger)?;
        entry.result = proposal_receipt(&p);
        crate::course_write_transaction::durable_json(&original_ledger, &entry)?;
    }
    drop(core);
    drop(authorization);
    if complete {
        settle_decision(engine, &p)?;
    }
    Ok(receipt)
}

// Compatibility for existing whole-proposal callers, using the same decisions.
pub(super) fn decide_all(
    engine: &crate::app::Engine,
    course: &str,
    id: &str,
    accept: bool,
) -> Result<Value> {
    let p = read_proposal(&*engine.lock()?, course, id)?;
    let review = p.text_review.ok_or_else(invalid)?;
    for h in review
        .hunks
        .iter()
        .filter(|h| matches!(h.status.as_str(), "pending" | "accepting"))
    {
        text_proposal_decide(engine, course, id, &h.id, accept)?;
    }
    let p = read_proposal(&*engine.lock()?, course, id)?;
    Ok(proposal_receipt(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn review(before: &str, after: &str) -> Review {
        let p:Proposal=serde_json::from_value(json!({"id":"p","courseId":"c","sessionId":"s","sectionId":"d","runId":"r","allowedIds":[],"path":"documents/d.md","original":before,"content":after,"status":"pending"})).unwrap();
        Review::new(&p)
    }
    #[test]
    fn canonical_insert_delete_replace_unicode_crlf_and_no_final_newline() {
        for (before, after) in [
            ("", "新增😀"),
            ("删除\r\n", ""),
            ("一\n\n不变\n\n二", "新一\n\n不变\n\n新二"),
            ("```js\n\nold()\n```\n\n尾\n", "```js\n\nnew()\n```\n\n尾\n"),
        ] {
            let mut r = review(before, after);
            assert_eq!(r.body(before, None), before);
            for h in &mut r.hunks {
                h.status = "accepted".into();
            }
            assert_eq!(r.body(before, None), after);
            let view = r.view(before);
            let chunks = view["hunks"].as_array().unwrap();
            assert_eq!(
                chunks
                    .iter()
                    .map(|h| format!(
                        "{}{}",
                        h["prefix"].as_str().unwrap(),
                        h["before"].as_str().unwrap()
                    ))
                    .collect::<String>()
                    + view["tail"].as_str().unwrap(),
                before
            );
        }
        assert_eq!(review("a\n\nkeep\n\nz", "A\n\nkeep\n\nZ").hunks.len(), 2);
        assert_eq!(review("same", "same").hunks.len(), 0);
        assert_eq!(
            review("```\n\na\n\nkeep\n\nb\n```", "```\n\nA\n\nkeep\n\nB\n```")
                .hunks
                .len(),
            1
        );
    }
}
