import { createApiClient, normalizeAdminToken } from "./api-client.js";

const state = {
  token: "",
  agents: [],
  policies: [],
  selectedAgentId: "",
  selectedPolicyId: "",
  lastJobId: "",
  createdEnrollmentToken: null,
  metricsWindowMs: 5 * 60 * 1000,
};

const DEFAULT_TELEMETRY_WINDOW_MS = 5 * 60 * 1000;
const TELEMETRY_PAGE_LIMIT = 120;
const DETAIL_PAGE_LIMIT = 25;
const JOB_OUTPUT_POLL_ATTEMPTS = 45;
const JOB_OUTPUT_POLL_INTERVAL_MS = 1000;
const TERMINAL_DISPATCH_STATES = new Set(["completed", "failed", "expired", "rejected", "canceled"]);
const TERMINAL_JOB_STATUSES = new Set(["success", "failed", "expired", "canceled"]);

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
      const session = agent.connected ? "session connected" : "session disconnected";
      const meta = [agent.hostname, platform, session, age].filter(Boolean).join(" · ");
      const selectedClass = agent.id === selectedAgentId ? " selected" : "";
      const status = agentDisplayStatus(agent);
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

export function agentDisplayStatus(agent) {
  if (agent?.revoked) {
    return "offline";
  }
  if (agent?.status === "reconnecting") {
    return "stale";
  }
  return agent?.status || "unknown";
}

export function renderAgentDetail(agent) {
  if (!agent) {
    return '<div class="empty">Select an agent.</div>';
  }
  const status = agentDisplayStatus(agent);
  const policies = Array.isArray(agent.assigned_policy_ids)
    ? agent.assigned_policy_ids.join(", ")
    : "";
  const rows = [
    ["Status", status],
    ["Session", agent.connected ? "connected" : "disconnected"],
    ["Revoked", agent.revoked ? "yes" : "no"],
    ["Hostname", agent.hostname],
    ["Platform", [agent.os, agent.arch].filter(Boolean).join("/")],
    ["Last seen", typeof agent.last_seen_age_seconds === "number" ? `${agent.last_seen_age_seconds}s ago` : ""],
    ["Assigned policies", policies],
  ];
  return `
    <div class="detail-grid">
      ${rows
        .map(
          ([label, value]) => `
            <div>
              <small>${escapeHtml(label)}</small>
              <strong>${escapeHtml(formatOptional(value))}</strong>
            </div>
          `,
        )
        .join("")}
    </div>
  `;
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
    ["Disk devices", formatOptional(body?.disk?.device_count)],
    ["Mounts", formatOptional(body?.disk?.mount_count)],
    ["Root disk total", formatKilobytes(body?.disk?.root_total_kb)],
    ["Root filesystem", body?.disk?.root_filesystem],
    ["Root FS type", body?.disk?.root_fs_type],
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

export function renderDiskInventory(snapshot, missingText = "No disk inventory.") {
  if (!snapshot || !snapshot.body) {
    return `<div class="empty">${escapeHtml(missingText)}</div>`;
  }
  const disk = snapshot.body.disk || {};
  const devices = Array.isArray(disk.devices) ? disk.devices : [];
  const mounts = Array.isArray(disk.mounts) ? disk.mounts : [];
  const deviceRows = devices.flatMap((device) => {
    const base = [
      {
        name: device.name,
        kind: device.kind,
        size: formatKilobytes(device.size_kb),
        mount: "device",
        fs: "",
      },
    ];
    const partitions = Array.isArray(device.partitions)
      ? device.partitions.map((partition) => ({
          name: partition.name,
          kind: "partition",
          size: formatKilobytes(partition.size_kb),
          mount: "",
          fs: "",
        }))
      : [];
    return base.concat(partitions);
  });
  const mountRows = mounts.map((mount) => ({
    name: mount.source,
    kind: "mount",
    size: "",
    mount: mount.mount_point,
    fs: mount.fs_type,
  }));
  const rows = deviceRows.concat(mountRows);
  if (rows.length === 0) {
    return `<div class="empty">${escapeHtml(missingText)}</div>`;
  }
  return `
    <table class="data-table">
      <thead>
        <tr>
          <th>Source</th>
          <th>Kind</th>
          <th>Size</th>
          <th>Mount</th>
          <th>FS</th>
        </tr>
      </thead>
      <tbody>
        ${rows
          .map(
            (row) => `
              <tr>
                <td>${escapeHtml(formatOptional(row.name))}</td>
                <td>${escapeHtml(formatOptional(row.kind))}</td>
                <td>${escapeHtml(row.size || "")}</td>
                <td>${escapeHtml(row.mount || "")}</td>
                <td>${escapeHtml(row.fs || "")}</td>
              </tr>
            `,
          )
          .join("")}
      </tbody>
    </table>
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
  const remediation = [
    report.acknowledged ? `acknowledged by ${report.acknowledged_by || "operator"}` : "open",
    report.resolved ? `resolved by ${report.resolution_job_id || "remediation job"}` : "not resolved",
  ].join(" · ");
  return `
    <div class="drift-summary">
      <span class="status-pill ${escapeHtml(report.status)}">${escapeHtml(report.status)}</span>
      <strong>${escapeHtml(report.policy_name)}</strong>
    </div>
    ${timeMeta ? `<div class="snapshot-time">${escapeHtml(timeMeta)}</div>` : ""}
    <div class="snapshot-time">${escapeHtml(remediation)}</div>
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

export function renderDriftHistory(page) {
      const reports = snapshotItems(page);
  if (reports.length === 0) {
    return '<div class="empty">No drift history.</div>';
  }
  return reports
    .map(
      (report) => `
        <div class="compact-row">
          <span class="status-pill ${escapeHtml(report.status || "unknown")}">${escapeHtml(report.status || "unknown")}</span>
          <span>
            <strong>${escapeHtml(report.policy_name || "policy")}</strong>
            <small>${escapeHtml(formatUnixMillis(report.agent_system_time_ms || report.checked_at_ms) || "unknown time")}</small>
            <small>${escapeHtml(report.resolved ? `resolved by ${report.resolution_job_id || "remediation job"}` : "open")}</small>
          </span>
        </div>
      `,
    )
    .join("");
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

export function recentSnapshots(snapshots, windowMs = DEFAULT_TELEMETRY_WINDOW_MS) {
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

export function renderTelemetryCharts(
  snapshots,
  definitions,
  emptyText = "No recent telemetry snapshots.",
  windowMs = DEFAULT_TELEMETRY_WINDOW_MS,
) {
  const recent = recentSnapshots(snapshots, windowMs);
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
      <strong>Last ${escapeHtml(formatDuration(windowMs))}</strong>
      <small>${escapeHtml(range || `${recent.length} sample(s)`)}</small>
    </div>
    <div class="chart-grid">${cards}</div>
  `;
}

function formatDuration(value) {
  const milliseconds = asNumber(value);
  if (milliseconds === null || milliseconds <= 0) {
    return "5 minutes";
  }
  const minutes = Math.round(milliseconds / 60_000);
  return minutes === 1 ? "1 minute" : `${minutes} minutes`;
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
      const dispatchState = job.dispatch_state || job.status || "unknown";
      const targets = Array.isArray(job.target_agents) ? job.target_agents : [];
      const connectedCount = targets.filter((target) => target?.connected).length;
      const targetSummary =
        targets.length > 0
          ? `${targets.length} target(s), ${connectedCount} connected`
          : `${job.target_count ?? 0} target(s)`;
      const assignmentSummarySource =
        job.assignment_summary && typeof job.assignment_summary === "object" ? job.assignment_summary : null;
      const assignmentEntries = assignmentSummarySource
        ? ["queued", "dispatched", "accepted", "started", "succeeded", "failed", "rejected", "canceled", "expired", "skipped"]
            .map((status) => [status, Number(assignmentSummarySource[status] || 0)])
            .filter(([, count]) => count > 0)
        : Array.from(
            targets.reduce((counts, target) => {
              const status = target?.assignment_status;
              if (status) {
                counts.set(status, (counts.get(status) || 0) + 1);
              }
              return counts;
            }, new Map()),
          );
      const assignmentSummary =
        assignmentEntries.length > 0
          ? `, ${assignmentEntries.map(([status, count]) => `${count} ${status}`).join(", ")}`
          : "";
      return `
        <button class="job-row" type="button" data-job-id="${escapeHtml(job.id)}">
          <span>
            <strong>${escapeHtml(job.id)}</strong>
            <small>${escapeHtml(command || "non-command job")}</small>
          </span>
          <span class="job-status">
            <span class="status-pill ${escapeHtml(job.status || "unknown")}">${escapeHtml(job.status || "unknown")}</span>
            <span class="status-pill ${escapeHtml(dispatchState)}">${escapeHtml(dispatchState)}</span>
          </span>
          <small>${escapeHtml(`${targetSummary}${assignmentSummary}`)}</small>
        </button>
      `;
    })
    .join("");
}

export function renderJobTargetTable(job) {
  const targets = Array.isArray(job?.target_agents) ? job.target_agents : [];
  if (targets.length === 0) {
    return '<div class="empty">No target assignment details.</div>';
  }
  return `
    <table class="data-table">
      <thead>
        <tr>
          <th>Agent</th>
          <th>Agent state</th>
          <th>Session</th>
          <th>Assignment</th>
          <th>Task</th>
          <th>Last error</th>
        </tr>
      </thead>
      <tbody>
        ${targets
          .map(
            (target) => `
              <tr>
                <td>${escapeHtml(target.agent_id || "agent")}</td>
                <td>${escapeHtml(target.revoked ? "revoked" : target.status || "unknown")}</td>
                <td>${escapeHtml(target.connected ? "connected" : "disconnected")}</td>
                <td>${escapeHtml(target.assignment_status || "not assigned")}</td>
                <td>${escapeHtml(target.task_id || "")}</td>
                <td>${escapeHtml(target.last_error || "")}</td>
              </tr>
            `,
          )
          .join("")}
      </tbody>
    </table>
  `;
}

export function renderApprovals(approvals, jobs = []) {
  if (!Array.isArray(approvals) || approvals.length === 0) {
    return '<div class="empty">No pending approvals.</div>';
  }
  return approvals
    .map((approval) => {
      const expires = formatUnixMillis(approval.expires_at_ms) || "unknown expiry";
      const job = Array.isArray(jobs) ? jobs.find((item) => item.id === approval.job_id) : null;
      const targets = Array.isArray(job?.target_agents) ? job.target_agents : [];
      const targetText =
        targets.length > 0
          ? targets.map((target) => `${target.agent_id || "agent"}:${target.assignment_status || "pending"}`).join(", ")
          : `${job?.target_count ?? 0} target(s)`;
      return `
        <div class="approval-row">
          <span class="status-pill ${escapeHtml(approval.status || "pending")}">${escapeHtml(approval.status || "pending")}</span>
          <span>
            <strong>${escapeHtml(approval.job_id || approval.id)}</strong>
            <small>${escapeHtml(approval.reason || "approval required")}</small>
            <small>${escapeHtml(targetText)}</small>
            <small>expires ${escapeHtml(expires)}</small>
          </span>
          <button type="button" data-approve-approval-id="${escapeHtml(approval.id)}">Approve</button>
          <button type="button" data-reject-approval-id="${escapeHtml(approval.id)}">Reject</button>
        </div>
      `;
    })
    .join("");
}

export function renderPolicies(policies, selectedPolicyId = "") {
  if (!Array.isArray(policies) || policies.length === 0) {
    return '<div class="empty">No policies saved.</div>';
  }
  return policies
    .map((policy) => {
      const selectedClass = policy.id === selectedPolicyId ? " selected" : "";
      return `
        <button class="policy-row${selectedClass}" type="button" data-policy-id="${escapeHtml(policy.id)}">
          <span>
            <strong>${escapeHtml(policy.name || policy.id)}</strong>
            <small>${escapeHtml(policy.id)}</small>
          </span>
          <span class="status-pill active">v${escapeHtml(policy.version || 1)}</span>
        </button>
      `;
    })
    .join("");
}

export function renderAgentPolicies(assignments, selectedAgentId = "") {
  if (!selectedAgentId) {
    return '<div class="empty">Select an agent to view assigned policies.</div>';
  }
  if (!Array.isArray(assignments) || assignments.length === 0) {
    return '<div class="empty">No policies assigned to this agent.</div>';
  }
  return assignments
    .map(
      (assignment) => `
        <div class="compact-row">
          <span class="status-pill active">assigned</span>
          <span>
            <strong>${escapeHtml(assignment.policy_id || "policy")}</strong>
            <small>${escapeHtml(formatUnixMillis(assignment.assigned_at_ms) || selectedAgentId)}</small>
          </span>
        </div>
      `,
    )
    .join("");
}

export function renderAgentLogs(page) {
  const chunks = snapshotItems(page);
  if (chunks.length === 0) {
    return '<div class="empty">No agent logs.</div>';
  }
  return chunks
    .map(
      (chunk) => `
        <div class="log-row">
          <small>${escapeHtml(formatUnixMillis(chunk.collected_at_ms) || "unknown time")}</small>
          <code>${escapeHtml(chunk.line || "")}</code>
        </div>
      `,
    )
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

export function buildRunbookJobRequest({ agentId, document, confirmed }) {
  if (!agentId) {
    throw new Error("Select an agent from the Agents list before creating a runbook job.");
  }
  const runbookDocument = String(document ?? "").trim();
  if (!runbookDocument) {
    throw new Error("Paste a runbook document before creating a job.");
  }
  if (!confirmed) {
    throw new Error("Check Confirm runbook execution before creating the job.");
  }
  const jobId = `job-runbook-ui-${Date.now()}`;
  return {
    job_id: jobId,
    target_agent_ids: [agentId],
    runbook_document: runbookDocument,
    timeout_seconds: 180,
    confirmed_high_risk: true,
    confirmed_by: "web-admin",
    expires_in_seconds: 300,
    nonce_prefix: jobId,
  };
}

export function buildPolicySaveRequest({ source }) {
  const policySource = String(source ?? "").trim();
  if (!policySource) {
    throw new Error("Paste a policy document before saving.");
  }
  return { source: policySource };
}

export function buildPolicyAssignmentRequest({ policyId, agentId }) {
  if (!policyId) {
    throw new Error("Select a policy first.");
  }
  if (!agentId) {
    throw new Error("Select an agent first.");
  }
  return { agent_id: agentId };
}

export function buildPolicyScheduleRequest({ policyId, agentId, intervalSeconds }) {
  if (!policyId) {
    throw new Error("Select a policy first.");
  }
  if (!agentId) {
    throw new Error("Select an agent first.");
  }
  const interval_seconds = Number.parseInt(intervalSeconds, 10);
  if (!Number.isInteger(interval_seconds) || interval_seconds < 1) {
    throw new Error("Drift interval must be at least 1 second.");
  }
  return { agent_id: agentId, interval_seconds };
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

export function renderJobOutput(chunks, { jobId = "", job = null } = {}) {
  if (!Array.isArray(chunks) || chunks.length === 0) {
    return renderJobOutputStatus(job || { id: jobId, status: "success", dispatch_state: "completed" }, {
      jobId,
    });
  }
  const lines = [];
  if (jobId) {
    lines.push(`Job: ${jobId}`);
  }
  if (job) {
    lines.push(`Status: ${job.status || "unknown"}`);
    lines.push(`Dispatch: ${job.dispatch_state || "unknown"}`);
  }
  lines.push(`Output chunks: ${chunks.length}`, "");
  for (const chunk of chunks) {
    const sequence = Number.isFinite(chunk?.sequence) ? ` #${chunk.sequence}` : "";
    const prefix = `[${chunk?.agent_id || "agent"} ${chunk?.stream || "stdout"}${sequence}]`;
    const data = String(chunk?.data ?? "");
    if (data.includes("\n")) {
      lines.push(prefix, data.replace(/\n$/, ""));
    } else {
      lines.push(`${prefix} ${data}`);
    }
  }
  return lines.join("\n");
}

export function renderJobOutputWaiting({ jobId = "", attempt = 0, maxAttempts = JOB_OUTPUT_POLL_ATTEMPTS } = {}) {
  return renderJobOutputStatus(
    { id: jobId, status: "queued", dispatch_state: "created", target_agents: [] },
    { jobId, attempt, maxAttempts },
  );
}

export function renderJobOutputStatus(
  job,
  { jobId = "", attempt = 0, maxAttempts = JOB_OUTPUT_POLL_ATTEMPTS, paused = false } = {},
) {
  const resolvedJobId = job?.id || jobId;
  const lines = [];
  if (resolvedJobId) {
    lines.push(`Job: ${resolvedJobId}`);
  }
  const progress = attempt > 0 ? ` (${attempt}/${maxAttempts})` : "";
  lines.push(`Status: ${job?.status || "unknown"}`);
  lines.push(`Dispatch: ${job?.dispatch_state || "unknown"}`);
  lines.push(jobStatusMessage(job));
  const targetSummary = jobTargetSummary(job);
  if (targetSummary) {
    lines.push(`Targets: ${targetSummary}`);
  }
  const expiresAt = formatUnixMillis(job?.expires_at_ms);
  if (expiresAt) {
    lines.push(`Expires at: ${expiresAt}`);
  }
  if (!isTerminalJob(job)) {
    lines.push(`Polling job output${progress}.`);
  }
  if (paused) {
    lines.push("Polling paused. Refresh or select the job again to continue.");
  }
  return lines.join("\n");
}

export function renderJobOutputEmpty({ jobId = "", maxAttempts = JOB_OUTPUT_POLL_ATTEMPTS } = {}) {
  return renderJobOutputStatus(
    { id: jobId, status: "success", dispatch_state: "completed", target_agents: [] },
    { jobId, maxAttempts },
  );
}

export function jobStatusMessage(job) {
  const status = String(job?.status || "").toLowerCase();
  const dispatchState = String(job?.dispatch_state || "").toLowerCase();
  const reason = job?.last_error ? ` Reason: ${job.last_error}` : "";
  if (status === "pending_approval") {
    return `Approval required before dispatch.${reason}`;
  }
  if (dispatchState === "created") {
    return "Job created. Checking dispatch state.";
  }
  if (dispatchState === "queued") {
    return `Queued until agent reconnects.${reason}`;
  }
  if (dispatchState === "delivered" || dispatchState === "running" || status === "running") {
    return `Running on agent. Waiting for output.${reason}`;
  }
  if (dispatchState === "completed" || status === "success") {
    return "Completed with no output.";
  }
  if (dispatchState === "expired" || status === "expired") {
    return `Expired before delivery or completion.${reason} Create a new job after the agent reconnects.`;
  }
  if (dispatchState === "canceled" || status === "canceled") {
    return `Canceled before completion.${reason} Check audit entries before retrying.`;
  }
  if (dispatchState === "rejected") {
    return `Rejected by controller policy.${reason} Review confirmation, target state, or audit entries before retrying.`;
  }
  if (dispatchState === "failed" || status === "failed") {
    return `Failed on agent.${reason} Review job output or audit entries before retrying.`;
  }
  return `Job state is ${dispatchState || status || "unknown"}. Refresh jobs or check controller logs.`;
}

export function isTerminalJob(job) {
  const status = String(job?.status || "").toLowerCase();
  const dispatchState = String(job?.dispatch_state || "").toLowerCase();
  return TERMINAL_JOB_STATUSES.has(status) || TERMINAL_DISPATCH_STATES.has(dispatchState);
}

function jobTargetSummary(job) {
  const targets = Array.isArray(job?.target_agents) ? job.target_agents : [];
  if (targets.length === 0) {
    return "";
  }
  return targets
    .map((target) => {
      const connected = target?.connected ? "connected" : "disconnected";
      const revoked = target?.revoked ? ", revoked" : "";
      return `${target?.agent_id || "agent"} ${target?.status || "unknown"} ${connected}${revoked}`;
    })
    .join("; ");
}

export function formatApiError(path, status) {
  if (status === 401) {
    return "Admin authentication expired or missing. Paste the admin token and refresh.";
  }
  if (status === 403) {
    return "Controller rejected this request. Check the admin token permissions.";
  }
  if (status === 404) {
    return `${path} was not found. Refresh the list and select an existing resource.`;
  }
  if (status === 409) {
    return `${path} conflicted with existing controller state. Refresh before retrying.`;
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
  const assignButton = document.querySelector("#assign-policy");
  const scheduleButton = document.querySelector("#schedule-policy");
  const agent = selectedAgent();
  if (revokeButton) {
    revokeButton.disabled = !agent || Boolean(agent.revoked);
  }
  if (refreshButton) {
    refreshButton.disabled = !agent;
  }
  if (assignButton) {
    assignButton.disabled = !agent || !state.selectedPolicyId;
  }
  if (scheduleButton) {
    scheduleButton.disabled = !agent || !state.selectedPolicyId;
  }
  const detail = document.querySelector("#agent-detail");
  if (detail) {
    detail.innerHTML = renderAgentDetail(agent);
  }
}

async function refreshSelectedAgent() {
  const agentId = state.selectedAgentId;
  if (!agentId) {
    return;
  }
  const [facts, metrics, drift, metricsHistory, driftHistory, logs, agentPolicies] = await Promise.all([
    readOptionalAgentData("facts", () => api.getLatestFacts(agentId)),
    readOptionalAgentData("metrics", () => api.getLatestMetrics(agentId)),
    readOptionalAgentData("drift", () => api.getLatestDrift(agentId)),
    readOptionalAgentData("metrics history", () => api.listMetrics(agentId, { limit: TELEMETRY_PAGE_LIMIT })),
    readOptionalAgentData("drift history", () => api.listDrift(agentId, { limit: DETAIL_PAGE_LIMIT })),
    readOptionalAgentData("agent logs", () => api.listAgentLogs(agentId, { limit: DETAIL_PAGE_LIMIT })),
    readOptionalAgentData("agent policies", () => api.listAgentPolicies(agentId)),
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
  document.querySelector("#disk-inventory").innerHTML = facts.error
    ? `<div class="empty">${escapeHtml(facts.error)}</div>`
    : renderDiskInventory(facts.value, "No disk inventory.");
  document.querySelector("#metrics-chart").innerHTML = metricsHistory.error
    ? `<div class="empty">${escapeHtml(metricsHistory.error)}</div>`
    : renderTelemetryCharts(
        snapshotItems(metricsHistory.value),
        METRICS_CHARTS,
        "No metrics snapshots in the selected window.",
        state.metricsWindowMs,
      );
  document.querySelector("#drift-panel").innerHTML = drift.error
    ? `<div class="empty">${escapeHtml(drift.error)}</div>`
    : renderDrift(drift.value);
  document.querySelector("#drift-history").innerHTML = driftHistory.error
    ? `<div class="empty">${escapeHtml(driftHistory.error)}</div>`
    : renderDriftHistory(driftHistory.value);
  document.querySelector("#agent-logs").innerHTML = logs.error
    ? `<div class="empty">${escapeHtml(logs.error)}</div>`
    : renderAgentLogs(logs.value);
  document.querySelector("#agent-policies").innerHTML = agentPolicies.error
    ? `<div class="empty">${escapeHtml(agentPolicies.error)}</div>`
    : renderAgentPolicies(agentPolicies.value, agentId);
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
  const [jobs, audit, enrollmentTokens, approvals, policies] = await Promise.all([
    api.listJobs(),
    api.listAudit(),
    api.listEnrollmentTokens(),
    api.listApprovals("pending"),
    api.listPolicies(),
  ]);
  document.querySelector("#jobs-list").innerHTML = renderJobs(jobs);
  document.querySelectorAll("[data-job-id]").forEach((button) => {
    button.addEventListener("click", () => {
      state.lastJobId = button.dataset.jobId;
      pollJobOutput(state.lastJobId).catch((error) => setStatus(error.message, "error"));
    });
  });
  document.querySelector("#approvals-list").innerHTML = renderApprovals(approvals, jobs);
  wireApprovalButtons();
  state.policies = Array.isArray(policies) ? policies : [];
  if (state.selectedPolicyId && !state.policies.some((policy) => policy.id === state.selectedPolicyId)) {
    state.selectedPolicyId = "";
  }
  if (!state.selectedPolicyId) {
    state.selectedPolicyId = state.policies[0]?.id || "";
  }
  renderPolicyList();
  document.querySelector("#enrollment-tokens-list").innerHTML = renderEnrollmentTokens(enrollmentTokens);
  document.querySelectorAll("[data-revoke-token-id]").forEach((button) => {
    button.addEventListener("click", () => {
      revokeEnrollmentToken(button.dataset.revokeTokenId).catch((error) => setStatus(error.message, "error"));
    });
  });
  document.querySelector("#audit-list").innerHTML = renderAudit(audit);
  setStatus("Loaded latest controller data.", "ok");
}

function renderPolicyList() {
  const list = document.querySelector("#policies-list");
  if (list) {
    list.innerHTML = renderPolicies(state.policies, state.selectedPolicyId);
  }
  syncAgentActions();
}

function wireApprovalButtons() {
  document.querySelectorAll("[data-approve-approval-id]").forEach((button) => {
    button.addEventListener("click", () => {
      decideApproval(button.dataset.approveApprovalId, "approve").catch((error) =>
        setStatus(error.message, "error"),
      );
    });
  });
  document.querySelectorAll("[data-reject-approval-id]").forEach((button) => {
    button.addEventListener("click", () => {
      decideApproval(button.dataset.rejectApprovalId, "reject").catch((error) =>
        setStatus(error.message, "error"),
      );
    });
  });
}

async function loadApprovalsJobsAndAudit() {
  const [approvals, jobs, audit] = await Promise.all([
    api.listApprovals("pending"),
    api.listJobs(),
    api.listAudit(),
  ]);
  document.querySelector("#approvals-list").innerHTML = renderApprovals(approvals, jobs);
  wireApprovalButtons();
  document.querySelector("#jobs-list").innerHTML = renderJobs(jobs);
  document.querySelector("#audit-list").innerHTML = renderAudit(audit);
}

async function decideApproval(id, action) {
  syncAdminTokenFromInput({ requireToken: true });
  if (!id) {
    throw new Error("Select an approval request first.");
  }
  const reason =
    typeof globalThis.prompt === "function"
      ? globalThis.prompt(`${action === "approve" ? "Approve" : "Reject"} reason`, "")
      : "";
  if (reason === null) {
    return;
  }
  if (action === "approve") {
    await api.approveApproval(id, { reason: reason || "" });
  } else {
    await api.rejectApproval(id, { reason: reason || "" });
  }
  setStatus(`${action === "approve" ? "Approved" : "Rejected"} ${id}.`, "ok");
  await loadApprovalsJobsAndAudit();
}

async function expireDueApprovals() {
  syncAdminTokenFromInput({ requireToken: true });
  const response = await api.expireApprovals();
  setStatus(`Expired ${response?.expired_count ?? 0} approval request(s).`, "ok");
  await loadApprovalsJobsAndAudit();
}

async function loadPolicies() {
  const policies = await api.listPolicies();
  state.policies = Array.isArray(policies) ? policies : [];
  if (state.selectedPolicyId && !state.policies.some((policy) => policy.id === state.selectedPolicyId)) {
    state.selectedPolicyId = "";
  }
  if (!state.selectedPolicyId) {
    state.selectedPolicyId = state.policies[0]?.id || "";
  }
  renderPolicyList();
}

async function submitPolicy(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const request = buildPolicySaveRequest({ source: data.get("policy-source") });
  const saved = await api.savePolicy(request);
  state.selectedPolicyId = saved.id;
  setStatus(`Saved policy ${saved.id}.`, "ok");
  await loadPolicies();
}

async function assignSelectedPolicy() {
  syncAdminTokenFromInput({ requireToken: true });
  const request = buildPolicyAssignmentRequest({
    policyId: state.selectedPolicyId,
    agentId: state.selectedAgentId,
  });
  await api.assignPolicy(state.selectedPolicyId, request);
  setStatus(`Assigned policy ${state.selectedPolicyId} to ${state.selectedAgentId}.`, "ok");
  await refreshSelectedAgent();
}

async function scheduleSelectedPolicy() {
  syncAdminTokenFromInput({ requireToken: true });
  const request = buildPolicyScheduleRequest({
    policyId: state.selectedPolicyId,
    agentId: state.selectedAgentId,
    intervalSeconds: document.querySelector("#policy-schedule-interval")?.value,
  });
  await api.schedulePolicyDrift(state.selectedPolicyId, request);
  setStatus(`Scheduled drift for ${state.selectedPolicyId} on ${state.selectedAgentId}.`, "ok");
  await refreshSelectedAgent();
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
  const pendingApproval = response.status === "pending_approval" || response.approval_request_id;
  document.querySelector("#job-output").textContent = renderJobOutputStatus(
    {
      id: response.job_id,
      status: pendingApproval ? "pending_approval" : "queued",
      dispatch_state: pendingApproval ? "created" : "created",
      target_count: response.target_count,
      target_agents: [],
    },
    { jobId: response.job_id },
  );
  document.querySelector("#job-targets").innerHTML = renderJobTargetTable(null);
  if (pendingApproval) {
    setStatus(`Created ${response.job_id}; approval ${response.approval_request_id} is required.`, "ok");
    await loadApprovalsJobsAndAudit();
    return;
  }
  setStatus(`Created ${response.job_id} for ${response.target_count} target.`, "ok");
  await pollJobOutput(response.job_id);
}

async function submitRunbook(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const request = buildRunbookJobRequest({
    agentId: state.selectedAgentId,
    document: data.get("runbook-document"),
    confirmed: data.get("confirm-risk") === "on",
  });
  const response = await api.createRunbookJob(request);
  state.lastJobId = response.job_id;
  const pendingApproval = response.status === "pending_approval" || response.approval_request_id;
  const statusText = pendingApproval
    ? `Runbook job ${response.job_id} requires approval ${response.approval_request_id}.`
    : `Runbook job ${response.job_id} created.`;
  document.querySelector("#runbook-result").textContent = statusText;
  document.querySelector("#job-output").textContent = renderJobOutputStatus(
    {
      id: response.job_id,
      status: pendingApproval ? "pending_approval" : "queued",
      dispatch_state: "created",
      target_count: response.target_count,
      target_agents: [],
    },
    { jobId: response.job_id },
  );
  setStatus(statusText, "ok");
  await loadApprovalsJobsAndAudit();
  if (!pendingApproval) {
    await pollJobOutput(response.job_id);
  }
}

async function pollJobOutput(jobId) {
  const outputElement = document.querySelector("#job-output");
  const targetElement = document.querySelector("#job-targets");
  for (let attempt = 1; attempt <= JOB_OUTPUT_POLL_ATTEMPTS; attempt += 1) {
    const [job, chunks] = await Promise.all([api.getJob(jobId), api.getJobOutput(jobId)]);
    if (targetElement) {
      targetElement.innerHTML = renderJobTargetTable(job);
    }
    if (Array.isArray(chunks) && chunks.length > 0) {
      outputElement.textContent = renderJobOutput(chunks, { jobId, job });
      if (isTerminalJob(job)) {
        return;
      }
    } else {
      outputElement.textContent = renderJobOutputStatus(job || { id: jobId }, {
        jobId,
        attempt,
        maxAttempts: JOB_OUTPUT_POLL_ATTEMPTS,
      });
      if (isTerminalJob(job)) {
        return;
      }
    }
    if (attempt < JOB_OUTPUT_POLL_ATTEMPTS) {
      await new Promise((resolve) => setTimeout(resolve, JOB_OUTPUT_POLL_INTERVAL_MS));
    }
  }
  const [job, chunks] = await Promise.all([api.getJob(jobId), api.getJobOutput(jobId)]);
  if (targetElement) {
    targetElement.innerHTML = renderJobTargetTable(job);
  }
  outputElement.textContent =
    Array.isArray(chunks) && chunks.length > 0
      ? renderJobOutput(chunks, { jobId, job })
      : renderJobOutputStatus(job || { id: jobId }, {
          jobId,
          attempt: JOB_OUTPUT_POLL_ATTEMPTS,
          maxAttempts: JOB_OUTPUT_POLL_ATTEMPTS,
          paused: !isTerminalJob(job),
        });
}

export async function pollJobOutputOnce(apiClient, jobId) {
  const [job, chunks] = await Promise.all([apiClient.getJob(jobId), apiClient.getJobOutput(jobId)]);
  if (Array.isArray(chunks) && chunks.length > 0) {
    return {
      text: renderJobOutput(chunks, { jobId, job }),
      terminal: isTerminalJob(job),
    };
  }
  return {
    text: renderJobOutputStatus(job || { id: jobId }, {
      jobId,
      attempt: 1,
      maxAttempts: 1,
    }),
    terminal: isTerminalJob(job),
  };
}

export function renderJobOutputAfterPolling({ job = null, chunks = [], jobId = "" } = {}) {
  if (Array.isArray(chunks) && chunks.length > 0) {
    return renderJobOutput(chunks, { jobId, job });
  }
  return renderJobOutputStatus(job || { id: jobId }, {
    jobId,
    attempt: JOB_OUTPUT_POLL_ATTEMPTS,
    maxAttempts: JOB_OUTPUT_POLL_ATTEMPTS,
  });
}

function boot() {
  const form = document.querySelector("#admin-auth");
  if (!form) {
    return;
  }
  const warning = document.querySelector("#transport-warning");
  if (warning && globalThis.location?.protocol === "http:") {
    warning.hidden = false;
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
  const runbookForm = document.querySelector("#runbook-form");
  if (runbookForm) {
    runbookForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitRunbook(runbookForm).catch((error) => setStatus(error.message, "error"));
    });
  }
  const policyForm = document.querySelector("#policy-form");
  if (policyForm) {
    policyForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitPolicy(policyForm).catch((error) => setStatus(error.message, "error"));
    });
  }
  const agentsList = document.querySelector("#agents-list");
  if (agentsList) {
    agentsList.addEventListener("click", handleAgentsListClick);
  }
  const policiesList = document.querySelector("#policies-list");
  if (policiesList) {
    policiesList.addEventListener("click", (event) => {
      const button = event.target?.closest?.("[data-policy-id]");
      if (!button?.dataset?.policyId) {
        return;
      }
      state.selectedPolicyId = button.dataset.policyId;
      renderPolicyList();
    });
  }
  document.querySelector("#revoke-agent-key")?.addEventListener("click", () => {
    revokeSelectedAgentKey().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#expire-approvals")?.addEventListener("click", () => {
    expireDueApprovals().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#assign-policy")?.addEventListener("click", () => {
    assignSelectedPolicy().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#schedule-policy")?.addEventListener("click", () => {
    scheduleSelectedPolicy().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#metrics-range")?.addEventListener("change", (event) => {
    const value = Number.parseInt(event.target?.value || "", 10);
    state.metricsWindowMs = Number.isInteger(value) && value > 0 ? value : DEFAULT_TELEMETRY_WINDOW_MS;
    if (state.selectedAgentId) {
      refreshSelectedAgent().catch((error) => setStatus(error.message, "error"));
    }
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
