use super::repository::{content, fingerprint, identity, now};
use super::{ChatMessage, Conversation, DeliveryStatus, Direction, Error, Repository, Result};
use rusqlite::params;
use serde_json::Value;

impl Repository<'_> {
    pub fn open_interface(
        &self,
        agent: &str,
        interface: &str,
        peer: &str,
        title: &str,
    ) -> Result<Conversation> {
        self.atomic(|| {
            self.require_agent(agent)?;
            if !crate::interfaces::is_allowed(self.conn, agent, interface)? {
                return Err(Error::Forbidden);
            }
            identity("peer ID", peer)?;
            self.conversation(interface, peer, agent, title)
        })
    }

    /// Browser input uses the same durable transcript and idempotency policy as adapters.
    pub fn send_rich(
        &self,
        conversation: &str,
        text: &str,
        kind: &str,
        payload: Option<Value>,
        request: &str,
    ) -> Result<ChatMessage> {
        identity("request ID", request)?;
        content(text, kind, &payload)?;
        self.atomic(|| {
            let route=self.get_conversation(conversation)?;
            let info=crate::interfaces::get(self.conn,&route.adapter_id)?.ok_or(Error::Forbidden)?;
            if info.provider!="web" || !crate::interfaces::is_allowed(self.conn,&route.agent_id,&route.adapter_id)? { return Err(Error::Forbidden); }
            let caller=format!("interface:{}:{}",route.adapter_id,route.peer_id);
            let fingerprint=fingerprint(&serde_json::json!([conversation,text,kind,payload]));
            if let Some(message)=self.request(&caller,request,&fingerprint)? {return Ok(message);}
            let project=self.require_agent(&route.agent_id)?;
            let timestamp=now();
            let mut inbox_text=text.to_string();
            if let Some(assets)=payload.as_ref().and_then(|p|p["attachments"].as_array()) {
                for path in assets.iter().filter_map(|a|a["path"].as_str()) {
                    inbox_text.push_str("\n📎 attachment: ");
                    inbox_text.push_str(path);
                }
            }
            self.conn.execute("INSERT INTO messages(project_id,sender,recipient,text,kind,structured_payload,sent_at) VALUES(?1,'user:web',?2,?3,?4,?5,?6)",params![project,route.agent_id,inbox_text,kind,payload.as_ref().map(serde_json::to_string).transpose()?,timestamp])?;
            let mailbox=self.conn.last_insert_rowid();
            let message=self.insert_message(&route.id,Direction::Human,text,kind,payload.as_ref(),Some(mailbox),None,None,DeliveryStatus::Received,timestamp)?;
            self.link_mailbox(mailbox,&route,message.id,None)?;
            self.save_request(&caller,request,&fingerprint,message.id)?;
            Ok(message)
        })
    }
}
