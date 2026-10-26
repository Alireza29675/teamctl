use super::*;
use rusqlite::params;
use team_core::chat::{ChatMessage, Repository};

pub(super) fn prepare(root: &Path) -> Result<()> {
    let manifest = root.join("team-compose.yaml");
    if manifest.exists() && !root.join(".web-demo").exists() {
        bail!(
            "--demo requires an empty disposable --root directory; it never modifies a real team"
        );
    }
    std::fs::create_dir_all(root.join("projects"))?;
    if !manifest.exists() {
        std::fs::write(&manifest,"version: \"2.0.0\"\nprojects:\n  - file: projects/personal.yaml\nweb:\n  cache_retention_days: 30\n")?;
        std::fs::write(root.join("projects/personal.yaml"),"version: 1\nproject:\n  id: personal\n  name: Personal workspace\n  cwd: ..\nchannels:\n  - name: notes\n    members: '*'\nagents:\n  companion:\n    display_name: Companion\n    runtime: codex\n    autonomy: autonomous\n    can_broadcast: [notes]\n  scout:\n    display_name: Scout\n    runtime: claude-code\n    autonomy: autonomous\n    can_broadcast: [notes]\n    reports_to: companion\n")?;
        std::fs::write(
            root.join(".web-demo"),
            "Disposable web fixture. No model providers are started.\n",
        )?;
    }
    Ok(())
}
pub(super) fn seed(state: &Workspace, c: &rusqlite::Connection) -> Result<()> {
    let repo = Repository::new(c);
    for (id,greeting) in [("personal:companion","Welcome to your workspace. I can help keep your ideas, projects, and open loops in one place.\n\nSend a message, attach an image, or open **Inspect** to explore my context."),("personal:scout","I’m ready to research alongside you. This is a local demonstration; no model provider is connected.")] {
        let conv=repo.open_interface(id,"web","operator",id)?;
        if repo.history(&conv.id,None,None,1)?.is_empty(){repo.reply(id,Some(&conv.id),None,greeting,"text",None,Some(&format!("demo-welcome:{id}")))?;}
    }
    let conv = repo.open_interface("personal:companion", "web", "operator", "Companion")?;
    if repo.history(&conv.id, None, None, 10)?.len() == 1 {
        repo.reply("personal:companion",Some(&conv.id),None,"Here’s a little focus counter. Interactive components stay isolated from your workspace.","html",Some(json!({"html":"<!doctype html><html><body style='margin:0;padding:24px;font-family:system-ui;background:#f4f2ed;color:#292b25'><h3 style='margin-top:0'>One thing at a time.</h3><p>Choose how many minutes you want to focus.</p><button id='less'>−</button> <strong id='minutes'>25</strong> minutes <button id='more'>+</button><script>let n=25;document.getElementById('less').onclick=()=>document.getElementById('minutes').textContent=n=Math.max(5,n-5);document.getElementById('more').onclick=()=>document.getElementById('minutes').textContent=n+=5;</script></body></html>"})),Some("demo-component"))?;
    }
    let count: i64 = c.query_row("SELECT COUNT(*) FROM approvals", [], |r| r.get(0))?;
    if count == 0 {
        c.execute("INSERT INTO approvals(project_id,agent_id,action,summary,status,requested_at,expires_at) VALUES('personal','personal:companion','organize','May I organize the demo inbox into project notes?','pending',?1,?2)",params![now(),now()+86400.0])?;
    }
    let _ = &state;
    Ok(())
}
pub(super) fn respond(
    c: &rusqlite::Connection,
    agent: &str,
    conversation: &str,
    message: &ChatMessage,
) -> ApiResult<()> {
    let repo = Repository::new(c);
    repo.reply(agent,Some(conversation),message.mailbox_id,"Received. **This is the local demo**, so I’m showing the message round trip without calling a model.\n\nYour message and attachments are saved in this workspace.","text",None,Some(&format!("demo-reply:{}",message.id)))?;
    Ok(())
}
