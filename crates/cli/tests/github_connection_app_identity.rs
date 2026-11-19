//! Acceptance tests for the identity the connections screen reports: a stored
//! GitHub app is named by its app id once its key can sign, and that name goes
//! away when the app is disconnected.
//!
//! Both criteria drive the real server over its public surface (GraphQL), so the
//! base revision fails here because `Connection` carries no app id yet, not
//! because a helper is missing.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A value that cannot appear by accident in a SQLite page.
const APP_ID: &str = "271828182845";

/// A real, complete 2048-bit RSA key: it can sign, so the app it belongs to is
/// connected.
const PRIVATE_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAr0Kzi8yfE49NHxfclF8l+fscRnWVb0irZx0WLG5Y+lReU5b4
6noosgqBl9ddadI5sOlhWewk9CEWXgUxISA6qUjn+WNNbFHqlofL7vZa5g6zZonq
6X651L45LVlXi1vplzJ3LsIkD9Y0a0bNktHC04X6jswkk9UHcUqdBL5FWZtgTmue
/FYwZEyu2xVYiCPbDvApnS+knHCGjGw2eRoskc76m4u/E22Xce3SwATc0DBt4DG4
8ZubP6PW2rbA41EVpSGXr+9fno5hZrVJfc3mF2+nHi5uGi0YR+5Et9MY/p19TisT
4QYEoSqEf3XOJIM5V7+SioBqK7r3dJW2KZTnTwIDAQABAoIBACeoSrNoDljhNLzD
6/xA2XmK2jeRVMqVIX/YI5PTNlwSqjN09ocsNDgxS2i96VvQH5DKBmbQGf6J3CNT
Pm41+kAjCKa0UykfEhuUx88fPKtRCUQ7EgZa0Bb1IkACNt6IYj99wl04ves9d6gM
oejGgTTW3L7iL6ap7iCzFvujSFMBhvxqJh8PYuOjYa+r9ySLzwv90613MbSU5Rjs
gZvPcKqVlJB7VTa/gFBULxev9njDUybth7a0Mrvaa15CdJq/OwRi/G3OmqrC7oLI
utEdXjAdC1fdoDjAKh49atZLG+LuLeHwzPkiZ4nC+O7KxL8U3+fFt8B6lWku5eDE
GcHDqBkCgYEA405mlnQ6Y/30ai3CDCJ+AZpT2QH7TXKXylf9f7Tytax7trGqcTX7
mj2cHi5olDrVYm+J7vMPOfW1pvwUyu7T1EZqnaIUk49vwdlV8rX3u+UDHJqfwQi6
LiPuMMfDbIjuoE+XfF++TvqaSu3VwQsQ6Q2Crx5OF/RV++DG1OA/vBkCgYEAxWJm
BjTKBDFpf+nqXoFxIrbKQdL934Ssb33d9JZ5yG2RftSspiMszGEU9WbbpOsws0Yt
TDrrDzYk/iqtwO/tFGyNB/i5bar8Cn36eF5FVSW5Hrej1r4u7U9dM4IHTE4DBefJ
TokwShvN2ZvH600e840ctb3cizSIOU5/fNPRK6cCgYAL7Jr5HZPKlxHxWAEr/cs6
bkwjwDTctTw9QyxrPKTW3vuGc/+K/Nf8GWtNMgYOcJ+tEEjeHiYYVkl3PxYgJzM5
mZASQORvcUDdnHcjymP6mWL2Zy4mRDzwGrZuHKylzvElu8cooSUso3zgnWVvQRNc
BndqlF5oZ0rzcbE8jZ8KUQKBgQCVdendPzHNNQE225AdWZSqfZDvgO29x7+ueLeN
0jeLHfRy/mVxM+djJwvwup8M5Q4uYbBBLcf577zZPMXu934k3gF4jM0UDsnkw/na
FUAYtoTTjqigHch454SiUQ5nqBa/7QBQ2K7ZHu6jnUMnmHEcaLjZoVeTjQatphQB
GH8ltwKBgQCyErmb02Z/JiQVd/9n3nkITDSfB0oz++LBltPXIYps00+kSGSZqc+y
2tHY7DN/e52Nnw50tCbkXPqbLIBSx0LGu7COSyoT7JJFIykpuw7PQSVMkG7KTKAc
gerU/uBGgcEkg/OmBjuFYTwt9IjFkJ5LEmcYTk5c6mZ59ARsKA8ldg==
-----END RSA PRIVATE KEY-----";

struct Fixture {
    _root: PathBuf,
    url: String,
    token: String,
    client: reqwest::Client,
}

impl Fixture {
    async fn start(tag: &str) -> Fixture {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hulahoop-app-identity-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        std::fs::create_dir_all(&data).expect("create data dir");

        let serve_data = data.clone();
        tokio::spawn(async move {
            let _ = hulahoop_server::serve(vec![
                "--data-dir".to_string(),
                serve_data.to_string_lossy().into_owned(),
                "--port".to_string(),
                "0".to_string(),
                "--listen".to_string(),
                "127.0.0.1".to_string(),
            ])
            .await;
        });

        let connection = wait_for_connection(&data);
        Fixture {
            _root: root,
            url: connection["url"]
                .as_str()
                .expect("connection url")
                .to_string(),
            token: connection["token"]
                .as_str()
                .expect("connection token")
                .to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn gql(&self, query: &str, variables: Value) -> Value {
        let response = self
            .client
            .post(format!("{}/graphql", self.url))
            .bearer_auth(&self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("graphql request failed: {error}\nquery: {query}"));
        let status = response.status();
        let body = response.text().await.expect("graphql body");
        let value: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!("graphql returned non-json ({status}): {error}: {body}")
        });
        if let Some(errors) = value.get("errors") {
            panic!("graphql error for {query}: {errors}");
        }
        value["data"].clone()
    }

    async fn create_app(&self) {
        self.gql(
            r#"mutation ($appId: String!, $privateKey: String!) {
                createGithubApp(appId: $appId, privateKey: $privateKey) { __typename }
            }"#,
            json!({ "appId": APP_ID, "privateKey": PRIVATE_KEY }),
        )
        .await;
    }

    async fn disconnect_app(&self) {
        self.gql(
            r#"mutation { disconnectGithubApp { provider connected } }"#,
            json!({}),
        )
        .await;
    }

    async fn github_connection(&self) -> Value {
        let connections = self
            .gql("{ connections { provider connected appId } }", json!({}))
            .await["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        connections
            .iter()
            .find(|entry| {
                entry["provider"]
                    .as_str()
                    .is_some_and(|provider| provider.eq_ignore_ascii_case("github"))
            })
            .cloned()
            .unwrap_or_else(|| panic!("connections should list github, got {connections:?}"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac1_connections_query_returns_the_app_id_once_the_key_can_sign() {
    let fixture = Fixture::start("ac1").await;
    fixture.create_app().await;

    let github = fixture.github_connection().await;
    assert_eq!(
        github["appId"].as_str(),
        Some(APP_ID),
        "a stored app whose key can sign should report its app id, got {github}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac2_connections_query_returns_no_app_id_after_the_app_is_disconnected() {
    let fixture = Fixture::start("ac2").await;
    fixture.create_app().await;

    let before = fixture.github_connection().await;
    assert_eq!(
        before["appId"].as_str(),
        Some(APP_ID),
        "the app id should be visible before disconnecting, got {before}"
    );

    fixture.disconnect_app().await;

    let after = fixture.github_connection().await;
    assert_eq!(
        after["appId"],
        Value::Null,
        "a disconnected app should report no app id, got {after}"
    );
}

fn wait_for_connection(data: &Path) -> Value {
    let path = data.join("connection.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(contents) = std::fs::read(&path) {
            if let Ok(value) = serde_json::from_slice::<Value>(&contents) {
                if value.get("url").and_then(Value::as_str).is_some() {
                    return value;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "server did not write {} within 60s",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
