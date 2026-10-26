use super::*;
use axum::extract::{Path, Query, State};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use team_core::{
    chat::{ChatMessage, Direction, Repository},
    compose::Compose,
    supervisor::{AgentSpec, AgentState, AgentSupervisor, Supervisor},
};

pub(super) async fn state(State(state): State<Shared>) -> ApiResult<Json<Value>> {
    blocking(move || {
        let compose=Compose::load_read_only(&state.root).map_err(ApiError::from)?;
        let c=team_core::chat::open(&state.db)?;
        let mut agents=Vec::new();
        for h in compose.agents() {
            let id=h.id();
            let spec=AgentSpec::from_handle(h,&state.root,&compose.global.supervisor.tmux_prefix);
            let running=AgentSupervisor.state(&spec).unwrap_or(AgentState::Unknown);
            let health=std::fs::read_to_string(team_core::supervisor::native_log_path(&spec).with_extension("status.json")).ok().and_then(|s|serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
            let observed=if state.demo {"idle"} else if running==AgentState::Running {health["state"].as_str().filter(|s|matches!(*s,"idle"|"busy"|"error")).unwrap_or("running")} else {match running {AgentState::Running=>"running",AgentState::Stopped=>"stopped",AgentState::Unknown=>"unknown"}};
            let cwd=compose.projects.iter().find(|p|p.project.id==h.project).map(|p|state.root.join(&p.project.cwd).display().to_string());
            let interfaces=team_core::interfaces::list_for_agent(&c,&id)?;
            let inbox:i64=c.query_row("SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acked_at IS NULL",[&id],|r|r.get(0))?;
            let last:Option<String>=c.query_row("SELECT text FROM chat_messages WHERE conversation_id IN (SELECT id FROM chat_conversations WHERE agent_id=?1) ORDER BY id DESC LIMIT 1",[&id],|r|r.get(0)).optional()?;
            let mut stmt=c.prepare("SELECT id,sender,text,sent_at,kind FROM messages WHERE recipient=?1 AND acked_at IS NULL ORDER BY id DESC LIMIT 20")?;
            let pending=stmt.query_map([&id],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"sender":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?,"created_at":r.get::<_,f64>(3)?,"kind":r.get::<_,Option<String>>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            agents.push(json!({"id":id,"name":h.spec.display_name.as_deref().unwrap_or(h.agent),"description":h.spec.role_prompt.as_ref().map(|v|v.paths().iter().map(|p|p.display().to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),"provider":h.spec.runtime,"runtime":h.spec.runtime,"status":observed,"health":health,"cwd":cwd,"execution":h.spec.execution,"model":h.spec.model,"interfaces":interfaces,"tools":["dm","broadcast","inbox_read","list_interfaces","reply_to_user"],"mcp":h.spec.mcps.keys().collect::<Vec<_>>(),"reports_to":h.spec.reports_to,"autonomy":h.spec.autonomy,"can_dm":h.spec.can_dm,"can_broadcast":h.spec.can_broadcast,"inbox_count":inbox,"unread_mail":inbox,"inbox":pending,"last_message":last,"session":spec.tmux_session}));
        }
        let channels=compose.projects.iter().flat_map(|p|p.channels.iter().map(move |ch|json!({"id":format!("{}:{}",p.project.id,ch.name),"name":ch.name,"project":p.project.id,"members":ch.members}))).collect::<Vec<_>>();
        let mut stmt=c.prepare("SELECT id,agent_id,action,summary,expires_at,options_json,payload_json FROM approvals WHERE status='pending' AND expires_at>?1 ORDER BY id")?;
        let approvals=stmt.query_map([now()],|r|{ let options:Option<String>=r.get(5)?; let payload:Option<String>=r.get(6)?; Ok(json!({"id":r.get::<_,i64>(0)?,"agent":r.get::<_,String>(1)?,"action":r.get::<_,String>(2)?,"summary":r.get::<_,String>(3)?,"expires_at":r.get::<_,f64>(4)?,"options":options.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"payload":payload.and_then(|s|serde_json::from_str::<Value>(&s).ok())}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
        // Reading pending cards confirms they have been offered on a human surface.
        c.execute("UPDATE approvals SET delivered_at=COALESCE(delivered_at,?1) WHERE status='pending' AND expires_at>?1",[now()])?;
        let mut stmt=c.prepare("SELECT agent_id,COALESCE(SUM(usd),0),SUM(input_tok),SUM(output_tok) FROM budget WHERE observed_at>=?1 GROUP BY agent_id")?;
        let budgets=stmt.query_map([(now()/86400.0).floor()*86400.0],|r|Ok(json!({"agent":r.get::<_,Option<String>>(0)?,"usd":r.get::<_,f64>(1)?,"input_tokens":r.get::<_,i64>(2)?,"output_tokens":r.get::<_,i64>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt=c.prepare("SELECT id,sender,recipient,text,sent_at,kind FROM messages ORDER BY id DESC LIMIT 40")?;
        let events=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"sender":r.get::<_,String>(1)?,"recipient":r.get::<_,String>(2)?,"text":r.get::<_,String>(3)?,"created_at":r.get::<_,f64>(4)?,"kind":r.get::<_,Option<String>>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt=c.prepare("SELECT id FROM interface_instances WHERE active=1 ORDER BY id")?;
        let ids=stmt.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let interfaces=ids.iter().map(|id|team_core::interfaces::get(&c,id)).collect::<team_core::chat::Result<Vec<_>>>()?;
        let (count,bytes):(i64,i64)=c.query_row("SELECT COUNT(*),COALESCE(SUM(size),0) FROM web_assets WHERE expired_at IS NULL",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        Ok(Json(json!({"project":{"name":compose.projects.iter().map(|p|p.project.name.as_str()).collect::<Vec<_>>().join(" · "),"root":state.root.display().to_string()},"agents":agents,"channels":channels,"interfaces":interfaces,"approvals":approvals,"budgets":budgets,"events":events,"cache":{"retention_days":state.retention_days,"max_upload_bytes":state.max_upload,"count":count,"bytes":bytes},"demo":state.demo})))
    }).await
}

#[derive(Deserialize)]
pub(super) struct MessageQuery {
    agent: Option<String>,
    channel: Option<String>,
    before: Option<i64>,
    after: Option<i64>,
}
pub(super) async fn messages(
    State(state): State<Shared>,
    Query(q): Query<MessageQuery>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        if q.before.is_some() && q.after.is_some() {return Err(ApiError::bad("Use before or after, not both"));}
        let compose=Compose::load_read_only(&state.root).map_err(ApiError::from)?;
        let c=team_core::chat::open(&state.db)?;
        let order=if q.after.is_some(){"ASC"}else{"DESC"};
        if let Some(channel)=q.channel {
            require_channel(&compose,&channel)?;
            let sql=format!("SELECT id,sender,text,sent_at,interface_json FROM messages WHERE recipient=?1 AND (?2 IS NULL OR id<?2) AND (?3 IS NULL OR id>?3) ORDER BY id {order} LIMIT 200");
            let mut stmt=c.prepare(&sql)?;
            let mut messages=stmt.query_map(params![format!("channel:{channel}"),q.before,q.after],|r|{let sender:String=r.get(1)?;let meta:Option<String>=r.get(4)?;let interface=meta.and_then(|s|serde_json::from_str::<Value>(&s).ok()); Ok(json!({"id":r.get::<_,i64>(0)?,"agent":sender,"role":if sender.starts_with("user:") {"user"} else {"assistant"},"text":r.get::<_,String>(2)?,"created_at":r.get::<_,f64>(3)?,"attachments":[],"interface":interface}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
            if q.after.is_none(){messages.reverse();}
            return Ok(Json(json!({"messages":messages})));
        }
        let agent=q.agent.ok_or_else(||ApiError::bad("Choose an agent or channel"))?;
        require_agent(&compose,&agent)?;
        let repo=Repository::new(&c);
        let sql=format!("SELECT id FROM chat_messages WHERE conversation_id IN (SELECT id FROM chat_conversations WHERE agent_id=?1) AND (?2 IS NULL OR id<?2) AND (?3 IS NULL OR id>?3) ORDER BY id {order} LIMIT 200");
        let mut stmt=c.prepare(&sql)?;
        let mut ids=stmt.query_map(params![agent,q.before,q.after],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if q.after.is_none(){ids.reverse();}
        let mut messages=Vec::new();
        for id in ids { messages.push(message_json(&c,&repo,&repo.message(id)?)?); }
        Ok(Json(json!({"messages":messages})))
    }).await
}

#[derive(Deserialize)]
pub(super) struct Send {
    agent: Option<String>,
    channel: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    attachments: Vec<String>,
    html: Option<String>,
    request_id: Option<String>,
}
pub(super) async fn send(
    State(state): State<Shared>,
    Json(input): Json<Send>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        if input.text.len()>64*1024 || input.html.as_ref().is_some_and(|s|s.len()>64*1024) || input.attachments.len()>10 { return Err(ApiError::bad("Message exceeds workspace limits")); }
        if input.text.trim().is_empty() && input.attachments.is_empty() && input.html.is_none() { return Err(ApiError::bad("Write a message or attach a file")); }
        if input.agent.is_some()==input.channel.is_some() {return Err(ApiError::bad("Choose exactly one agent or channel"));}
        let compose=Compose::load_read_only(&state.root).map_err(ApiError::from)?;
        let c=team_core::chat::open(&state.db)?;
        if let Some(channel)=input.channel {
            if !input.attachments.is_empty() || input.html.is_some() {return Err(ApiError::bad("Channel messages currently support text only"));}
            let project=require_channel(&compose,&channel)?;
            c.execute_batch("CREATE TABLE IF NOT EXISTS web_channel_requests(request_id TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,message_id INTEGER NOT NULL)")?;
            let request=input.request_id.unwrap_or_else(random_id);
            if request.is_empty() || request.len()>128{return Err(ApiError::bad("Invalid request ID"));}
            let fingerprint=blake3::hash(serde_json::to_string(&json!([channel,input.text])).map_err(anyhow::Error::from)?.as_bytes()).to_hex().to_string();
            let tx=rusqlite::Transaction::new_unchecked(&c,rusqlite::TransactionBehavior::Immediate)?;
            let cached:Option<(String,i64)>=tx.query_row("SELECT fingerprint,message_id FROM web_channel_requests WHERE request_id=?1",[&request],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (id,created)=if let Some((previous,id))=cached {
                if previous!=fingerprint{return Err(ApiError(StatusCode::CONFLICT,"Request ID reused with different content".into()));}
                let time:f64=tx.query_row("SELECT sent_at FROM messages WHERE id=?1",[id],|r|r.get(0))?;(id,time)
            }else{
                let time=now();
                tx.execute("INSERT INTO messages(project_id,sender,recipient,text,kind,sent_at,interface_json) VALUES(?1,'user:web',?2,?3,'text',?4,?5)",params![project,format!("channel:{channel}"),input.text,time,serde_json::to_string(&team_core::interfaces::get(&tx,"web")?).map_err(anyhow::Error::from)?])?;
                let id=tx.last_insert_rowid();
                tx.execute("INSERT INTO web_channel_requests(request_id,fingerprint,message_id) VALUES(?1,?2,?3)",params![request,fingerprint,id])?;(id,time)
            };
            tx.commit()?;
            return Ok(Json(json!({"message":{"id":id,"text":input.text,"role":"user","created_at":created}})));
        }
        let agent=input.agent.unwrap_or_default();
        require_agent(&compose,&agent)?;
        let mut attachments=Vec::new();
        for id in &input.attachments { attachments.push(assets::descriptor(&state,&c,id)?); }
        let text=input.text;
        for asset in &mut attachments {
            if let Some(id)=asset["id"].as_str().map(str::to_owned) {asset["path"]=json!(state.assets.join(id).display().to_string());}
        }
        let repo=Repository::new(&c);
        let conv=repo.open_interface(&agent,"web","operator",&agent)?;
        let kind=if input.html.is_some() {"html"} else if attachments.is_empty() {"text"} else {"media"};
        let payload=if kind=="text" {None} else {Some(json!({"attachments":attachments,"html":input.html}))};
        let message=repo.send_rich(&conv.id,&text,kind,payload,&input.request_id.unwrap_or_else(random_id))?;
        if state.demo { demo::respond(&c,&agent,&conv.id,&message)?; }
        Ok(Json(json!({"message":message_json(&c,&repo,&message)?})))
    }).await
}

pub(super) fn message_json(
    _c: &rusqlite::Connection,
    repo: &Repository<'_>,
    message: &ChatMessage,
) -> ApiResult<Value> {
    let conversation = repo.get_conversation(&message.conversation_id)?;
    let interface = message.interface.clone();
    let payload = message.payload.as_ref();
    let attachments = payload
        .and_then(|p| p.get("attachments"))
        .cloned()
        .unwrap_or(json!([]));
    let html = payload.and_then(|p| p.get("html")).cloned();
    Ok(
        json!({"id":message.id,"agent":conversation.agent_id,"conversation_id":conversation.id,"role":if message.direction==Direction::Human {"user"} else {"assistant"},"text":message.text,"created_at":message.created_at,"kind":message.kind,"status":message.status,"error":message.error,"interface":interface,"attachments":attachments,"html":html}),
    )
}

#[derive(Deserialize)]
pub(super) struct Decision {
    decision: String,
    value: Option<String>,
}
pub(super) async fn decide(
    State(state): State<Shared>,
    Path(id): Path<i64>,
    Json(input): Json<Decision>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        let approved=match input.decision.as_str() {"approve"=>true,"reject"=>false,_=>return Err(ApiError::bad("Choose approve or reject"))};
        let c=team_core::chat::open(&state.db)?;
        let options:Option<Option<String>>=c.query_row("SELECT options_json FROM approvals WHERE id=?1 AND status='pending' AND expires_at>?2",params![id,now()],|r|r.get(0)).optional()?;
        let options=options.ok_or_else(||ApiError(StatusCode::CONFLICT,"This approval is no longer pending".into()))?;
        if approved {
            if let Some(options)=options {
                let options:Value=serde_json::from_str(&options).map_err(anyhow::Error::from)?;
                if !options.as_array().is_some_and(|items|items.iter().any(|item|item.get("value").and_then(Value::as_str)==input.value.as_deref())) {return Err(ApiError::bad("Select one of the offered approval choices"));}
            } else if input.value.is_some() {return Err(ApiError::bad("This approval has no choices"));}
        }
        let changed=c.execute("UPDATE approvals SET status=?1,decided_at=?2,decided_by='user:web',decision_value=?3,delivered_at=COALESCE(delivered_at,?2) WHERE id=?4 AND status='pending' AND expires_at>?2",params![if approved {"approved"} else {"denied"},now(),if approved {input.value} else {None},id])?;
        if changed==0 {return Err(ApiError(StatusCode::CONFLICT,"This approval has already been decided".into()));}
        Ok(Json(json!({"ok":true})))
    }).await
}

fn require_agent(compose: &Compose, id: &str) -> ApiResult<()> {
    if compose.agents().any(|a| a.id() == id) {
        Ok(())
    } else {
        Err(ApiError::missing("Unknown agent"))
    }
}
fn require_channel<'a>(compose: &'a Compose, id: &str) -> ApiResult<&'a str> {
    compose
        .projects
        .iter()
        .find_map(|p| {
            p.channels
                .iter()
                .any(|ch| format!("{}:{}", p.project.id, ch.name) == id)
                .then_some(p.project.id.as_str())
        })
        .ok_or_else(|| ApiError::missing("Unknown channel"))
}
