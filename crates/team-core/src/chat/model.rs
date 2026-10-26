use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub adapter_id: String,
    pub peer_id: String,
    pub agent_id: String,
    pub title: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub revision: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Human,
    Agent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Queued,
    Sent,
    Failed,
    Received,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub interface: Option<crate::interfaces::InterfaceInfo>,
    pub id: i64,
    pub conversation_id: String,
    pub direction: Direction,
    pub text: String,
    pub kind: String,
    pub payload: Option<Value>,
    pub mailbox_id: Option<i64>,
    pub external_id: Option<String>,
    pub status: DeliveryStatus,
    pub error: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboundMessage {
    pub adapter_id: String,
    pub peer_id: String,
    pub sender_id: String,
    pub agent_id: String,
    pub external_id: String,
    pub text: String,
    pub kind: String,
    pub payload: Option<Value>,
    pub title: Option<String>,
    pub reply_to_external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Delivery {
    pub id: i64,
    pub conversation_id: String,
    pub adapter_id: String,
    pub peer_id: String,
    pub message: ChatMessage,
    pub reply_to_external_id: Option<String>,
    pub attempts: u32,
}
