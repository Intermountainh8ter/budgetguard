//! Live HTTP observer wrapper.
//!
//! An agent endpoint under budget audit speaks any protocol; what the
//! auditor needs is a four-field summary of what the run spent:
//! tokens, tool_calls, usd, wall_seconds. `AgentProfile` names where to
//! POST a prompt and where to find each of those four fields in the
//! response. The adapter POSTs once, extracts the four fields, and
//! hands the caller an `Actual` struct the existing `enforced` /
//! `breaker_run` / `certify` functions consume.

use crate::Actual;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentProfile {
    pub name: String,
    pub url: String,
    #[serde(default = "default_prompt_field")]
    pub prompt_field: String,
    pub tokens_json_path: String,
    pub tool_calls_json_path: String,
    pub usd_json_path: String,
    pub wall_seconds_json_path: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_prompt_field() -> String {
    "prompt".into()
}

fn default_timeout_s() -> u64 {
    120
}

fn default_max_retries() -> u32 {
    2
}

#[derive(Debug, Error)]
pub enum ObserveError {
    #[error("api_key_env `{0}` is set but the environment variable is not set")]
    AuthMissing(String),
    #[error("endpoint refused authentication (HTTP {status})")]
    AuthRejected { status: u16 },
    #[error("endpoint returned HTTP {status} after {attempts} attempts")]
    HttpStatus { status: u16, attempts: u32 },
    #[error("response missing field at JSON path `{path}`")]
    MissingField { path: String },
    #[error("response field at `{path}` is not a number")]
    NotANumber { path: String },
    #[error("response field at `{path}` is negative: {value}")]
    Negative { path: String, value: f64 },
    #[error("response could not be parsed as JSON")]
    MalformedJson,
    #[error("transport error after {attempts} attempts: {source}")]
    Transport {
        attempts: u32,
        #[source]
        source: reqwest::Error,
    },
}

fn pointer_from_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        let mut s = String::with_capacity(trimmed.len() + 1);
        for seg in trimmed.split('/') {
            s.push('/');
            s.push_str(seg);
        }
        s
    }
}

fn extract_nonnegative(body: &serde_json::Value, path: &str) -> Result<f64, ObserveError> {
    let p = pointer_from_path(path);
    let v = body.pointer(&p).ok_or(ObserveError::MissingField {
        path: path.to_string(),
    })?;
    let n = v.as_f64().ok_or(ObserveError::NotANumber {
        path: path.to_string(),
    })?;
    if !n.is_finite() || n < 0.0 {
        return Err(ObserveError::Negative {
            path: path.to_string(),
            value: n,
        });
    }
    Ok(n)
}

/// POST the prompt to the agent and extract the four spend fields.
pub fn observe(profile: &AgentProfile, prompt: &str) -> Result<Actual, ObserveError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    rt.block_on(observe_async(profile, prompt))
}

pub async fn observe_async(profile: &AgentProfile, prompt: &str) -> Result<Actual, ObserveError> {
    let auth = match &profile.api_key_env {
        None => None,
        Some(name) => Some(std::env::var(name).map_err(|_| ObserveError::AuthMissing(name.clone()))?),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(profile.timeout_s))
        .build()
        .map_err(|e| ObserveError::Transport {
            attempts: 0,
            source: e,
        })?;
    let body = serde_json::json!({ profile.prompt_field.clone(): prompt });
    let resp =
        send_with_retry(&client, &profile.url, auth.as_deref(), &body, profile.max_retries).await?;
    let tokens = extract_nonnegative(&resp, &profile.tokens_json_path)?;
    let tool_calls = extract_nonnegative(&resp, &profile.tool_calls_json_path)?;
    let usd = extract_nonnegative(&resp, &profile.usd_json_path)?;
    let wall_seconds = extract_nonnegative(&resp, &profile.wall_seconds_json_path)?;
    Ok(Actual {
        tokens: tokens as u64,
        tool_calls: tool_calls as u64,
        usd,
        wall_seconds,
    })
}

async fn send_with_retry(
    client: &reqwest::Client,
    url: &str,
    auth: Option<&str>,
    body: &serde_json::Value,
    max_retries: u32,
) -> Result<serde_json::Value, ObserveError> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let mut req = client.post(url).json(body);
        if let Some(k) = auth {
            req = req.bearer_auth(k);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return resp.json().await.map_err(|_| ObserveError::MalformedJson);
                }
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    return Err(ObserveError::AuthRejected {
                        status: status.as_u16(),
                    });
                }
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if !retryable || attempt > max_retries {
                    return Err(ObserveError::HttpStatus {
                        status: status.as_u16(),
                        attempts: attempt,
                    });
                }
            }
            Err(e) => {
                let retryable = e.is_timeout() || e.is_connect() || e.is_request();
                if !retryable || attempt > max_retries {
                    return Err(ObserveError::Transport {
                        attempts: attempt,
                        source: e,
                    });
                }
            }
        }
        let delay_ms = 100u64.saturating_mul(1u64 << (attempt - 1).min(10));
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn profile(base: &str) -> AgentProfile {
        AgentProfile {
            name: "mock".into(),
            url: format!("{base}/run"),
            prompt_field: "prompt".into(),
            tokens_json_path: "usage/tokens".into(),
            tool_calls_json_path: "usage/tool_calls".into(),
            usd_json_path: "usage/usd".into(),
            wall_seconds_json_path: "usage/wall_seconds".into(),
            api_key_env: None,
            timeout_s: 2,
            max_retries: 2,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn observe_extracts_the_four_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "usage": {"tokens": 1024, "tool_calls": 3, "usd": 0.12, "wall_seconds": 4.5}
            })))
            .mount(&server)
            .await;
        let a = observe_async(&profile(&server.uri()), "ignored").await.expect("ok");
        assert_eq!(a.tokens, 1024);
        assert_eq!(a.tool_calls, 3);
        assert!((a.usd - 0.12).abs() < 1e-12);
        assert!((a.wall_seconds - 4.5).abs() < 1e-12);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn negative_wall_seconds_errors_with_the_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "usage": {"tokens": 1, "tool_calls": 0, "usd": 0.0, "wall_seconds": -1.0}
            })))
            .mount(&server)
            .await;
        match observe_async(&profile(&server.uri()), "x").await {
            Err(ObserveError::Negative { path, .. }) => assert_eq!(path, "usage/wall_seconds"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_tokens_names_the_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "usage": {"tool_calls": 0, "usd": 0.0, "wall_seconds": 0.1}
            })))
            .mount(&server)
            .await;
        match observe_async(&profile(&server.uri()), "x").await {
            Err(ObserveError::MissingField { path }) => assert_eq!(path, "usage/tokens"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retries_a_429() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "usage": {"tokens": 1, "tool_calls": 0, "usd": 0.0, "wall_seconds": 0.1}
            })))
            .mount(&server)
            .await;
        observe_async(&profile(&server.uri()), "x").await.expect("retry ok");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_rejected_is_fatal() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/run"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let err = observe_async(&profile(&server.uri()), "x").await.expect_err("auth");
        assert!(matches!(err, ObserveError::AuthRejected { status: 401 }));
    }
}
