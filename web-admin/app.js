import { createApiClient, normalizeAdminToken } from "./api-client.js";

const state = {
  token: "",
  agents: [],
  selectedAgentId: "",
  lastJobId: "",
  createdEnrollmentToken: null,
};

const TELEMETRY_WINDOW_MS = 5 * 60 * 1000;
const TELEMETRY_PAGE_LIMIT = 120;

const api = createApiClient({
  tokenProvider: () => state.token,
  formatError: formatApiError,
});

const METRICS_CHARTS = [
  {
    label: "CPU used",
    unit: "%",
    read: (body) => asNumber(body?.cpu?.usage_percent),
  },
  {
    label: "Memory used",
    unit: "%",
    read: (body) => memoryUsedPercent(body),
  },
  {
    label: "Disk used",
    unit: "%",
    read: (body) => diskUsedPercent(body),
  },
  {
    label: "Processes",
    unit: "",
    read: (body) => asNumber(body?.process?.count),
  },
  {
    label: "Failed units",
    unit: "",
    read: (body) => asNumber(body?.service?.failed_units_count),
  },
];

export function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export function renderAgents(agents, selectedAgentId = "") {
  if (!Array.isArray(agents) || agents.length === 0) {
    return '<div class="empty">No agents enrolled.</div>';
  }
  return agents
    .map((agent) => {
      const labels = Array.isArray(agent.labels)
        ? agent.labels.map((label) => `${label.key}=${label.value}`).join(", ")
        : "";
      const platform = [agent.os, agent.arch].filter(Boolean).join("/");
      const age =
        typeof agent.last_seen_age_seconds === "number"
          ? `last seen ${agent.last_seen_age_seconds}s ago`
          : "";
      const meta = [agent.hostname, platform, age].filter(Boolean).join(" · ");
      const selectedClass = agent.id === selectedAgentId ? " selected" : "";
      const status = agent.revoked ? "offline" : agent.status || "unknown";
      const revokedBadge = agent.revoked
        ? '<span class="status-pill revoked">revoked</span>'
        : "";
      return `
        <button class="agent-row${selectedClass}" type="button" data-agent-id="${escapeHtml(agent.id)}">
          <span>
            <strong>${escapeHtml(agent.name || agent.id)}</strong>
            <small>${escapeHtml(agent.id)}</small>
          </span>
          <span class="agent-status">
            <span class="status-pill ${escapeHtml(status)}">${escapeHtml(status)}</span>
            ${revokedBadge}
          </span>
          <small class="labels">${escapeHtml(labels || "no labels")}</small>
          <small class="agent-meta">${escapeHtml(meta || "no facts summary")}</small>
        </button>
      `;
    })
    .join("");
}

export function renderSnapshot(snapshot, missingText) {
  if (!snapshot || !snapshot.body) {
    return missingText;
  }
  const agentTime = formatUnixMillis(snapshot.agent_system_time_ms);
  const collectedAt = formatUnixMillis(snapshot.collected_at_ms);
  const header = [
    agentTime ? `Agent time: ${agentTime}` : "",
    collectedAt ? `Stored at: ${collectedAt}` : "",
  ].filter(Boolean);
  const body = JSON.stringify(snapshot.body, null, 2);
  return header.length > 0 ? `${header.join("\n")}\n\n${body}` : body;
}

export function renderFactsInventory(snapshot, missingText = "No facts snapshot.") {
  if (!snapshot || !snapshot.body) {
    return `<div class="empty">${escapeHtml(missingText)}</div>`;
  }
  const body = snapshot.body;
  const rows = [
    ["OS", [body.os, body.arch].filter(Boolean).join("/")],
    ["Hostname", body.hostname],
    ["CPU logical cores", body?.cpu?.logical_count],
    ["Memory total", formatKilobytes(body?.memory?.total_kb)],
    [
      "Memory modules",
      body?.memory?.module_count_known === false
        ? "unknown"
        : formatOptional(body?.memory?.module_count),
    ],
    ["Root disk total", formatKilobytes(body?.disk?.root_total_kb)],
    ["Root filesystem", body?.disk?.root_filesystem],
    [
      "Network interfaces",
      Array.isArray(body?.network?.interfaces)
        ? String(body.network.interfaces.length)
        : "unknown",
    ],
  ];
  return `
    <div class="inventory-grid">
      ${rows
        .map(
          ([label, value]) => `
            <div class="inventory-card">
              <small>${escapeHtml(label)}</small>
              <strong>${escapeHtml(formatOptional(value))}</strong>
            </div>
          `,
        )
        .join("")}
    </div>
  `;
}

export function renderDrift(report) {
  if (!report) {
    return '<div class="empty">No drift report.</div>';
  }
  const agentTime = formatUnixMillis(report.agent_system_time_ms);
  const checkedAt = formatUnixMillis(report.checked_at_ms);
  const timeMeta = [agentTime ? `Agent time ${agentTime}` : "", checkedAt ? `Checked ${checkedAt}` : ""]
    .filter(Boolean)
    .join(" | ");
  return `
    <div class="drift-summary">
      <span class="status-pill ${escapeHtml(report.status)}">${escapeHtml(report.status)}</span>
      <strong>${escapeHtml(report.policy_name)}</strong>
    </div>
    ${timeMeta ? `<div class="snapshot-time">${escapeHtml(timeMeta)}</div>` : ""}
    <div class="diff-grid">
      <section>
        <h3>Expected</h3>
        <pre>${escapeHtml(report.expected)}</pre>
      </section>
      <section>
        <h3>Actual</h3>
        <pre>${escapeHtml(report.actual)}</pre>
      </section>
    </div>
  `;
}

export function formatUnixMillis(value) {
  if (!Number.isFinite(value)) {
    return "";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return `${date.toISOString()} (${value} ms)`;
}

export function asNumber(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

export function percent(part, total) {
  const partNumber = asNumber(part);
  const totalNumber = asNumber(total);
  if (partNumber === null || totalNumber === null || totalNumber <= 0) {
    return null;
  }
  return (partNumber / totalNumber) * 100;
}

export function diskUsedPercent(body) {
  const direct = asNumber(body?.disk?.used_percent);
  if (direct !== null) {
    return direct;
  }
  const used = asNumber(body?.disk?.used_kb);
  const total = asNumber(body?.disk?.total_kb);
  if (used === null || total === null || total <= 0) {
    return null;
  }
  return (used / total) * 100;
}

export function memoryUsedPercent(body) {
  const direct = asNumber(body?.memory?.used_percent);
  if (direct !== null) {
    return direct;
  }
  return percent(body?.memory?.used_kb, body?.memory?.total_kb);
}

export function formatKilobytes(value) {
  const kilobytes = asNumber(value);
  if (kilobytes === null) {
    return "unknown";
  }
  if (kilobytes >= 1024 * 1024) {
    return `${(kilobytes / 1024 / 1024).toFixed(1)} GiB`;
  }
  if (kilobytes >= 1024) {
    return `${(kilobytes / 1024).toFixed(1)} MiB`;
  }
  return `${kilobytes} KiB`;
}

function formatOptional(value) {
  return value === null || value === undefined || value === "" ? "unknown" : String(value);
}

export function snapshotTimeMs(snapshot) {
  return (
    asNumber(snapshot?.agent_system_time_ms) ??
    asNumber(snapshot?.collected_at_ms) ??
    asNumber(snapshot?.checked_at_ms) ??
    asNumber(snapshot?.body?.system_time_ms)
  );
}

export function recentSnapshots(snapshots, windowMs = TELEMETRY_WINDOW_MS) {
  const rows = (Array.isArray(snapshots) ? snapshots : [])
    .map((snapshot) => ({ snapshot, time: snapshotTimeMs(snapshot) }))
    .filter((row) => row.time !== null)
    .sort((left, right) => left.time - right.time);
  if (rows.length === 0) {
    return [];
  }
  const newest = rows[rows.length - 1].time;
  const cutoff = newest - windowMs;
  return rows.filter((row) => row.time >= cutoff).map((row) => row.snapshot);
}

export function renderTelemetryCharts(snapshots, definitions, emptyText = "No recent telemetry snapshots.") {
  const recent = recentSnapshots(snapshots);
  if (recent.length === 0) {
    return `<div class="empty">${escapeHtml(emptyText)}</div>`;
  }
  const first = snapshotTimeMs(recent[0]);
  const last = snapshotTimeMs(recent[recent.length - 1]);
  const range = [formatClock(first), formatClock(last)].filter(Boolean).join(" - ");
  const cards = definitions
    .map((definition) => renderChartCard(recent, definition))
    .join("");
  return `
    <div class="chart-window">
      <strong>Last 5 minutes</strong>
      <small>${escapeHtml(range || `${recent.length} sample(s)`)}</small>
    </div>
    <div class="chart-grid">${cards}</div>
  `;
}

function renderChartCard(snapshots, definition) {
  const series = snapshots
    .map((snapshot) => ({
      time: snapshotTimeMs(snapshot),
      value: definition.read(snapshot.body || {}),
    }))
    .filter((point) => point.time !== null && point.value !== null);
  const latest = series.length > 0 ? series[series.length - 1].value : null;
  const latestLabel = latest === null ? "No samples" : formatMetricValue(latest, definition.unit);
  return `
    <article class="chart-card">
      <div class="chart-card-heading">
        <span>${escapeHtml(definition.label)}</span>
        <strong>${escapeHtml(latestLabel)}</strong>
      </div>
      ${renderSparkline(series)}
    </article>
  `;
}

function renderSparkline(series) {
  if (series.length === 0) {
    return '<div class="chart-empty">No samples</div>';
  }
  const width = 300;
  const height = 96;
  const padX = 10;
  const padY = 14;
  const values = series.map((point) => point.value);
  let min = Math.min(...values);
  let max = Math.max(...values);
  if (min === max) {
    min -= 1;
    max += 1;
  }
  const points = series
    .map((point, index) => {
      const x =
        series.length === 1 ? width / 2 : padX + (index * (width - padX * 2)) / (series.length - 1);
      const ratio = (point.value - min) / (max - min);
      const y = height - padY - ratio * (height - padY * 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return `
    <svg class="sparkline" viewBox="0 0 ${width} ${height}" role="img" aria-label="Telemetry trend">
      <line class="sparkline-axis" x1="${padX}" y1="${height - padY}" x2="${width - padX}" y2="${height - padY}"></line>
      <polyline class="sparkline-line" points="${points}"></polyline>
    </svg>
  `;
}

function formatMetricValue(value, unit) {
  if (unit === "%") {
    return `${value.toFixed(1)}%`;
  }
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function formatClock(value) {
  const number = asNumber(value);
  if (number === null) {
    return "";
  }
  const date = new Date(number);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return date.toISOString().slice(11, 19);
}

export function renderAudit(events) {
  if (!Array.isArray(events) || events.length === 0) {
    return '<div class="empty">No audit events.</div>';
  }
  return events
    .map(
      (event) => `
        <div class="audit-row">
          <span class="status-pill ${escapeHtml(event.category)}">${escapeHtml(event.category)}</span>
          <strong>${escapeHtml(event.action)}</strong>
          <small>${escapeHtml(event.actor)} -> ${escapeHtml(event.target)}</small>
          <code>${escapeHtml(event.value_kind)}:${escapeHtml(event.value)}</code>
        </div>
      `,
    )
    .join("");
}

export function renderJobs(jobs) {
  if (!Array.isArray(jobs) || jobs.length === 0) {
    return '<div class="empty">No jobs created.</div>';
  }
  return jobs
    .map((job) => {
      const command = [job.command_program, ...(job.command_args || [])].filter(Boolean).join(" ");
      return `
        <button class="job-row" type="button" data-job-id="${escapeHtml(job.id)}">
          <span>
            <strong>${escapeHtml(job.id)}</strong>
            <small>${escapeHtml(command || "non-command job")}</small>
          </span>
          <span class="status-pill ${escapeHtml(job.status || "unknown")}">${escapeHtml(job.status || "unknown")}</span>
          <small>${escapeHtml(job.target_count ?? 0)} target(s)</small>
        </button>
      `;
    })
    .join("");
}

export function renderEnrollmentTokens(tokens) {
  if (!Array.isArray(tokens) || tokens.length === 0) {
    return '<div class="empty">No enrollment tokens created.</div>';
  }
  return tokens
    .map((token) => {
      const revokedClass = token.revoked ? " revoked" : "";
      const expiresAt = token.expires_at_epoch
        ? new Date(token.expires_at_epoch * 1000).toLocaleString()
        : "unknown";
      const labels = token.default_labels || "no default labels";
      const remaining = token.remaining_uses ?? Math.max((token.max_uses ?? 0) - (token.used_count ?? 0), 0);
      return `
        <div class="token-row${revokedClass}">
          <span>
            <strong>${escapeHtml(token.id)}</strong>
            <small>${escapeHtml(labels)}</small>
          </span>
          <span class="status-pill ${token.revoked ? "revoked" : "active"}">${token.revoked ? "revoked" : "active"}</span>
          <small>${escapeHtml(remaining)} of ${escapeHtml(token.max_uses ?? 0)} use(s) left</small>
          <small>expires ${escapeHtml(expiresAt)}</small>
          <button type="button" data-revoke-token-id="${escapeHtml(token.id)}" ${token.revoked ? "disabled" : ""}>Revoke</button>
        </div>
      `;
    })
    .join("");
}

export function renderCreatedEnrollmentToken(result, controllerUrl = "", agentName = "") {
  if (!result || !result.token) {
    return "Create a token to show the one-time value here.";
  }
  const url = controllerUrl || "https://fleet.example.com";
  const name = agentName || "agent-01";
  return [
    "One-time token:",
    result.token,
    "",
    "Agent init command:",
    `sponzey agent init --url ${url} --token ${result.token} --name ${name}`,
  ].join("\n");
}

export function buildEnrollmentTokenRequest({ labels, maxUses, expiresInSeconds }) {
  const max_uses = Number.parseInt(maxUses, 10);
  const expires_in_seconds = Number.parseInt(expiresInSeconds, 10);
  if (!Number.isInteger(max_uses) || max_uses < 1) {
    throw new Error("Max uses must be at least 1.");
  }
  if (!Number.isInteger(expires_in_seconds) || expires_in_seconds < 1) {
    throw new Error("Expiry must be at least 1 second.");
  }
  return {
    labels: String(labels ?? "").trim(),
    max_uses,
    expires_in_seconds,
  };
}

export function parseCommandArgs(value) {
  return String(value ?? "")
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean);
}

export function buildCommandJobRequest({ agentId, program, args, confirmed }) {
  if (!agentId) {
    throw new Error("Select an agent from the Agents list before running a command.");
  }
  if (!program || !String(program).trim()) {
    throw new Error("Enter a program to run, for example uptime.");
  }
  if (!confirmed) {
    throw new Error("Check Confirm high-risk execution before running the command.");
  }
  const jobId = `job-ui-${Date.now()}`;
  return {
    job_id: jobId,
    target_agent_ids: [agentId],
    program: String(program).trim(),
    args: Array.isArray(args) ? args : parseCommandArgs(args),
    timeout_seconds: 30,
    confirmed_high_risk: true,
    confirmed_by: "web-admin",
    expires_in_seconds: 60,
    nonce_prefix: jobId,
  };
}

export function renderJobOutput(chunks) {
  if (!Array.isArray(chunks) || chunks.length === 0) {
    return "No job output.";
  }
  return chunks
    .map((chunk) => {
      const prefix = `${chunk.agent_id || "agent"} ${chunk.stream || "stdout"}`;
      return `[${prefix}] ${chunk.data || ""}`;
    })
    .join("");
}

export function formatApiError(path, status) {
  if (status === 401 || status === 403) {
    return "Controller rejected this request. Check the admin token and permissions.";
  }
  return `${path} returned ${status}`;
}

function setStatus(message, kind = "") {
  const element = document.querySelector("#status");
  element.textContent = message;
  element.className = `status ${kind}`.trim();
}

function readAdminTokenInput() {
  return normalizeAdminToken(document.querySelector("#admin-token")?.value || "");
}

function syncAdminTokenFromInput({ requireToken = false } = {}) {
  const token = readAdminTokenInput();
  if (token) {
    state.token = token;
  }
  if (requireToken && !state.token) {
    throw new Error("Admin token is required. Paste the token from controller init, then retry.");
  }
  return state.token;
}

async function loadAgents() {
  const agents = await api.listAgents();
  state.agents = Array.isArray(agents) ? agents : [];
  const selected = state.agents.some((agent) => agent.id === state.selectedAgentId)
    ? state.selectedAgentId
    : state.agents[0]?.id || "";
  state.selectedAgentId = selected;
  document.querySelector("#agent-count").textContent = `${state.agents.length} known`;
  document.querySelector("#agents-list").innerHTML = renderAgents(state.agents, selected);
  syncAgentActions();
  if (selected) {
    await refreshSelectedAgent();
  }
}

function handleAgentsListClick(event) {
  const button = event.target?.closest?.("[data-agent-id]");
  if (!button?.dataset?.agentId) {
    return;
  }
  state.selectedAgentId = button.dataset.agentId;
  document.querySelector("#agents-list").innerHTML = renderAgents(state.agents, state.selectedAgentId);
  syncAgentActions();
  refreshSelectedAgent().catch((error) => setStatus(error.message, "error"));
}

function selectedAgent() {
  return state.agents.find((agent) => agent.id === state.selectedAgentId) || null;
}

function syncAgentActions() {
  const revokeButton = document.querySelector("#revoke-agent-key");
  const refreshButton = document.querySelector("#refresh-telemetry");
  const agent = selectedAgent();
  if (revokeButton) {
    revokeButton.disabled = !agent || Boolean(agent.revoked);
  }
  if (refreshButton) {
    refreshButton.disabled = !agent;
  }
}

async function refreshSelectedAgent() {
  const agentId = state.selectedAgentId;
  if (!agentId) {
    return;
  }
  const [facts, metrics, drift, metricsHistory] = await Promise.all([
    readOptionalAgentData("facts", () => api.getLatestFacts(agentId)),
    readOptionalAgentData("metrics", () => api.getLatestMetrics(agentId)),
    readOptionalAgentData("drift", () => api.getLatestDrift(agentId)),
    readOptionalAgentData("metrics history", () => api.listMetrics(agentId, { limit: TELEMETRY_PAGE_LIMIT })),
  ]);
  document.querySelector("#facts-panel").textContent = renderSnapshot(
    facts.value,
    facts.error || "No facts snapshot.",
  );
  document.querySelector("#metrics-panel").textContent = renderSnapshot(
    metrics.value,
    metrics.error || "No metrics snapshot.",
  );
  document.querySelector("#facts-chart").innerHTML = facts.error
    ? `<div class="empty">${escapeHtml(facts.error)}</div>`
    : renderFactsInventory(facts.value, "No facts snapshot.");
  document.querySelector("#metrics-chart").innerHTML = metricsHistory.error
    ? `<div class="empty">${escapeHtml(metricsHistory.error)}</div>`
    : renderTelemetryCharts(snapshotItems(metricsHistory.value), METRICS_CHARTS, "No metrics snapshots in the recent window.");
  document.querySelector("#drift-panel").innerHTML = drift.error
    ? `<div class="empty">${escapeHtml(drift.error)}</div>`
    : renderDrift(drift.value);
}

function snapshotItems(page) {
  return Array.isArray(page?.items) ? page.items : [];
}

async function readOptionalAgentData(label, load) {
  try {
    return { value: await load(), error: "" };
  } catch (error) {
    return {
      value: null,
      error: `Could not load ${label}. Refresh or check controller logs.`,
    };
  }
}

async function refreshAll() {
  if (!state.token) {
    setStatus("Admin token is required.", "error");
    return;
  }
  setStatus("Loading controller data...");
  await loadAgents();
  const [jobs, audit, enrollmentTokens] = await Promise.all([
    api.listJobs(),
    api.listAudit(),
    api.listEnrollmentTokens(),
  ]);
  document.querySelector("#jobs-list").innerHTML = renderJobs(jobs);
  document.querySelectorAll("[data-job-id]").forEach((button) => {
    button.addEventListener("click", () => {
      state.lastJobId = button.dataset.jobId;
      pollJobOutput(state.lastJobId).catch((error) => setStatus(error.message, "error"));
    });
  });
  document.querySelector("#enrollment-tokens-list").innerHTML = renderEnrollmentTokens(enrollmentTokens);
  document.querySelectorAll("[data-revoke-token-id]").forEach((button) => {
    button.addEventListener("click", () => {
      revokeEnrollmentToken(button.dataset.revokeTokenId).catch((error) => setStatus(error.message, "error"));
    });
  });
  document.querySelector("#audit-list").innerHTML = renderAudit(audit);
  setStatus("Loaded latest controller data.", "ok");
}

async function submitEnrollmentToken(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const request = buildEnrollmentTokenRequest({
    labels: data.get("labels"),
    maxUses: data.get("max-uses"),
    expiresInSeconds: data.get("expires-in-seconds"),
  });
  const response = await api.createEnrollmentToken(request);
  state.createdEnrollmentToken = response;
  document.querySelector("#created-enrollment-token").textContent = renderCreatedEnrollmentToken(
    response,
    data.get("controller-url")?.toString() || "",
    data.get("agent-name")?.toString() || "",
  );
  setStatus(`Created enrollment token ${response.id}. Copy the token before refreshing.`, "ok");
  await loadEnrollmentTokens();
}

async function loadEnrollmentTokens() {
  const enrollmentTokens = await api.listEnrollmentTokens();
  document.querySelector("#enrollment-tokens-list").innerHTML = renderEnrollmentTokens(enrollmentTokens);
  document.querySelectorAll("[data-revoke-token-id]").forEach((button) => {
    button.addEventListener("click", () => {
      revokeEnrollmentToken(button.dataset.revokeTokenId).catch((error) => setStatus(error.message, "error"));
    });
  });
}

async function revokeEnrollmentToken(id) {
  syncAdminTokenFromInput({ requireToken: true });
  await api.revokeEnrollmentToken(id);
  setStatus(`Revoked enrollment token ${id}.`, "ok");
  await loadEnrollmentTokens();
}

async function revokeSelectedAgentKey() {
  syncAdminTokenFromInput({ requireToken: true });
  const agent = selectedAgent();
  if (!agent) {
    throw new Error("Select an agent first.");
  }
  const label = agent.name || agent.id;
  if (
    typeof globalThis.confirm === "function" &&
    !globalThis.confirm(`Revoke agent ${label}? This disables its current key.`)
  ) {
    return;
  }
  const updated = await api.revokeAgentKey(agent.id);
  if (updated) {
    state.agents = state.agents.map((item) => (item.id === updated.id ? updated : item));
  }
  document.querySelector("#agents-list").innerHTML = renderAgents(state.agents, state.selectedAgentId);
  syncAgentActions();
  await refreshSelectedAgent();
  document.querySelector("#audit-list").innerHTML = renderAudit(await api.listAudit());
  setStatus(`Revoked agent ${label}.`, "ok");
}

async function submitCommand(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const request = buildCommandJobRequest({
    agentId: state.selectedAgentId,
    program: data.get("program"),
    args: data.get("args"),
    confirmed: data.get("confirm-risk") === "on",
  });
  const response = await api.createCommandJob(request);
  state.lastJobId = response.job_id;
  document.querySelector("#job-output").textContent = "Job queued. Waiting for output...";
  setStatus(`Created ${response.job_id} for ${response.target_count} target.`, "ok");
  await pollJobOutput(response.job_id);
}

async function pollJobOutput(jobId) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const chunks = await api.getJobOutput(jobId);
    document.querySelector("#job-output").textContent = renderJobOutput(chunks);
    if (Array.isArray(chunks) && chunks.length > 0) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

function boot() {
  const form = document.querySelector("#admin-auth");
  if (!form) {
    return;
  }
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    state.token = readAdminTokenInput();
    refreshAll().catch((error) => setStatus(error.message, "error"));
  });
  const runForm = document.querySelector("#run-command-form");
  if (runForm) {
    runForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitCommand(runForm).catch((error) => setStatus(error.message, "error"));
    });
  }
  const agentsList = document.querySelector("#agents-list");
  if (agentsList) {
    agentsList.addEventListener("click", handleAgentsListClick);
  }
  document.querySelector("#revoke-agent-key")?.addEventListener("click", () => {
    revokeSelectedAgentKey().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#refresh-telemetry")?.addEventListener("click", () => {
    try {
      syncAdminTokenFromInput({ requireToken: true });
      if (!state.selectedAgentId) {
        throw new Error("Select an agent before refreshing telemetry.");
      }
      setStatus(`Refreshing telemetry for ${state.selectedAgentId}...`);
      refreshSelectedAgent()
        .then(() => setStatus(`Refreshed telemetry for ${state.selectedAgentId}.`, "ok"))
        .catch((error) => setStatus(error.message, "error"));
    } catch (error) {
      setStatus(error.message, "error");
    }
  });
  const enrollmentForm = document.querySelector("#enrollment-token-form");
  if (enrollmentForm) {
    enrollmentForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitEnrollmentToken(enrollmentForm).catch((error) => setStatus(error.message, "error"));
    });
  }
}

if (typeof document !== "undefined") {
  boot();
}
