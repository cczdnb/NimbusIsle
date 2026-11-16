//! 勾选完成时尽量写回源系统；失败返回可读中文说明。

use super::adapters::{self, RemoteItem};
use super::Source;
use serde_json::{json, Value};

pub fn complete(source: Source, address: &str, token: &str, item: &RemoteItem) -> Result<String, String> {
    match source {
        Source::Jira => complete_jira(address, token, item),
        Source::Linear => complete_linear(token, item),
        Source::GitHub => complete_github(address, token, item),
        Source::Notion => complete_notion(token, item),
        Source::WindowsTodo => complete_windows(address, item),
        Source::Feishu => Ok("已在本地标记完成（飞书为只读同步，请在飞书侧关闭）".into()),
        Source::CustomJson => Ok("已在本地标记完成（自建 JSON 不支持写回）".into()),
    }
}

fn complete_jira(base: &str, token: &str, item: &RemoteItem) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() || token.trim().is_empty() { return Err("请先配置 Jira URL / Token".into()); }
    let auth = adapters::jira_authorization(token)?;
    let key = item.key.trim();
    if key.is_empty() { return Err("缺少 Jira 议题编号".into()); }
    let url = format!("{base}/rest/api/2/issue/{key}/transitions");
    let doc = adapters::http_get(&url, &[("Authorization", &auth), ("Accept", "application/json")])?;
    let arr = doc.get("transitions").and_then(|v| v.as_array()).ok_or("未找到可用流转")?;
    let mut chosen: Option<String> = None;
    let keywords = ["完成", "done", "closed", "resolved", "关闭", "已完成", "resolve", "complete"];
    for tr in arr {
        let name = tr.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let to = tr.pointer("/to/name").and_then(|v| v.as_str()).unwrap_or("");
        let id = tr.get("id").and_then(|v| v.as_str()).or_else(|| tr.get("id").and_then(|v| v.as_i64().map(|n| {
            // leak-free: handle below
            None::<&str>
        }).flatten()));
        let id = tr.get("id").map(|v| {
            if let Some(s) = v.as_str() { s.to_string() } else { v.to_string().trim_matches('"').to_string() }
        }).unwrap_or_default();
        let hay = format!("{name} {to}").to_ascii_lowercase();
        let hit = keywords.iter().any(|k| name.contains(k) || to.contains(k) || hay.contains(&k.to_ascii_lowercase()));
        if hit && !id.is_empty() { chosen = Some(id); break; }
        let _ = id;
    }
    // 再扫一遍 to 状态类别
    if chosen.is_none() {
        for tr in arr {
            let cat = tr.pointer("/to/statusCategory/key").and_then(|v| v.as_str()).unwrap_or("");
            if cat.eq_ignore_ascii_case("done") {
                if let Some(id) = tr.get("id").map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())) {
                    chosen = Some(id.trim_matches('"').to_string());
                    break;
                }
            }
        }
    }
    let tid = chosen.ok_or("未找到「完成」类流转，请在 Jira 手动关闭")?;
    let body = json!({ "transition": { "id": tid } }).to_string();
    adapters::http_post(&format!("{base}/rest/api/2/issue/{key}/transitions"), &[
        ("Authorization", &auth),
        ("Accept", "application/json"),
    ], &body)?;
    Ok(format!("已写回 Jira · {key}"))
}

fn complete_linear(token: &str, item: &RemoteItem) -> Result<String, String> {
    let token = token.trim().strip_prefix("Bearer ").unwrap_or(token.trim());
    if token.is_empty() { return Err("请先填写 Linear API Key".into()); }
    let ident = item.ext_id.as_deref().filter(|s| !s.is_empty()).unwrap_or(item.key.trim());
    if ident.is_empty() { return Err("缺少 Linear issue identifier".into()); }
    // Linear 的 issue(id) 也接受 ENG-123 这种 identifier。
    let q1 = format!(
        r#"{{ "query": "query($id: String!) {{ issue(id: $id) {{ id team {{ id }} }} }}", "variables": {{ "id": {} }} }}"#,
        Value::String(ident.to_string())
    );
    let doc1 = adapters::http_post(
        "https://api.linear.app/graphql",
        &[("Authorization", token), ("Accept", "application/json")],
        &q1,
    )?;
    if let Some(errs) = doc1.get("errors").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        let msg = errs[0].get("message").and_then(|v| v.as_str()).unwrap_or("查询失败");
        return Err(format!("Linear 查询失败：{}", adapters::shorten(msg)));
    }
    let issue = doc1.pointer("/data/issue").ok_or("Linear 未找到 Issue")?;
    let uuid = issue.get("id").and_then(|v| v.as_str()).ok_or("Linear Issue 无 id")?;
    let team_id = issue.pointer("/team/id").and_then(|v| v.as_str()).unwrap_or("");

    // 找 completed 工作流状态
    let mut state_id = String::new();
    if !team_id.is_empty() {
        let q2 = format!(
            r#"{{ "query": "query($tid: String!) {{ team(id: $tid) {{ states: workflowStates {{ nodes {{ id name type }} }} }} }}", "variables": {{ "tid": {} }} }}"#,
            Value::String(team_id.to_string())
        );
        if let Ok(doc2) = adapters::http_post(
            "https://api.linear.app/graphql",
            &[("Authorization", token), ("Accept", "application/json")],
            &q2,
        ) {
            if let Some(nodes) = doc2.pointer("/data/team/states/nodes").and_then(|v| v.as_array()) {
                for n in nodes {
                    let ty = n.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if ty.eq_ignore_ascii_case("completed") {
                        if let Some(id) = n.get("id").and_then(|v| v.as_str()) { state_id = id.to_string(); break; }
                    }
                }
            }
        }
    }
    if state_id.is_empty() { return Err("未找到 Linear completed 状态（需 API Key 可读 workflowStates）".into()); }
    let q3 = format!(
        r#"{{ "query": "mutation($id: String!, $sid: String!) {{ issueUpdate(id: $id, input: {{ stateId: $sid }}) {{ success }} }}", "variables": {{ "id": {}, "sid": {} }} }}"#,
        Value::String(uuid.to_string()), Value::String(state_id)
    );
    let doc3 = adapters::http_post(
        "https://api.linear.app/graphql",
        &[("Authorization", token), ("Accept", "application/json")],
        &q3,
    )?;
    if let Some(errs) = doc3.get("errors").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        let msg = errs[0].get("message").and_then(|v| v.as_str()).unwrap_or("写回失败");
        return Err(format!("Linear 写回失败：{}", adapters::shorten(msg)));
    }
    Ok(format!("已写回 Linear · {ident}"))
}

fn complete_github(address: &str, token: &str, item: &RemoteItem) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() { return Err("请先填写 GitHub PAT（需 issues 写权限）".into()); }
    let (owner, repo) = if let Some(list) = item.list_id.as_deref().filter(|s| s.contains('/')) {
        let (o, r) = list.split_once('/').unwrap();
        (o.to_string(), r.to_string())
    } else {
        adapters::try_parse_github_repo(address).ok_or("无法解析 GitHub 仓库")?
    };
    let number = item.ext_id.as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| item.key.rsplit('#').next().and_then(|s| s.parse().ok()))
        .ok_or("无法解析 GitHub Issue 编号")?;
    let t = token.strip_prefix("Bearer ").unwrap_or(token);
    let url = format!("https://api.github.com/repos/{}/{}/issues/{number}", adapters::encode(&owner), adapters::encode(&repo));
    adapters::http_patch(&url, &[
        ("Authorization", &format!("Bearer {t}")),
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ], "{\"state\":\"closed\"}")?;
    Ok(format!("已写回 GitHub · {}", item.key))
}

fn complete_notion(token: &str, item: &RemoteItem) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() { return Err("请先填写 Notion Integration Token".into()); }
    let Some(id) = item.ext_id.as_deref().filter(|s| !s.is_empty()) else {
        return Ok("已在本地完成（缺少 Notion page id，未写回）".into());
    };
    let t = token.strip_prefix("Bearer ").unwrap_or(token);
    let url = format!("https://api.notion.com/v1/pages/{id}");
    let headers = [
        ("Authorization", format!("Bearer {t}")),
        ("Notion-Version", "2022-06-28".into()),
    ];
    // headers lifetime: use two calls with string refs
    let try_payload = |body: &str| -> Result<(), String> {
        adapters::http_patch(&url, &[
            ("Authorization", &format!("Bearer {t}")),
            ("Notion-Version", "2022-06-28"),
        ], body).map(|_| ())
    };
    if try_payload("{\"properties\":{\"Status\":{\"status\":{\"name\":\"Done\"}}}}").is_ok() {
        return Ok("已写回 Notion（Status=Done）".into());
    }
    match try_payload("{\"properties\":{\"状态\":{\"status\":{\"name\":\"完成\"}}}}") {
        Ok(()) => Ok("已写回 Notion（状态=完成）".into()),
        Err(e) => Err(format!("Notion 写回失败（请确认属性名 Status/状态 与 Done/完成 存在）：{}", adapters::shorten(&e))),
    }
}

fn complete_windows(client_id: &str, item: &RemoteItem) -> Result<String, String> {
    let token = super::windows_auth::acquire_token(client_id, false)?;
    let list = item.list_id.as_deref().ok_or("缺少 Windows 待办列表 ID")?;
    let id = item.ext_id.as_deref().ok_or("缺少 Windows 待办任务 ID")?;
    let url = format!(
        "https://graph.microsoft.com/v1.0/me/todo/lists/{}/tasks/{}",
        adapters::encode(list), adapters::encode(id)
    );
    adapters::http_patch(&url, &[
        ("Authorization", &format!("Bearer {token}")),
        ("Accept", "application/json"),
    ], "{\"status\":\"completed\"}")?;
    Ok(format!("已写回 Windows 待办 · {}", item.key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_and_json_are_local_only() {
        let item = RemoteItem {
            key: "X".into(), summary: "t".into(), due: None, status: None, url: None,
            priority: 2, ext_id: None, list_id: None, source: Source::Feishu,
            bucket: adapters::Bucket::Unscheduled, done: false, updated_at: None,
        };
        assert!(complete(Source::Feishu, "", "", &item).unwrap().contains("只读"));
        assert!(complete(Source::CustomJson, "", "", &item).unwrap().contains("不支持写回"));
    }
}
