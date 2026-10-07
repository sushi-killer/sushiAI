//! JSON-RPC 2.0 message types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Frame;

const VERSION: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Message {
    pub fn parse(text: &str) -> Result<Message, serde_json::Error> {
        let value: Value = serde_json::from_str(text)?;
        let has = |key: &str| value.get(key).is_some();
        Ok(if has("method") && has("id") {
            Message::Request(serde_json::from_value(value)?)
        } else if has("method") {
            Message::Notification(serde_json::from_value(value)?)
        } else {
            Message::Response(serde_json::from_value(value)?)
        })
    }
}

fn frame<T: Serialize>(message: &T) -> Frame {
    // Serializing these types cannot fail: all fields are strings and JSON values.
    Frame::Json(serde_json::to_string(message).unwrap_or_default())
}

impl Request {
    pub fn new(id: u64, method: &str, params: impl Serialize) -> Self {
        Request {
            jsonrpc: VERSION.into(),
            id: id.into(),
            method: method.into(),
            params: serde_json::to_value(params).unwrap_or(Value::Null),
        }
    }

    pub fn frame(&self) -> Frame {
        frame(self)
    }
}

impl Notification {
    pub fn new(method: &str, params: impl Serialize) -> Self {
        Notification {
            jsonrpc: VERSION.into(),
            method: method.into(),
            params: serde_json::to_value(params).unwrap_or(Value::Null),
        }
    }

    pub fn frame(&self) -> Frame {
        frame(self)
    }
}

impl Response {
    pub fn ok(id: Value, result: impl Serialize) -> Self {
        Response {
            jsonrpc: VERSION.into(),
            id,
            result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
            error: None,
        }
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Response {
            jsonrpc: VERSION.into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn frame(&self) -> Frame {
        frame(self)
    }
}
