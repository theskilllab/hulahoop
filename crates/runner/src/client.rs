//! The runner's only channel to the outside: HTTP against the server.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Clone)]
pub struct Connection {
    /// Where the server listens, scheme included: `http://10.0.0.4:51234`.
    pub base: String,
    pub token: String,
    pub data_dir: PathBuf,
}

impl Connection {
    /// A runner that does not share a directory with the server: the host runs
    /// the server, so the runner needs an address and a token, nothing else.
    pub fn remote(server: &str, token: &str, data_dir: PathBuf) -> Self {
        Connection {
            base: server.trim_end_matches('/').to_string(),
            token: token.to_string(),
            data_dir,
        }
    }

    pub fn load(data_dir: &std::path::Path) -> Result<Self> {
        let path = hulahoop_core::contract::connection_file(data_dir);
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "cannot read {} — start the app (or `just dev`) first",
                path.display()
            )
        })?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let port = value["port"]
            .as_u64()
            .context("connection.json has no port")? as u16;
        let base = value["url"]
            .as_str()
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
        Ok(Connection {
            base,
            token: value["token"]
                .as_str()
                .context("connection.json has no token")?
                .to_string(),
            data_dir: value["data_dir"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.to_path_buf()),
        })
    }
}

impl Client {
    pub fn new(connection: &Connection) -> Result<Self> {
        Ok(Client {
            base: connection.base.clone(),
            token: connection.token.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
        })
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {path}"))?;
        self.decode(path, response).await
    }

    async fn decode(&self, path: &str, response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("{path} returned {status}: {text}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn claim_run(&self, runner_id: &str) -> Result<Option<Value>> {
        let value = self
            .post("/internal/runs/claim", json!({ "runner_id": runner_id }))
            .await?;
        Ok(if value.is_object() { Some(value) } else { None })
    }

    pub async fn heartbeat(&self, run_id: &str, pid: Option<i64>, pgid: Option<i64>) -> Result<()> {
        self.post(
            &format!("/internal/runs/{run_id}/heartbeat"),
            json!({ "pid": pid, "pgid": pgid }),
        )
        .await?;
        Ok(())
    }

    pub async fn event(&self, run_id: &str, kind: &str, payload: Value) -> Result<()> {
        self.post(
            &format!("/internal/runs/{run_id}/events"),
            json!({ "kind": kind, "payload": payload }),
        )
        .await?;
        Ok(())
    }

    pub async fn complete_run(&self, run_id: &str, completion: Value) -> Result<()> {
        self.post(&format!("/internal/runs/{run_id}/complete"), completion)
            .await?;
        Ok(())
    }

    pub async fn claim_job(&self, lumi: bool) -> Result<Option<Value>> {
        let value = self
            .post("/internal/jobs/claim", json!({ "lumi": lumi }))
            .await?;
        Ok(if value.is_object() { Some(value) } else { None })
    }

    pub async fn complete_job(
        &self,
        job_id: &str,
        result: Value,
        error: Option<String>,
    ) -> Result<()> {
        self.post(
            &format!("/internal/jobs/{job_id}/complete"),
            json!({ "result": result, "error": error }),
        )
        .await?;
        Ok(())
    }
}
