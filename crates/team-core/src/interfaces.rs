//! Named human-facing interface instances and explicit agent grants.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{chat, compose::Compose};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterfaceInfo {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
}

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS interface_instances (
    id TEXT PRIMARY KEY, provider TEXT NOT NULL, name TEXT NOT NULL,
    description TEXT NOT NULL, capabilities_json TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS agent_interfaces (
    agent_id TEXT NOT NULL, interface_id TEXT NOT NULL REFERENCES interface_instances(id),
    PRIMARY KEY(agent_id,interface_id)
);
"#;

pub fn register(conn: &Connection, info: &InterfaceInfo, agents: &[String]) -> chat::Result<()> {
    if info.id.is_empty() || info.id.len() > 128 || info.id.chars().any(char::is_control) {
        return Err(chat::Error::InvalidInput("invalid interface ID".into()));
    }
    conn.execute("INSERT INTO interface_instances(id,provider,name,description,capabilities_json) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET provider=excluded.provider,name=excluded.name,description=excluded.description,capabilities_json=excluded.capabilities_json,active=1",params![info.id,info.provider,info.name,info.description,serde_json::to_string(&info.capabilities)?])?;
    for agent in agents {
        conn.execute(
            "INSERT OR IGNORE INTO agent_interfaces(agent_id,interface_id) VALUES(?1,?2)",
            params![agent, info.id],
        )?;
    }
    chat::Repository::new(conn).register_capabilities(&info.id, &info.capabilities)?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> chat::Result<Option<InterfaceInfo>> {
    let row: Option<(String,String,String,String,String)> = conn.query_row("SELECT provider,id,name,description,capabilities_json FROM interface_instances WHERE id=?1 AND active=1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    row.map(|(provider, id, name, description, caps)| {
        Ok(InterfaceInfo {
            provider,
            id,
            name,
            description,
            capabilities: serde_json::from_str(&caps)?,
        })
    })
    .transpose()
}

pub fn list_for_agent(conn: &Connection, agent: &str) -> chat::Result<Vec<InterfaceInfo>> {
    let mut stmt=conn.prepare("SELECT i.id FROM interface_instances i JOIN agent_interfaces a ON a.interface_id=i.id WHERE a.agent_id=?1 AND i.active=1 ORDER BY i.provider,i.id")?;
    let ids = stmt
        .query_map([agent], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter()
        .filter_map(|id| get(conn, id).transpose())
        .collect()
}

pub fn is_allowed(conn: &Connection, agent: &str, interface: &str) -> chat::Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM agent_interfaces a JOIN interface_instances i ON i.id=a.interface_id WHERE a.agent_id=?1 AND i.id=?2 AND i.active=1",params![agent,interface],|_|Ok(())).optional()?.is_some())
}

/// Apply configured grants atomically; cached historical provenance stays in inbox rows.
pub fn sync(conn: &Connection, compose: &Compose) -> chat::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE interface_instances SET active=0", [])?;
    tx.execute("DELETE FROM agent_interfaces", [])?;
    let all: Vec<_> = compose.agents().map(|a| a.id()).collect();
    register(
        &tx,
        &InterfaceInfo {
            provider: "web".into(),
            id: "web".into(),
            name: "Web workspace".into(),
            description: "Your private teamctl workspace in the browser".into(),
            capabilities: [
                "text", "markdown", "image", "file", "audio", "voice", "html", "typing",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        &all,
    )?;
    for interface in &compose.global.interfaces {
        let mut agents: Vec<_> = compose
            .agents()
            .filter(|a| {
                a.spec
                    .interfaces
                    .as_ref()
                    .is_some_and(|i| i.instances.iter().any(|id| id == interface.instance_id()))
            })
            .map(|a| a.id())
            .collect();
        if let Some(agent) = interface.agent() {
            if !agents.contains(&agent) {
                agents.push(agent);
            }
        }
        let capabilities = match interface.r#type.as_str() {
            "web" => vec![
                "text", "markdown", "image", "file", "audio", "voice", "html", "typing",
            ],
            "telegram" => vec!["text", "image", "file", "typing", "reaction"],
            _ => vec!["text"],
        }
        .into_iter()
        .map(str::to_string)
        .collect();
        register(
            &tx,
            &InterfaceInfo {
                provider: interface.r#type.clone(),
                id: interface.instance_id().into(),
                name: interface.name.clone(),
                description: interface.description.clone(),
                capabilities,
            },
            &agents,
        )?;
    }
    for a in compose.agents().filter(|a| a.spec.telegram().is_some()) {
        register(
            &tx,
            &InterfaceInfo {
                provider: "telegram".into(),
                id: format!("telegram:{}", a.id()),
                name: a
                    .spec
                    .display_name
                    .clone()
                    .unwrap_or_else(|| a.agent.into()),
                description: "Direct Telegram conversation".into(),
                capabilities: ["text", "image", "file", "typing", "reaction"]
                    .map(str::to_string)
                    .to_vec(),
            },
            &[a.id()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Registered instances retain their identity after removal so stale routes cannot bypass grants.
pub fn require_access(conn: &Connection, agent: &str, interface: &str) -> chat::Result<()> {
    let known = conn
        .query_row(
            "SELECT 1 FROM interface_instances WHERE id=?1",
            [interface],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if known && !is_allowed(conn, agent, interface)? {
        return Err(chat::Error::Forbidden);
    }
    Ok(())
}
