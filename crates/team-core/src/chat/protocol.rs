//! Versioned, bounded JSON-lines contract for out-of-process chat adapters.
//!
//! QR payloads are ephemeral credentials. Their Debug implementation is
//! redacted; hosts must display them only during an explicit foreground pair.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::repository::{content, identity};
use super::{Error, Result, MAX_TEXT_BYTES};

pub const VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Run,
    Pair,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Starting,
    Connecting,
    Connected,
    Disconnected,
    PairingRequired,
    AuthFailed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutboundMessage {
    pub text: String,
    #[serde(default = "text_kind")]
    pub kind: String,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub reply_to_external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostFrame {
    Initialize {
        version: u32,
        instance_id: String,
        state_dir: String,
        mode: Mode,
        config: Value,
    },
    Send {
        request_id: String,
        peer_id: String,
        message: OutboundMessage,
    },
    Accepted {
        peer_id: String,
        external_id: String,
    },
    Rejected {
        peer_id: String,
        external_id: String,
        code: String,
    },
    Shutdown,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterFrame {
    Ready {
        version: u32,
        capabilities: Vec<String>,
        display_name: String,
    },
    Message {
        external_id: String,
        peer_id: String,
        sender_id: String,
        text: String,
        #[serde(default = "text_kind")]
        kind: String,
        #[serde(default)]
        payload: Option<Value>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        reply_to_external_id: Option<String>,
    },
    Sent {
        request_id: String,
        #[serde(default)]
        external_id: Option<String>,
    },
    Error {
        #[serde(default)]
        request_id: Option<String>,
        code: String,
        message: String,
        retryable: bool,
    },
    Connection {
        state: ConnectionState,
    },
    Pairing {
        qr: String,
    },
}

impl fmt::Debug for AdapterFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pairing { .. } => f
                .debug_struct("Pairing")
                .field("qr", &"[REDACTED]")
                .finish(),
            Self::Ready {
                version,
                capabilities,
                display_name,
            } => f
                .debug_struct("Ready")
                .field("version", version)
                .field("capabilities", capabilities)
                .field("display_name", display_name)
                .finish(),
            Self::Message {
                external_id,
                peer_id,
                sender_id,
                kind,
                ..
            } => f
                .debug_struct("Message")
                .field("external_id", external_id)
                .field("peer_id", peer_id)
                .field("sender_id", sender_id)
                .field("kind", kind)
                .finish_non_exhaustive(),
            Self::Sent {
                request_id,
                external_id,
            } => f
                .debug_struct("Sent")
                .field("request_id", request_id)
                .field("external_id", external_id)
                .finish(),
            Self::Error {
                request_id,
                code,
                retryable,
                ..
            } => f
                .debug_struct("Error")
                .field("request_id", request_id)
                .field("code", code)
                .field("retryable", retryable)
                .finish_non_exhaustive(),
            Self::Connection { state } => {
                f.debug_struct("Connection").field("state", state).finish()
            }
        }
    }
}

impl HostFrame {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Initialize {
                version,
                instance_id,
                state_dir,
                config,
                ..
            } => {
                version_check(*version)?;
                identity("instance ID", instance_id)?;
                bounded("state directory", state_dir, 16 * 1024)?;
                if !config.is_object() {
                    return Err(Error::InvalidInput(
                        "adapter config must be an object".into(),
                    ));
                }
            }
            Self::Send {
                request_id,
                peer_id,
                message,
            } => {
                identity("request ID", request_id)?;
                identity("peer ID", peer_id)?;
                content(&message.text, &message.kind, &message.payload)?;
                if let Some(id) = &message.reply_to_external_id {
                    identity("reply external ID", id)?;
                }
            }
            Self::Accepted {
                peer_id,
                external_id,
            }
            | Self::Rejected {
                peer_id,
                external_id,
                ..
            } => {
                identity("peer ID", peer_id)?;
                identity("external ID", external_id)?;
                if let Self::Rejected { code, .. } = self {
                    identity("error code", code)?;
                }
            }
            Self::Shutdown => {}
        }
        Ok(())
    }
}

impl AdapterFrame {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Ready {
                version,
                capabilities,
                display_name,
            } => {
                version_check(*version)?;
                identity("display name", display_name)?;
                if capabilities.len() > 32 {
                    return Err(Error::InvalidInput("too many adapter capabilities".into()));
                }
                for capability in capabilities {
                    identity("capability", capability)?;
                }
            }
            Self::Message {
                external_id,
                peer_id,
                sender_id,
                text,
                kind,
                payload,
                title,
                reply_to_external_id,
            } => {
                for (label, id) in [
                    ("external ID", external_id),
                    ("peer ID", peer_id),
                    ("sender ID", sender_id),
                ] {
                    identity(label, id)?;
                }
                content(text, kind, payload)?;
                if matches!(kind.as_str(), "reaction" | "typing") {
                    return Err(Error::InvalidInput(
                        "inbound control messages are unsupported".into(),
                    ));
                }
                if let Some(title) = title {
                    identity("title", title)?;
                }
                if let Some(id) = reply_to_external_id {
                    identity("reply external ID", id)?;
                }
            }
            Self::Sent {
                request_id,
                external_id,
            } => {
                identity("request ID", request_id)?;
                if let Some(id) = external_id {
                    identity("external ID", id)?;
                }
            }
            Self::Error {
                request_id,
                code,
                message,
                ..
            } => {
                if let Some(id) = request_id {
                    identity("request ID", id)?;
                }
                identity("error code", code)?;
                bounded("diagnostic", message, 4096)?;
            }
            Self::Connection { .. } => {}
            Self::Pairing { qr } => bounded("pairing payload", qr, MAX_TEXT_BYTES)?,
        }
        Ok(())
    }
}

pub fn decode_host(bytes: &[u8]) -> Result<HostFrame> {
    check_frame(bytes)?;
    let frame: HostFrame = serde_json::from_slice(bytes)
        .map_err(|_| Error::InvalidInput("malformed host protocol frame".into()))?;
    frame.validate()?;
    Ok(frame)
}

pub fn decode_adapter(bytes: &[u8]) -> Result<AdapterFrame> {
    check_frame(bytes)?;
    // Serde's unknown-field/variant diagnostics echo untrusted values. Keep
    // parse errors generic so malformed pairing frames cannot reveal a secret.
    let frame: AdapterFrame = serde_json::from_slice(bytes)
        .map_err(|_| Error::InvalidInput("malformed adapter protocol frame".into()))?;
    frame.validate()?;
    Ok(frame)
}

pub fn encode_host(frame: &HostFrame) -> Result<Vec<u8>> {
    frame.validate()?;
    encode(frame)
}
pub fn encode_adapter(frame: &AdapterFrame) -> Result<Vec<u8>> {
    frame.validate()?;
    encode(frame)
}

fn encode(frame: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    check_frame(&bytes)?;
    Ok(bytes)
}

fn check_frame(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(Error::InvalidInput(
            "protocol frame exceeds its limit or is empty".into(),
        ));
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if body.contains(&b'\n') {
        return Err(Error::InvalidInput(
            "protocol requires one object per line".into(),
        ));
    }
    Ok(())
}

fn version_check(version: u32) -> Result<()> {
    if version != VERSION {
        return Err(Error::InvalidInput(
            "unsupported adapter protocol version".into(),
        ));
    }
    Ok(())
}

fn bounded(label: &str, value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max || value.contains('\0') {
        return Err(Error::InvalidInput(format!(
            "{label} is empty or exceeds its limit"
        )));
    }
    Ok(())
}

fn text_kind() -> String {
    "text".into()
}
