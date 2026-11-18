use serde_json::{Value, json};

/// Raw JSON-RPC over HTTP to `/mcp`: every call self-contained, as a stateless server takes it.
#[derive(Clone)]
pub struct Mcp {
    http: reqwest::Client,
    pub url: String,
}

pub struct Answer {
    pub status: u16,
    pub text: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|error| panic!("{error}: {} {}", self.status, self.text))
    }

    pub fn result(&self) -> Value {
        let body = self.json();
        assert!(body.get("error").is_none(), "a JSON-RPC error: {body}");
        body["result"].clone()
    }

    pub fn error_code(&self) -> i64 {
        self.json()["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("no error: {}", self.text))
    }
}

impl Mcp {
    pub fn on(port: u16) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: format!("http://127.0.0.1:{port}/mcp"),
        }
    }

    pub async fn post(&self, body: &Value, headers: &[(&str, &str)]) -> Answer {
        self.post_raw(body.to_string(), headers).await
    }

    pub async fn post_raw(&self, body: String, headers: &[(&str, &str)]) -> Answer {
        self.try_post_raw(body, headers).await.expect("the request failed")
    }

    /// `None` when the server hung up before the request was written whole.
    pub async fn try_post_raw(&self, body: String, headers: &[(&str, &str)]) -> Option<Answer> {
        let mut request = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.body(body).send().await.ok()?;
        Some(Answer {
            status: response.status().as_u16(),
            text: response.text().await.ok()?,
        })
    }

    pub async fn call_with(&self, tool: &str, arguments: &Value, headers: &[(&str, &str)]) -> Answer {
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
        self.post(&body, headers).await
    }

    /// The result object of a tool call, which must not be a JSON-RPC error.
    pub async fn call(&self, tool: &str, arguments: &Value) -> Value {
        self.call_with(tool, arguments, &[]).await.result()
    }

    pub async fn tools(&self) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
        self.post(&body, &[]).await.result()
    }
}

/// A tool failure of ours: `isError`, `{"error","status"}` as the structured content and as the text.
pub fn failure(result: &Value) -> (String, u64) {
    assert_eq!(result["isError"], true, "{result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured.as_object().unwrap().len(), 2, "{result}");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, structured, "the text content differs from the structured one");
    (
        structured["error"].as_str().unwrap().to_owned(),
        structured["status"].as_u64().unwrap(),
    )
}
