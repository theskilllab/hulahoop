//! Workspace-scoped cost reporting. Run activity is dated by finish; task effort
//! is attributed to the period its issue merged, including earlier failed runs.
use async_graphql::Json;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{sqlite::SqliteRow, FromRow, Row};
use uuid::Uuid;

use crate::db::Db;

#[derive(Debug, Default, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    runs: i64,
    succeeded: i64,
    failed: i64,
    cancelled: i64,
    tokens_in: i64,
    tokens_out: i64,
    cache_read: i64,
    cache_write: i64,
    known_cost_runs: i64,
    equivalent_usd: f64,
    known_cash_runs: i64,
    cash_usd: f64,
    active_runs: i64,
    active_minutes: f64,
    known_turn_runs: i64,
    turns: i64,
    known_tool_runs: i64,
    tools: i64,
}

const TOTALS: &str = "count(*) as runs,
 sum(case when r.status = 'succeeded' then 1 else 0 end) as succeeded,
 sum(case when r.status in ('failed', 'timed_out') then 1 else 0 end) as failed,
 sum(case when r.status = 'cancelled' then 1 else 0 end) as cancelled,
 coalesce(sum(r.tokens_in),0) as tokens_in, coalesce(sum(r.tokens_out),0) as tokens_out,
 coalesce(sum(r.cache_read_tokens),0) as cache_read,
 coalesce(sum(r.cache_write_tokens),0) as cache_write,
 count(r.api_equivalent_usd) as known_cost_runs,
 coalesce(sum(r.api_equivalent_usd),0.0) as equivalent_usd,
 count(r.actual_cost_usd) as known_cash_runs,
 coalesce(sum(r.actual_cost_usd),0.0) as cash_usd,
 count(case when r.started_at is not null and r.finished_at is not null then 1 end) as active_runs,
 coalesce(sum(case when r.started_at is not null and r.finished_at is not null
   then max(0.0,(julianday(r.finished_at)-julianday(r.started_at))*1440) end),0.0) as active_minutes,
 count(r.model_turns) as known_turn_runs, coalesce(sum(r.model_turns),0) as turns,
 count(r.tool_calls) as known_tool_runs, coalesce(sum(r.tool_calls),0) as tools";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Breakdown {
    label: String,
    #[serde(flatten)]
    totals: Totals,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DailyModel {
    day: String,
    model: String,
    #[serde(flatten)]
    totals: Totals,
}

impl<'r> FromRow<'r, SqliteRow> for Breakdown {
    fn from_row(row: &'r SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            label: row.try_get("label")?,
            totals: Totals::from_row(row)?,
        })
    }
}

impl<'r> FromRow<'r, SqliteRow> for DailyModel {
    fn from_row(row: &'r SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            day: row.try_get("day")?,
            model: row.try_get("model")?,
            totals: Totals::from_row(row)?,
        })
    }
}

#[derive(Debug, Default, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cohort {
    tasks: i64,
    runs: i64,
    failed_runs: i64,
    elapsed_minutes: f64,
    active_runs: i64,
    active_minutes: f64,
    known_cost_runs: i64,
    equivalent_usd: f64,
    known_cash_runs: i64,
    cash_usd: f64,
    known_turn_runs: i64,
    turns: i64,
    known_tool_runs: i64,
    tools: i64,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct Failure {
    id: Uuid,
    issue_id: Uuid,
    issue_key: String,
    issue_title: String,
    agent: String,
    model: String,
    reason: String,
    finished_at: String,
    api_equivalent_usd: Option<f64>,
}

fn window(period: &str) -> async_graphql::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = Utc::now();
    let midnight = Utc
        .with_ymd_and_hms(end.year(), end.month(), end.day(), 0, 0, 0)
        .single()
        .expect("valid UTC day");
    let start = match period {
        "7d" => midnight - Duration::days(6),
        "30d" => midnight - Duration::days(29),
        "mtd" => Utc
            .with_ymd_and_hms(end.year(), end.month(), 1, 0, 0, 0)
            .single()
            .expect("valid UTC month"),
        _ => return Err("period must be 7d, 30d or mtd".into()),
    };
    Ok((start, end))
}

fn prior_window(
    period: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    match period {
        "7d" => (start - Duration::days(7), end - Duration::days(7)),
        "30d" => (start - Duration::days(30), end - Duration::days(30)),
        _ => {
            let last_month = start - Duration::days(1);
            let first = Utc
                .with_ymd_and_hms(last_month.year(), last_month.month(), 1, 0, 0, 0)
                .single()
                .expect("valid UTC month");
            (first, first + (end - start).min(start - first))
        }
    }
}

async fn totals(
    db: &Db,
    workspace: Uuid,
    start: i64,
    end: i64,
    agent: Option<Uuid>,
    model: Option<&str>,
) -> sqlx::Result<Totals> {
    let sql = format!(
        "select {TOTALS} from runs r join issues i on i.id = r.issue_id
        where i.workspace_id = ? and unixepoch(r.finished_at) >= ? and unixepoch(r.finished_at) < ?
        and (? is null or r.agent_id = ?) and (? is null or coalesce(r.actual_model, r.model) = ?)"
    );
    sqlx::query_as(&sql)
        .bind(workspace)
        .bind(start)
        .bind(end)
        .bind(agent)
        .bind(agent)
        .bind(model)
        .bind(model)
        .fetch_one(&db.pool)
        .await
}

async fn breakdown(
    db: &Db,
    workspace: Uuid,
    start: i64,
    end: i64,
    agent: Option<Uuid>,
    model: Option<&str>,
    dimension: &str,
) -> sqlx::Result<Vec<Breakdown>> {
    // All three dimensions are fixed SQL expressions supplied only by this module.
    let status = if dimension == "coalesce(r.failure_category, 'uncategorized')" {
        "and r.status in ('failed', 'timed_out')"
    } else {
        ""
    };
    let sql = format!("select {dimension} as label, {TOTALS} from runs r join issues i on i.id = r.issue_id left join agents a on a.id = r.agent_id
        where i.workspace_id = ? and unixepoch(r.finished_at) >= ? and unixepoch(r.finished_at) < ?
        and (? is null or r.agent_id = ?) and (? is null or coalesce(r.actual_model, r.model) = ?) {status}
        group by {dimension} order by equivalent_usd desc, runs desc");
    sqlx::query_as(&sql)
        .bind(workspace)
        .bind(start)
        .bind(end)
        .bind(agent)
        .bind(agent)
        .bind(model)
        .bind(model)
        .fetch_all(&db.pool)
        .await
}

async fn cohort(
    db: &Db,
    workspace: Uuid,
    start: i64,
    end: i64,
    agent: Option<Uuid>,
    model: Option<&str>,
) -> sqlx::Result<Cohort> {
    // One row per finished issue before summing: a task's earlier failed attempts
    // and all collaborators belong to the full task effort at merge time.
    sqlx::query_as("select count(*) as tasks, coalesce(sum(t.runs),0) as runs,
        coalesce(sum(t.failed_runs),0) as failed_runs,
        coalesce(sum(t.elapsed_minutes),0.0) as elapsed_minutes,
        coalesce(sum(t.active_runs),0) as active_runs,
        coalesce(sum(t.active_minutes),0.0) as active_minutes,
        coalesce(sum(t.known_cost_runs),0) as known_cost_runs,
        coalesce(sum(t.equivalent_usd),0.0) as equivalent_usd,
        coalesce(sum(t.known_cash_runs),0) as known_cash_runs,
        coalesce(sum(t.cash_usd),0.0) as cash_usd,
        coalesce(sum(t.known_turn_runs),0) as known_turn_runs,
        coalesce(sum(t.turns),0) as turns,
        coalesce(sum(t.known_tool_runs),0) as known_tool_runs,
        coalesce(sum(t.tools),0) as tools
        from (select i.id, count(r.id) as runs,
          sum(case when r.status in ('failed', 'timed_out') then 1 else 0 end) as failed_runs,
          max(0.0,(julianday(i.merged_at)-julianday(i.created_at))*1440) as elapsed_minutes,
          count(case when r.started_at is not null and r.finished_at is not null then 1 end) as active_runs,
          coalesce(sum(case when r.started_at is not null and r.finished_at is not null
            then max(0.0,(julianday(r.finished_at)-julianday(r.started_at))*1440) end),0.0) as active_minutes,
          count(r.api_equivalent_usd) as known_cost_runs,
          coalesce(sum(r.api_equivalent_usd),0.0) as equivalent_usd,
          count(r.actual_cost_usd) as known_cash_runs,
          coalesce(sum(r.actual_cost_usd),0.0) as cash_usd,
          count(r.model_turns) as known_turn_runs, coalesce(sum(r.model_turns),0) as turns,
          count(r.tool_calls) as known_tool_runs, coalesce(sum(r.tool_calls),0) as tools
          from issues i left join runs r on r.issue_id = i.id
            and r.finished_at is not null and julianday(r.finished_at) <= julianday(i.merged_at)
          where i.workspace_id = ? and i.merged_at is not null
            and unixepoch(i.merged_at) >= ? and unixepoch(i.merged_at) < ?
            and ((? is null and ? is null) or exists
                (select 1 from runs picked where picked.issue_id = i.id
                  and picked.finished_at is not null
                  and julianday(picked.finished_at) <= julianday(i.merged_at)
                  and (? is null or picked.agent_id = ?) and (? is null or coalesce(picked.actual_model, picked.model) = ?)))
          group by i.id) t")
        .bind(workspace).bind(start).bind(end)
        .bind(agent).bind(model).bind(agent).bind(agent).bind(model).bind(model)
        .fetch_one(&db.pool).await
}

pub async fn report(
    db: &Db,
    workspace: Uuid,
    period: &str,
    agent: Option<Uuid>,
    model: Option<&str>,
) -> async_graphql::Result<Json<Value>> {
    let (start, end) = window(period)?;
    let (prior, prior_end) = prior_window(period, start, end);
    let current = totals(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
    )
    .await?;
    let previous = totals(
        db,
        workspace,
        prior.timestamp(),
        prior_end.timestamp(),
        agent,
        model,
    )
    .await?;
    let tasks = cohort(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
    )
    .await?;
    let prior_tasks = cohort(
        db,
        workspace,
        prior.timestamp(),
        prior_end.timestamp(),
        agent,
        model,
    )
    .await?;
    let daily = breakdown(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
        "date(r.finished_at)",
    )
    .await?;
    let daily_models: Vec<DailyModel> = sqlx::query_as(&format!(
        "select date(r.finished_at) as day, coalesce(r.actual_model, r.model) as model, {TOTALS}
        from runs r join issues i on i.id = r.issue_id where i.workspace_id = ?
        and unixepoch(r.finished_at) >= ? and unixepoch(r.finished_at) < ?
        and (? is null or r.agent_id = ?) and (? is null or coalesce(r.actual_model, r.model) = ?)
        group by day, model order by day, model"
    ))
    .bind(workspace)
    .bind(start.timestamp())
    .bind(end.timestamp())
    .bind(agent)
    .bind(agent)
    .bind(model)
    .bind(model)
    .fetch_all(&db.pool)
    .await?;
    let agents = breakdown(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
        "coalesce(a.name, 'unknown')",
    )
    .await?;
    let models = breakdown(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
        "coalesce(r.actual_model, r.model)",
    )
    .await?;
    let reasons = breakdown(
        db,
        workspace,
        start.timestamp(),
        end.timestamp(),
        agent,
        model,
        "coalesce(r.failure_category, 'uncategorized')",
    )
    .await?;
    let (month_start, _) = window("mtd")?;
    let month = totals(
        db,
        workspace,
        month_start.timestamp(),
        end.timestamp(),
        None,
        None,
    )
    .await?;
    let plan_usage = breakdown(
        db,
        workspace,
        month_start.timestamp(),
        end.timestamp(),
        None,
        None,
        "coalesce(r.connector_id, 'unknown')",
    )
    .await?;
    let next_month = if end.month() == 12 {
        Utc.with_ymd_and_hms(end.year() + 1, 1, 1, 0, 0, 0)
    } else {
        Utc.with_ymd_and_hms(end.year(), end.month() + 1, 1, 0, 0, 0)
    }
    .single()
    .expect("valid UTC month");
    let unsuccessful_tasks: (i64,) = sqlx::query_as(
        "select count(distinct i.id) from issues i join runs r on r.issue_id = i.id
        where i.workspace_id = ? and i.merged_at is null and i.abandon_reason is null and r.status in ('failed', 'timed_out')
        and unixepoch(r.finished_at) >= ? and unixepoch(r.finished_at) < ?
        and (? is null or r.agent_id = ?) and (? is null or coalesce(r.actual_model, r.model) = ?)",
    )
    .bind(workspace)
    .bind(start.timestamp())
    .bind(end.timestamp())
    .bind(agent)
    .bind(agent)
    .bind(model)
    .bind(model)
    .fetch_one(&db.pool)
    .await?;
    let failures: Vec<Failure> = sqlx::query_as("select r.id, i.id as issue_id,
        i.issue_key, i.title as issue_title,
        coalesce(a.name,'unknown') as agent, coalesce(r.actual_model, r.model) as model, coalesce(r.failure_category,'uncategorized') as reason,
        r.finished_at, r.api_equivalent_usd
        from runs r join issues i on i.id = r.issue_id
        left join agents a on a.id = r.agent_id
        where i.workspace_id = ? and r.status in ('failed', 'timed_out')
        and unixepoch(r.finished_at) >= ? and unixepoch(r.finished_at) < ?
        and (? is null or r.agent_id = ?) and (? is null or coalesce(r.actual_model, r.model) = ?)
        order by r.finished_at desc limit 15")
        .bind(workspace).bind(start.timestamp()).bind(end.timestamp())
        .bind(agent).bind(agent).bind(model).bind(model).fetch_all(&db.pool).await?;
    let settings: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "select api_budget_usd, success_target_percent from cost_settings where workspace_id = ?",
    )
    .bind(workspace)
    .fetch_optional(&db.pool)
    .await?;
    let plans: Vec<(String, String, f64, Option<i64>)> = sqlx::query_as(
        "select connector_id, label, monthly_fee_usd, allowance_tokens from cost_plans where workspace_id = ? order by label")
        .bind(workspace).fetch_all(&db.pool).await?;
    Ok(Json(json!({
        "period": period, "start": start, "end": end, "priorStart": prior,
        "current": current, "previous": previous, "tasks": tasks, "priorTasks": prior_tasks,
        "month": month, "planUsage": plan_usage, "monthElapsedDays": (end - month_start).num_seconds() as f64 / 86_400.0,
        "monthTotalDays": (next_month - month_start).num_days(), "unsuccessfulTasks": unsuccessful_tasks.0,
        "daily": daily, "dailyModels": daily_models, "agents": agents, "models": models,
        "failureReasons": reasons,
        "failures": failures,
        "settings": {"apiBudgetUsd": settings.as_ref().and_then(|s| s.0),
                     "successTargetPercent": settings.as_ref().and_then(|s| s.1)},
        "plans": plans.into_iter().map(|(connector, label, fee, allowance)| json!({
            "connectorId": connector, "label": label, "monthlyFeeUsd": fee, "allowanceTokens": allowance
        })).collect::<Vec<_>>()
    })))
}

pub async fn save_settings(
    db: &Db,
    workspace: Uuid,
    budget: Option<f64>,
    target: Option<f64>,
) -> async_graphql::Result<bool> {
    if budget.is_some_and(|v| !v.is_finite() || v < 0.0)
        || target.is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
    {
        return Err("budget must be nonnegative and success target between 0 and 100".into());
    }
    sqlx::query("insert into cost_settings (workspace_id,api_budget_usd,success_target_percent,updated_at)
        values (?,?,?,?) on conflict(workspace_id) do update set api_budget_usd=excluded.api_budget_usd,
        success_target_percent=excluded.success_target_percent, updated_at=excluded.updated_at")
        .bind(workspace).bind(budget).bind(target).bind(Utc::now().to_rfc3339())
        .execute(&db.pool).await?;
    Ok(true)
}

pub async fn save_plan(
    db: &Db,
    workspace: Uuid,
    connector: &str,
    label: &str,
    fee: f64,
    allowance: Option<i64>,
) -> async_graphql::Result<bool> {
    if !matches!(
        connector,
        "claude-sub" | "codex-sub" | "pi" | "opencode-go" | "claude-api"
    ) || label.trim().is_empty()
        || label.len() > 80
        || !fee.is_finite()
        || fee < 0.0
        || allowance.is_some_and(|value| value < 0)
    {
        return Err("invalid plan, fee, or allowance".into());
    }
    sqlx::query("insert into cost_plans (workspace_id,connector_id,label,monthly_fee_usd,allowance_tokens)
        values (?,?,?,?,?) on conflict(workspace_id,connector_id) do update set
        label=excluded.label, monthly_fee_usd=excluded.monthly_fee_usd, allowance_tokens=excluded.allowance_tokens")
        .bind(workspace).bind(connector).bind(label.trim()).bind(fee).bind(allowance)
        .execute(&db.pool).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prior_period_matches_calendar_days_and_partial_day() {
        let end = Utc
            .with_ymd_and_hms(2026, 3, 31, 12, 0, 0)
            .single()
            .unwrap();
        let start = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).single().unwrap();
        let (prior, prior_end) = prior_window("mtd", start, end);
        assert_eq!(prior.date_naive().to_string(), "2026-02-01");
        assert_eq!(prior_end.date_naive().to_string(), "2026-03-01");
        let (prior, prior_end) = prior_window("7d", end - Duration::days(6), end);
        assert_eq!(prior_end - prior, end - (end - Duration::days(6)));
    }

    #[tokio::test]
    async fn task_cohort_includes_failed_attempts_and_period_report_is_uncapped() {
        let directory = std::env::temp_dir().join(format!("hulahoop-cost-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Cost test',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'builder','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at, merged_at) values (?,?,?,?, 'CST-1',1,'Task','done',?,?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo)
            .bind(now - Duration::days(2)).bind(now).bind(now).execute(&db.pool).await.unwrap();
        for (index, status) in ["failed", "succeeded"].iter().enumerate() {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, base_bookmark, prompt_revision, artifact_dir, status, model, tokens_in, tokens_out, api_equivalent_usd, actual_cost_usd, started_at, finished_at, created_at)
                values (?,?,?,?, 'main','prompt','/tmp',?,?,?,?,?, ?,?,?,?)")
                .bind(Uuid::now_v7()).bind(issue).bind(repo).bind(agent)
                .bind(status).bind(if index == 0 { "model-y" } else { "model-x" })
                .bind(100i64).bind(20i64).bind(0.25f64).bind(0.25f64)
                .bind(now - Duration::minutes(10)).bind(now).bind(now).execute(&db.pool).await.unwrap();
        }
        let full = report(&db, workspace, "7d", None, None).await.unwrap().0;
        assert_eq!(full["current"]["runs"], 2);
        assert_eq!(full["tasks"]["tasks"], 1);
        assert_eq!(full["tasks"]["failedRuns"], 1);
        assert_eq!(full["tasks"]["runs"], 2);
        assert_eq!(full["tasks"]["equivalentUsd"], 0.5);
        assert_eq!(full["dailyModels"].as_array().unwrap().len(), 2);
        assert_eq!(full["failureReasons"][0]["equivalentUsd"], 0.25);
        assert_eq!(full["month"]["runs"], 2);
        // A post-merge rerun is period activity, but cannot rewrite completed-task effort.
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, base_bookmark, prompt_revision, artifact_dir, status, model, tokens_in, tokens_out, api_equivalent_usd, finished_at, created_at)
            values (?,?,?,?, 'main','prompt','/tmp','failed','late-model',100,20,9.0,?,?)")
            .bind(Uuid::now_v7()).bind(issue).bind(repo).bind(agent)
            .bind(now + Duration::seconds(1)).bind(now + Duration::seconds(1))
            .execute(&db.pool).await.unwrap();
        let revised = report(&db, workspace, "7d", None, None).await.unwrap().0;
        assert_eq!(revised["current"]["runs"], 3);
        assert_eq!(revised["tasks"]["runs"], 2);
        assert_eq!(revised["tasks"]["equivalentUsd"], 0.5);
        assert_eq!(
            report(&db, workspace, "7d", None, Some("late-model"))
                .await
                .unwrap()
                .0["tasks"]["tasks"],
            0
        );
        let empty_issue = Uuid::now_v7();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at, merged_at)
            values (?,?,?,?, 'CST-2',2,'No runs','done',?,?,?)")
            .bind(empty_issue).bind(workspace).bind(project).bind(repo)
            .bind(now - Duration::days(1)).bind(now).bind(now).execute(&db.pool).await.unwrap();
        assert_eq!(
            report(&db, workspace, "7d", None, None).await.unwrap().0["tasks"]["tasks"],
            2
        );
        let filtered = report(&db, workspace, "7d", Some(agent), Some("model-x"))
            .await
            .unwrap()
            .0;
        assert_eq!(filtered["current"]["runs"], 1);
        assert_eq!(filtered["tasks"]["tasks"], 1);
        assert_eq!(filtered["tasks"]["runs"], 2); // full effort for a touched task
        save_settings(&db, workspace, Some(150.0), Some(80.0))
            .await
            .unwrap();
        assert_eq!(
            report(&db, workspace, "mtd", None, None).await.unwrap().0["settings"]["apiBudgetUsd"],
            150.0
        );
        sqlx::query(
            "update runs set actual_model = 'model-z' where issue_id = ? and status = 'succeeded'",
        )
        .bind(issue)
        .execute(&db.pool)
        .await
        .unwrap();
        let observed = report(&db, workspace, "7d", None, Some("model-z"))
            .await
            .unwrap()
            .0;
        assert_eq!(observed["current"]["runs"], 1);
        assert_eq!(observed["tasks"]["runs"], 2); // all attempts on a selected task
        assert_eq!(observed["models"][0]["label"], "model-z");
        let gql_db = Db {
            pool: db.pool.clone(),
            data_dir: db.data_dir.clone(),
        };
        let schema =
            crate::gql::build_schema(std::sync::Arc::new(crate::engine::Engine::new(gql_db)));
        let result = schema
            .execute(format!(
                "query {{ costReport(workspaceId: \"{workspace}\", period: \"7d\") }}"
            ))
            .await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            result.data.into_json().unwrap()["costReport"]["tasks"]["tasks"],
            2
        );
        let mutation = schema.execute(format!("mutation {{ saveCostPlan(workspaceId: \"{workspace}\", connectorId: \"claude-sub\", label: \"Claude Max\", monthlyFeeUsd: 100.0, allowanceTokens: 16000000) }}")).await;
        assert!(mutation.errors.is_empty(), "{:?}", mutation.errors);
        assert_eq!(mutation.data.into_json().unwrap()["saveCostPlan"], true);
        assert_eq!(
            report(&db, workspace, "mtd", None, None).await.unwrap().0["plans"][0]
                ["allowanceTokens"],
            16_000_000
        );
        sqlx::query("update runs set connector_id = 'claude-sub' where issue_id = ? and status = 'succeeded'")
            .bind(issue).execute(&db.pool).await.unwrap();
        let with_plan_usage = report(&db, workspace, "mtd", None, None).await.unwrap().0;
        let subscription = with_plan_usage["planUsage"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["label"] == "claude-sub")
            .unwrap();
        assert_eq!(subscription["runs"], 1);
        assert_eq!(subscription["tokensIn"], 100);
        sqlx::query("update runs set status = 'timed_out', failure_category = 'timeout' where issue_id = ? and status = 'failed' and api_equivalent_usd = 0.25")
            .bind(issue).execute(&db.pool).await.unwrap();
        let with_timeout = report(&db, workspace, "7d", None, None).await.unwrap().0;
        assert_eq!(with_timeout["tasks"]["failedRuns"], 1);
        assert_eq!(with_timeout["current"]["failed"], 2); // timeout + post-merge failure
        assert_eq!(
            with_timeout["failureReasons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["label"] == "timeout")
                .unwrap()["equivalentUsd"],
            0.25
        );
        let queued = Uuid::now_v7();
        let artifact_dir = directory.join("queued");
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, base_bookmark, prompt_revision, artifact_dir, status, model, created_at)
            values (?, ?, ?, ?, 'main', 'prompt', ?, 'queued', 'model-x', ?)")
            .bind(queued).bind(issue).bind(repo).bind(agent).bind(artifact_dir.to_str().unwrap())
            .bind(now).execute(&db.pool).await.unwrap();
        let engine = crate::engine::Engine::new(Db {
            pool: db.pool.clone(),
            data_dir: directory.clone(),
        });
        let cancelled = engine.cancel_run(queued).await.unwrap();
        assert_eq!(cancelled.api_equivalent_usd, Some(0.0));
        assert_eq!(cancelled.actual_cost_usd, Some(0.0));
        assert_eq!(cancelled.cost_source.as_deref(), Some("never_started"));
        db.pool.close().await;
        let _ = std::fs::remove_dir_all(directory);
    }
}
