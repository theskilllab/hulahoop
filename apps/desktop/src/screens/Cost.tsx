import { useEffect, useState } from "react";
import {
  loadCostReport, saveCostPlan, saveCostSettings,
  type Agent, type CostReport, type CostTotals,
} from "../api";
import PageHeader from "../PageHeader";

const number = (value: number) => value.toLocaleString("en-US", { maximumFractionDigits: 1 });
const money = (value: number | null) => value === null ? "—" : `$${value.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: value < 1 ? 4 : 2 })}`;
const average = (sum: number, count: number) => count ? sum / count : null;
const time = (minutes: number | null) => minutes === null ? "—" : minutes >= 60 ? `${number(minutes / 60)}h` : `${number(minutes)}m`;
const percent = (numerator: number, denominator: number) => denominator ? `${number(100 * numerator / denominator)}%` : "—";
const periodName = { "7d": "Last 7 days", "30d": "Last 30 days", mtd: "Month to date" };
const modelColors = ["var(--accent)", "var(--brand)", "var(--ok)", "var(--warn)", "var(--text-2)", "var(--muted)"];

type Period = CostReport["period"];
type View = "chart" | "table";
type Measure = "cost" | "tokens";
const tokens = (row: CostTotals) => row.tokensIn + row.tokensOut;
const coveredCost = (sum: number, known: number, total: number) => total > 0 && known === total ? sum : null;
const emptyTotals: CostTotals = { runs: 0, succeeded: 0, failed: 0, cancelled: 0, tokensIn: 0,
  tokensOut: 0, cacheRead: 0, cacheWrite: 0, knownCostRuns: 0, equivalentUsd: 0,
  knownCashRuns: 0, cashUsd: 0, activeRuns: 0, activeMinutes: 0, knownTurnRuns: 0,
  turns: 0, knownToolRuns: 0, tools: 0 };

function Delta({ current, previous, suffix = "" }: { current: number | null; previous: number | null; suffix?: string }) {
  if (current === null || previous === null || previous === 0) return <small className="dim">Prior period —</small>;
  const change = (current - previous) / Math.abs(previous) * 100;
  return <small className="dim">{change >= 0 ? "+" : ""}{number(change)}% vs prior{suffix}</small>;
}

function Tile({ label, value, detail, previous, current }: { label: string; value: string; detail: string; previous?: number | null; current?: number | null }) {
  return <article className="cost-tile">
    <span className="label">{label}</span>
    <strong>{value}</strong>
    <span className="dim">{detail}</span>
    {previous !== undefined && current !== undefined ? <Delta current={current} previous={previous} /> : null}
  </article>;
}

export default function Cost({ workspaceId, agents, onOpen }: {
  workspaceId: string; agents: Agent[]; onOpen: (id: string) => void;
}) {
  const [period, setPeriod] = useState<Period>("30d");
  const [agent, setAgent] = useState("");
  const [model, setModel] = useState("");
  const [measure, setMeasure] = useState<Measure>("cost");
  const [trendView, setTrendView] = useState<View>("chart");
  const [failureView, setFailureView] = useState<View>("chart");
  const [report, setReport] = useState<CostReport | null>(null);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [editing, setEditing] = useState(false);
  const [budget, setBudget] = useState("");
  const [target, setTarget] = useState("");
  const [connector, setConnector] = useState("claude-sub");
  const [fee, setFee] = useState("");
  const [allowance, setAllowance] = useState("");

  useEffect(() => {
    let live = true;
    loadCostReport(workspaceId, period, agent || null, model || null)
      .then((data) => { if (live) { setReport(data); setError(""); } })
      .catch((reason: Error) => { if (live) setError(reason.message); });
    return () => { live = false; };
  }, [workspaceId, period, agent, model, refresh]);

  const save = async () => {
    const b = budget.trim() === "" ? null : Number(budget);
    const t = target.trim() === "" ? null : Number(target);
    if ((b !== null && (!Number.isFinite(b) || b < 0)) ||
      (t !== null && (!Number.isFinite(t) || t < 0 || t > 100))) {
      setError("Enter a nonnegative API budget and a success target from 0 to 100."); return;
    }
    try {
      await saveCostSettings(workspaceId, b, t);
      setEditing(false); setRefresh((value) => value + 1);
    } catch (reason) { setError((reason as Error).message); }
  };
  const savePlan = async () => {
    const cost = Number(fee), value = allowance.trim() ? Number(allowance) : null;
    if (!fee.trim() || !Number.isFinite(cost) || cost < 0 ||
      (value !== null && (!Number.isInteger(value) || value < 0 || value > 2_147_483_647))) {
      setError("Enter a valid monthly fee and optional allowance."); return;
    }
    try {
      const label = { "claude-sub": "Claude Max", pi: "Pi plan", "opencode-go": "OpenCode Go", "claude-api": "Claude API" }[connector] ?? connector;
      await saveCostPlan(workspaceId, connector, label, cost, value);
      setFee(""); setAllowance(""); setRefresh((previous) => previous + 1);
    } catch (reason) { setError((reason as Error).message); }
  };

  const c = report?.current, p = report?.previous, task = report?.tasks, prior = report?.priorTasks;
  const taskCost = task ? coveredCost(task.equivalentUsd, task.knownCostRuns, task.runs) : null;
  const priorCost = prior ? coveredCost(prior.equivalentUsd, prior.knownCostRuns, prior.runs) : null;
  const runCost = c ? coveredCost(c.equivalentUsd, c.knownCostRuns, c.runs) : null;
  const taskCash = task ? coveredCost(task.cashUsd, task.knownCashRuns, task.runs) : null;
  const runCash = c ? coveredCost(c.cashUsd, c.knownCashRuns, c.runs) : null;
  const dayModels = report?.dailyModels ?? [];
  const modelNames = [...new Set(dayModels.map((entry) => entry.model))];
  const days = report ? Array.from({ length: Math.ceil((Date.parse(report.end) - Date.parse(report.start)) / 86_400_000) }, (_, index) => {
    const label = new Date(Date.parse(report.start) + index * 86_400_000).toISOString().slice(0, 10);
    return report.daily.find((day) => day.label === label) ?? { ...emptyTotals, label };
  }) : [];
  const maxTrend = Math.max(1, ...days.map((entry) => measure === "tokens" ? tokens(entry) : entry.equivalentUsd));
  const maxFailure = Math.max(1, ...days.map((entry) => entry.failed));
  const selectedPlans = report?.plans ?? [];
  const planFees = selectedPlans.reduce((sum, plan) => sum + plan.monthlyFeeUsd, 0);
  const spending = c ? c.runs === 0 ? 0 : coveredCost(c.cashUsd, c.knownCashRuns, c.runs) : null;
  const unknownCosts = c ? c.runs - c.knownCostRuns : 0;
  const monthCash = report ? report.month.runs === 0 ? 0 : coveredCost(report.month.cashUsd, report.month.knownCashRuns, report.month.runs) : null;
  const budgetUsed = report?.settings.apiBudgetUsd && monthCash !== null ? monthCash / report.settings.apiBudgetUsd * 100 : null;
  const projectedCash = monthCash !== null && report && report.monthElapsedDays > 0 ? monthCash * report.monthTotalDays / report.monthElapsedDays : null;
  const failedCost = report?.failureReasons.every((row) => row.knownCostRuns === row.runs)
    ? report.failureReasons.reduce((sum, row) => sum + row.equivalentUsd, 0) : null;
  const plannedAllowance = selectedPlans.reduce((sum, plan) => sum + (plan.allowanceTokens ?? 0), 0);
  const pricedModels = report?.models.filter((row) => row.runs > 0 && row.knownCostRuns === row.runs) ?? [];
  const maxModelCost = Math.max(0.000001, ...pricedModels.map((row) => row.equivalentUsd / row.runs));
  const bestModel = pricedModels.filter((row) => row.runs >= 3 && row.succeeded > 0)
    .sort((a, b) => a.equivalentUsd / a.succeeded - b.equivalentUsd / b.succeeded)[0];

  return <section className="screen cost-control">
    <PageHeader title="Cost control" subtitle="Task efficiency · UTC periods" />
    <div className="cost-body">
      <div className="cost-filters" aria-label="Cost report filters">
        <label>Period <select value={period} onChange={(event) => setPeriod(event.target.value as Period)}>
          {Object.entries(periodName).map(([value, label]) => <option value={value} key={value}>{label}</option>)}
        </select></label>
        <label>Agent <select value={agent} onChange={(event) => setAgent(event.target.value)}>
          <option value="">All agents</option>
          {agents.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
        </select></label>
        <label>Model <select value={model} onChange={(event) => setModel(event.target.value)}>
          <option value="">All models</option>
          {[...new Set([...agents.map((item) => item.model), ...modelNames, model].filter(Boolean))].sort()
            .map((name) => <option value={name} key={name}>{name}</option>)}
        </select></label>
        <span className="dim">{report ? `${report.start.slice(0, 10)} – ${report.end.slice(0, 10)}` : "Loading…"}</span>
      </div>
      {error ? <p role="alert" className="cost-error">{error}</p> : null}
      {!report ? <p className="dim">Loading cost report…</p> : <>
        <div className="cost-section-heading"><h2>Task efficiency</h2><span>Completed = issue merged in this period; all its runs count, including failed and timed-out attempts.</span></div>
        <div className="cost-kpis">
          <Tile label="Completed tasks" value={number(task!.tasks)} detail={`${number(task!.runs)} runs across completed tasks`} current={task!.tasks} previous={prior!.tasks} />
          <Tile label="Runs / completed task" value={task!.tasks ? number(task!.runs / task!.tasks) : "—"} detail={`${number(task!.failedRuns)} failed or timed-out attempts included`} current={average(task!.runs, task!.tasks)} previous={average(prior!.runs, prior!.tasks)} />
          <Tile label="Time / completed task" value={time(average(task!.elapsedMinutes, task!.tasks))} detail="Created → merged, including waits" current={average(task!.elapsedMinutes, task!.tasks)} previous={average(prior!.elapsedMinutes, prior!.tasks)} />
          <Tile label="Agent time / task" value={task!.activeRuns === task!.runs ? time(average(task!.activeMinutes, task!.tasks)) : "—"} detail={`${number(task!.activeRuns)} / ${number(task!.runs)} timed attempts; includes retries`} />
          <Tile label="Agent time / run" value={time(average(c!.activeMinutes, c!.activeRuns))} detail={`${number(c!.activeRuns)} timed / ${number(c!.runs)} finished runs`} current={average(c!.activeMinutes, c!.activeRuns)} previous={average(p!.activeMinutes, p!.activeRuns)} />
          <Tile label="API equiv. / completed task" value={money(taskCost !== null ? average(taskCost, task!.tasks) : null)} detail={`${number(task!.knownCostRuns)} / ${number(task!.runs)} priced task runs`} current={taskCost !== null ? average(taskCost, task!.tasks) : null} previous={priorCost !== null ? average(priorCost, prior!.tasks) : null} />
          <Tile label="API equiv. / run" value={money(runCost !== null ? average(runCost, c!.runs) : null)} detail={`${number(c!.knownCostRuns)} / ${number(c!.runs)} priced period runs`} current={runCost !== null ? average(runCost, c!.runs) : null} previous={coveredCost(p!.equivalentUsd, p!.knownCostRuns, p!.runs) !== null ? average(p!.equivalentUsd, p!.runs) : null} />
          <Tile label="Cash / completed task" value={money(taskCash !== null ? average(taskCash, task!.tasks) : null)} detail={`${number(task!.knownCashRuns)} / ${number(task!.runs)} billed attempts; excludes plan fees`} />
          <Tile label="Cash / run" value={money(runCash !== null ? average(runCash, c!.runs) : null)} detail={`${number(c!.knownCashRuns)} / ${number(c!.runs)} billed runs; excludes plan fees`} />
          <Tile label="Model turns / task" value={task!.knownTurnRuns === task!.runs ? (average(task!.turns, task!.tasks) === null ? "—" : number(task!.turns / task!.tasks)) : "—"} detail={`${number(task!.knownTurnRuns)} / ${number(task!.runs)} measured runs`} />
          <Tile label="Tool calls / task" value={task!.knownToolRuns === task!.runs ? (average(task!.tools, task!.tasks) === null ? "—" : number(task!.tools / task!.tasks)) : "—"} detail={`${number(task!.knownToolRuns)} / ${number(task!.runs)} measured runs`} />
        </div>

        <div className="cost-section-heading"><h2>Spend & reliability</h2><span>Activity uses run finish time; task metrics use merge time.</span></div>
        <div className="cost-kpis cost-kpis-small">
          <Tile label="API cash / period" value={money(spending)} detail={`${number(c!.knownCashRuns)} / ${number(c!.runs)} runs with known billing; plan fees below`} />
          <Tile label="Tokens" value={number(tokens(c!))} detail={`${number(c!.cacheRead)} cache read · ${number(c!.cacheWrite)} cache write`} current={tokens(c!)} previous={tokens(p!)} />
          <Tile label="Run success" value={percent(c!.succeeded, c!.runs)} detail={`${number(c!.succeeded)} successful / ${number(c!.runs)} finished`} current={average(c!.succeeded, c!.runs)} previous={average(p!.succeeded, p!.runs)} />
          <Tile label="Failed / timed out" value={number(c!.failed)} detail={`${money(failedCost)} API equiv. wasted · ${report.unsuccessfulTasks} tasks with failed / timed-out runs still open`} current={c!.failed} previous={p!.failed} />
        </div>
        {unknownCosts > 0 ? <p className="cost-note">{number(unknownCosts)} finished runs have unknown API-equivalent cost. Totals and per-task rates display — until every contributing run has a price. Catalogue estimates exclude cache; cash follows a known billing source.</p> : null}

        <section className="cost-panel">
          <header className="cost-panel-head"><div><h2>Usage over time</h2><p>Daily finish-date totals, stacked by provider-observed model when available</p></div>
            <div className="cost-actions"><div className="seg" aria-label="Usage measure">
              {(["cost", "tokens"] as Measure[]).map((value) => <button key={value} aria-pressed={measure === value} onClick={() => setMeasure(value)}>{value === "cost" ? "Cost" : "Tokens"}</button>)}
            </div><div className="seg" aria-label="Usage display">
              {(["chart", "table"] as View[]).map((value) => <button key={value} aria-pressed={trendView === value} onClick={() => setTrendView(value)}>{value}</button>)}
            </div></div></header>
          {trendView === "chart" ? <div className="cost-bars" role="img" aria-label={`Daily ${measure === "cost" ? "known API-equivalent subtotal" : "tokens"} by model`}>
            {days.map((day) => <div className="cost-bar-col" key={day.label} title={`${day.label}: ${measure === "cost" ? `${money(day.equivalentUsd)} known API equivalent${day.knownCostRuns < day.runs ? ` (${day.knownCostRuns}/${day.runs} priced runs)` : ""}` : number(tokens(day))}`}>
              <div className="cost-bar-track">{modelNames.map((name, index) => {
                const row = dayModels.find((entry) => entry.day === day.label && entry.model === name);
                const value = row ? measure === "cost" ? row.equivalentUsd : tokens(row) : 0;
                return <span key={name} style={{ height: `${value / maxTrend * 100}%`, background: modelColors[index % modelColors.length] }} />;
              })}</div><small>{day.label.slice(5)}</small>
            </div>)}
            {!c?.runs ? <p className="dim">No finished runs in this period.</p> : null}
          </div> : <div className="cost-scroll"><table className="table"><thead><tr><th>Day · model</th><th>Runs</th><th>Tokens</th><th>API equivalent</th><th>Failed</th></tr></thead>
            <tbody>{dayModels.map((entry) => <tr key={`${entry.day}-${entry.model}`}><td>{entry.day} · {entry.model}</td><td>{entry.runs}</td><td>{number(tokens(entry))}</td><td>{entry.knownCostRuns === entry.runs ? money(entry.equivalentUsd) : "—"}</td><td>{entry.failed}</td></tr>)}</tbody></table></div>}
          {measure === "cost" && unknownCosts > 0 ? <p className="cost-note">Cost bars show known API-equivalent subtotals; {unknownCosts} runs lack prices.</p> : null}
          <div className="cost-legend">{modelNames.map((name, index) => <span key={name}><i style={{ background: modelColors[index % modelColors.length] }} />{name}</span>)}</div>
        </section>

        <div className="cost-two-columns">
          <section className="cost-panel"><h2>Agents · run activity</h2><div className="cost-scroll"><table className="table"><thead><tr><th>Agent</th><th>Runs</th><th>Success</th><th>Agent min / run</th><th>API equiv.</th></tr></thead>
            <tbody>{report.agents.map((row) => <tr key={row.label}><td>{row.label}</td><td>{row.runs}</td><td>{percent(row.succeeded, row.runs)}</td><td>{time(average(row.activeMinutes, row.activeRuns))}</td><td>{row.knownCostRuns === row.runs ? money(row.equivalentUsd) : "—"}</td></tr>)}</tbody></table></div></section>
          <section className="cost-panel"><h2>Models · effort & efficiency</h2><div className="cost-scroll"><table className="table"><thead><tr><th>Model</th><th>Runs</th><th>Turns / run</th><th>Tools / run</th><th>Success</th><th>API equiv. / run</th></tr></thead>
            <tbody>{report.models.map((row) => <tr key={row.label}><td>{row.label}</td><td>{row.runs}</td><td>{row.knownTurnRuns === row.runs ? number(average(row.turns, row.runs) ?? 0) : "—"}</td><td>{row.knownToolRuns === row.runs ? number(average(row.tools, row.runs) ?? 0) : "—"}</td><td>{percent(row.succeeded, row.runs)}</td><td>{row.knownCostRuns === row.runs ? money(average(row.equivalentUsd, row.runs)) : "—"}</td></tr>)}</tbody></table></div></section>
        </div>

        <section className="cost-panel"><header className="cost-panel-head"><div><h2>Model trade-offs</h2><p>Observed model (assigned fallback) · run success vs API-equivalent cost per run; bubble size reflects sample size</p></div></header>
          <div className="cost-scatter" role="img" aria-label="Model run success rate by cost per run">
            {pricedModels.map((row, index) => <span key={row.label} title={`${row.label}: ${percent(row.succeeded, row.runs)} success, ${money(row.equivalentUsd / row.runs)} per run, ${row.runs} runs`}
              style={{ left: `${5 + (row.equivalentUsd / row.runs / maxModelCost) * 88}%`, bottom: `${5 + row.succeeded / row.runs * 85}%`, width: Math.min(28, 10 + row.runs), height: Math.min(28, 10 + row.runs), background: modelColors[index % modelColors.length] }} />)}
            <small className="cost-scatter-y">Higher success ↑</small><small className="cost-scatter-x">Higher cost / run →</small>
          </div>
          <p className="dim cost-verdict">{bestModel ? `Lowest observed cost per successful run (≥3 runs): ${bestModel.label} · ${money(bestModel.equivalentUsd / bestModel.succeeded)}. Compare comparable tasks before changing models.` : "A model needs at least 3 fully priced runs and a successful run for a value comparison."}</p>
          <div className="cost-scroll"><table className="table"><thead><tr><th>Model</th><th>Runs</th><th>Success</th><th>API equiv. / success</th></tr></thead><tbody>{report.models.map((row) => <tr key={row.label}><td>{row.label}</td><td>{row.runs}</td><td>{percent(row.succeeded, row.runs)}</td><td>{row.knownCostRuns === row.runs && row.succeeded ? money(row.equivalentUsd / row.succeeded) : "—"}</td></tr>)}</tbody></table></div>
        </section>

        <section className="cost-panel"><header className="cost-panel-head"><div><h2>Failures</h2><p>Failed and timed-out runs and their known API-equivalent spend</p></div>
          <div className="seg" aria-label="Failure display">{(["chart", "table"] as View[]).map((value) => <button key={value} aria-pressed={failureView === value} onClick={() => setFailureView(value)}>{value}</button>)}</div></header>
          {failureView === "chart" ? <div className="cost-bars cost-failure-bars" role="img" aria-label="Failed or timed-out runs per day">
            {days.map((day) => <div className="cost-bar-col" key={day.label} title={`${day.label}: ${day.failed} failed runs`}><div className="cost-bar-track"><span style={{ height: `${day.failed / maxFailure * 100}%`, background: "var(--bad)" }} /></div><small>{day.label.slice(5)}</small></div>)}
          </div> : <div className="cost-scroll"><table className="table"><thead><tr><th>Day</th><th>Failed</th><th>All runs</th></tr></thead><tbody>{days.map((day) => <tr key={day.label}><td>{day.label}</td><td>{day.failed}</td><td>{day.runs}</td></tr>)}</tbody></table></div>}
          <div className="cost-reasons"><strong>Reasons</strong>{report.failureReasons.map((row) => <span key={row.label}>{row.label.replaceAll("_", " ")} · {row.failed}</span>)}</div>
          <h3>Recent failed / timed-out runs</h3><div className="cost-scroll"><table className="table"><thead><tr><th>Task</th><th>Agent · model</th><th>Reason</th><th>Finished</th><th>API equiv.</th></tr></thead><tbody>
            {report.failures.map((run) => <tr key={run.id}><td><button type="button" className="cost-task-link" onClick={() => onOpen(run.issueId)}>{run.issueKey} · {run.issueTitle}</button></td><td>{run.agent} · {run.model}</td><td>{run.reason.replaceAll("_", " ")}</td><td>{run.finishedAt.slice(0, 16).replace("T", " ")}</td><td>{money(run.apiEquivalentUsd)}</td></tr>)}
          </tbody></table>{!report.failures.length ? <p className="dim">No failed or timed-out runs in this period.</p> : null}</div>
        </section>

        <section className="cost-panel"><header className="cost-panel-head"><div><h2>Budget & plans</h2><p>Advisory monthly limits · USD</p></div><button className="ghost small" onClick={() => { setBudget(report.settings.apiBudgetUsd?.toString() ?? ""); setTarget(report.settings.successTargetPercent?.toString() ?? ""); setEditing(!editing); }}>Edit targets</button></header>
          {editing ? <div className="cost-edit"><label>Monthly API budget <input type="number" min="0" step="0.01" value={budget} onChange={(event) => setBudget(event.target.value)} placeholder="Unset" /></label>
            <label>Run success target % <input type="number" min="0" max="100" step="1" value={target} onChange={(event) => setTarget(event.target.value)} placeholder="Unset" /></label>
            <button onClick={save}>Save targets</button></div> : null}
          <div className="cost-budget"><div><span className="label">API cash · month to date</span><strong>{money(monthCash)} / {money(report.settings.apiBudgetUsd)}</strong>
            <p className="dim">{report.month.knownCashRuns} / {report.month.runs} runs with measured API cash or subscription billing. Workspace-wide.</p>
            {budgetUsed !== null ? <div className="meter" role="meter" aria-label="Monthly API budget used" aria-valuenow={Math.min(100, budgetUsed)} aria-valuemin={0} aria-valuemax={100}><span style={{ width: `${Math.min(100, budgetUsed)}%` }} /></div> : null}</div>
            <div><span className="label">Projected / month</span><strong>{money(projectedCash)} API + {money(planFees)} plans</strong><p className="dim">{projectedCash === null ? "Awaiting measured cash across all runs." : `${money(projectedCash + planFees)} projected total at current daily pace.`} · {plannedAllowance ? `${number(plannedAllowance)} estimated plan tokens / month` : "Plan allowance unset."}</p></div>
            <div><span className="label">Run success target</span><strong>{percent(c!.succeeded, c!.runs)} / {report.settings.successTargetPercent === null ? "—" : `${report.settings.successTargetPercent}%`}</strong><p className="dim">Target uses the current period's run success rate.</p></div></div>
          <div className="cost-plans">{selectedPlans.map((plan) => {
            const usage = report.planUsage.find((row) => row.label === plan.connectorId);
            const used = usage ? tokens(usage) + usage.cacheRead + usage.cacheWrite : 0;
            return <span key={plan.connectorId}>{plan.label}: {money(plan.monthlyFeeUsd)}/mo · {number(used)} tokens used / {plan.allowanceTokens === null ? "—" : number(plan.allowanceTokens)} estimated allowance</span>;
          })}</div>
          <div className="cost-edit"><label>Plan <select value={connector} onChange={(event) => setConnector(event.target.value)}><option value="claude-sub">Claude Max</option><option value="opencode-go">OpenCode Go</option><option value="pi">Pi plan</option><option value="claude-api">Claude API</option></select></label>
            <label>Monthly fee <input type="number" min="0" step="0.01" value={fee} onChange={(event) => setFee(event.target.value)} placeholder="USD" /></label>
            <label>Est. tokens / month <input type="number" min="0" step="1" value={allowance} onChange={(event) => setAllowance(event.target.value)} placeholder="Optional" /></label>
            <button onClick={savePlan}>Save plan</button></div>
        </section>
      </>}
    </div>
  </section>;
}
