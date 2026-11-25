//! The runner's only channel to the outside: HTTP against the server.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone)]
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

/// What became of a live diff the server was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDiffSent {
    Sent,
    /// The server will not take a body this size; sending it again is no use.
    TooLarge,
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

    /// Asks for a queued run that fits in `capacity`, the room this runner has
    /// for one more; without it the server hands out the oldest queued run.
    pub async fn claim_run(
        &self,
        runner_id: &str,
        capacity: Option<Value>,
    ) -> Result<Option<Value>> {
        let value = self
            .post(
                "/internal/runs/claim",
                json!({ "runner_id": runner_id, "capacity": capacity }),
            )
            .await?;
        Ok(if value.is_object() { Some(value) } else { None })
    }

    /// Keeps the run alive, and answers with what the server thinks of it:
    /// `running`, or `cancelled` or `failed` for a run it has stopped. None
    /// from a server too old to say. A pid and pgid of zero say the agent has
    /// exited.
    pub async fn heartbeat(
        &self,
        run_id: &str,
        pid: Option<i64>,
        pgid: Option<i64>,
    ) -> Result<Option<String>> {
        let reply = self
            .post(
                &format!("/internal/runs/{run_id}/heartbeat"),
                json!({ "pid": pid, "pgid": pgid }),
            )
            .await?;
        Ok(reply["status"].as_str().map(str::to_string))
    }

    /// The room this runner has while every slot is taken: it claims
    /// nothing, and still tells the server how the host is doing.
    pub async fn report_capacity(&self, capacity: Value) -> Result<()> {
        self.post("/internal/runner/capacity", json!({ "capacity": capacity }))
            .await?;
        Ok(())
    }

    /// The working diff of a run in progress, sent whenever it changed.
    pub async fn live_diff(&self, run_id: &str, patch: &str) -> Result<LiveDiffSent> {
        let path = format!("/internal/runs/{run_id}/diff");
        let response = self
            .http
            .post(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .json(&json!({ "patch": patch }))
            .send()
            .await
            .with_context(|| format!("POST {path}"))?;
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            return Ok(LiveDiffSent::TooLarge);
        }
        self.decode(&path, response).await?;
        Ok(LiveDiffSent::Sent)
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

    /// Hand back an OAuth login the harness refreshed, so the server can seal it
    /// over the stored one. `owner` is `runs/<id>` or `jobs/<id>`.
    pub async fn hand_back_credential(&self, owner: &str, refreshed: Value) -> Result<()> {
        self.post(&format!("/internal/{owner}/credential"), refreshed)
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
