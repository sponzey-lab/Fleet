import { createApiClient, normalizeAdminToken } from "./api-client.js";

const state = {
  token: "",
  agents: [],
  policies: [],
  remediations: [],
  selectedAgentId: "",
  selectedPolicyId: "",
  selectedRemediationId: "",
  lastJobId: "",
  createdEnrollmentToken: null,
  metricsWindowMs: 5 * 60 * 1000,
  auditCursor: "",
  auditItems: [],
  catalogSources: [],
  catalogRevisions: [],
  catalogDocuments: [],
  selectedCatalogSourceId: "",
  selectedCatalogCommit: "",
  catalogActionInFlight: false,
};

const ADMIN_ROUTES = new Set([
  "overview",
  "agents",
  "run",
  "runbooks",
  "policies",
  "remediations",
  "audit",
  "settings",
]);

const ADMIN_ROUTE_ALIASES = { approvals: "run", jobs: "run" };

const ADMIN_ROUTE_PRESENTATION = {
  overview: ["Overview", "Live operational state from the Controller — no inferred health."],
  agents: ["Agents", "Inventory, facts, metrics, and the most recent agent activity."],
  run: ["Run", "Create signed commands, review pending approvals, and inspect assignment state."],
  runbooks: ["Runbooks", "Create signed runbook jobs and review pending approvals."],
  policies: ["Policies & Drift", "Manage policy sources, assignments, and observed drift."],
  remediations: ["Remediations", "Approve policy-driven fixes through their persisted lifecycle."],
  audit: ["Audit", "Review durable product and security events."],
  settings: ["Settings", "Enrollment and Controller signing trust administration."],
};

const DEFAULT_TELEMETRY_WINDOW_MS = 5 * 60 * 1000;
const TELEMETRY_PAGE_LIMIT = 120;
const DETAIL_PAGE_LIMIT = 25;
const JOB_OUTPUT_POLL_ATTEMPTS = 45;
const JOB_OUTPUT_POLL_INTERVAL_MS = 1000;
const COMMAND_JOB_EXPIRES_IN_SECONDS = 300;
const TERMINAL_DISPATCH_STATES = new Set(["completed", "failed", "expired", "rejected", "canceled"]);
const TERMINAL_JOB_STATUSES = new Set(["success", "failed", "expired", "canceled"]);

const api = createApiClient({
  tokenProvider: () => state.token,
  formatError: formatApiError,
});

function currentAdminRoute() {
  const route = globalThis.location?.hash?.slice(1).toLowerCase() || "overview";
  const canonicalRoute = ADMIN_ROUTE_ALIASES[route] || route;
  return ADMIN_ROUTES.has(canonicalRoute) ? canonicalRoute : "overview";
}

/// Applies a hash route without persisting token or view state in browser storage.
export function applyAdminRoute({ focus = false } = {}) {
  if (typeof document === "undefined") {
    return "overview";
  }
  const route = currentAdminRoute();
  document.querySelectorAll("[data-route]").forEach((element) => {
    element.hidden = !element.dataset.route.split(" ").includes(route);
  });
  document.querySelectorAll("[data-route-link]").forEach((link) => {
    const active = link.dataset.routeLink === route;
    link.classList.toggle("active", active);
    if (active) {
      link.setAttribute("aria-current", "page");
    } else {
      link.removeAttribute("aria-current");
    }
  });
  const [title, subtitle] = ADMIN_ROUTE_PRESENTATION[route];
  const pageTitle = document.querySelector("#page-title");
  const pageSubtitle = document.querySelector("#page-subtitle");
  if (pageTitle) pageTitle.textContent = title;
  if (pageSubtitle) pageSubtitle.textContent = subtitle;
  if (focus) {
    document.querySelector(`[data-route-focus="${route}"]`)?.focus({ preventScroll: true });
  }
  return route;
}

function renderOverviewSummary({ agents, jobs, approvals, remediations }) {
  const summary = [
    ["Known agents", Array.isArray(agents) ? agents.length : 0],
    ["Recent jobs", Array.isArray(jobs) ? jobs.length : 0],
    ["Pending approvals", Array.isArray(approvals) ? approvals.length : 0],
    ["Remediation requests", Array.isArray(remediations) ? remediations.length : 0],
  ];
  return `<div class="overview-grid">${summary
    .map(
      ([label, value]) => `<div class="overview-card"><small>${escapeHtml(label)}</small><strong>${escapeHtml(value)}</strong></div>`,
    )
    .join("")}</div>`;
}

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
  if (agent?.connected) {
    return "online";
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
  const capabilities = Array.isArray(agent.capabilities)
    ? agent.capabilities.join(", ")
    : "";
  const rows = [
    ["Status", status],
    ["Session", agent.connected ? "connected" : "disconnected"],
    ["Revoked", agent.revoked ? "yes" : "no"],
    ["Hostname", agent.hostname],
    ["Platform", [agent.os, agent.arch].filter(Boolean).join("/")],
    ["Last seen", typeof agent.last_seen_age_seconds === "number" ? `${agent.last_seen_age_seconds}s ago` : ""],
    ["Assigned policies", policies],
    ["Capabilities", capabilities],
    ["Capability reported", formatUnixMillis(agent.capability_reported_at_ms)],
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

export function renderJobArtifacts(job) {
  const artifacts = Array.isArray(job?.rendered_artifacts) ? job.rendered_artifacts : [];
  if (artifacts.length === 0) {
    return '<div class="empty">No rendered artifacts.</div>';
  }
  return `
    <table class="data-table">
      <thead>
        <tr>
          <th>Artifact</th>
          <th>Class</th>
          <th>Action</th>
        </tr>
      </thead>
      <tbody>
        ${artifacts
          .map((artifact) => {
            const checksum = String(artifact?.checksum_sha256 || "");
            return `
              <tr class="artifact-row">
                <td>
                  <strong>${escapeHtml(artifact?.artifact_id || "artifact")}</strong>
                  <small>${escapeHtml(`agent=${artifact?.agent_id || ""} task=${artifact?.task_id || ""}`)}</small>
                  <small>${escapeHtml(`sha256=${checksum.slice(0, 12)} size=${artifact?.size_bytes ?? 0}`)}</small>
                </td>
                <td>${escapeHtml(artifact?.retention_class || "artifact")}</td>
                <td>
                  <button type="button" data-artifact-id="${escapeHtml(artifact?.artifact_id || "")}" data-artifact-job-id="${escapeHtml(job?.id || "")}">Open</button>
                </td>
              </tr>
            `;
          })
          .join("")}
      </tbody>
    </table>
  `;
}

export function renderArtifactBody(artifact) {
  if (!artifact) {
    return "Artifact body is missing or no longer available.";
  }
  const bytes = Array.isArray(artifact.content_bytes)
    ? artifact.content_bytes.map((value) => Number(value)).filter((value) => Number.isInteger(value) && value >= 0 && value <= 255)
    : [];
  const checksum = String(artifact.checksum_sha256 || "");
  const text = printableArtifactPreview(bytes);
  const lines = [
    `Artifact: ${artifact.artifact_id || ""}`,
    `Job: ${artifact.job_id || ""}`,
    `Agent: ${artifact.agent_id || ""}`,
    `Task: ${artifact.task_id || ""}`,
    `Class: ${artifact.retention_class || ""}`,
    `Size: ${artifact.size_bytes ?? bytes.length} bytes`,
    `SHA-256: ${checksum.slice(0, 12)}`,
    "",
    text ? `Preview:\n${text}` : "Preview unavailable for non-printable content.",
  ];
  return lines.join("\n");
}

function printableArtifactPreview(bytes) {
  if (bytes.length === 0) {
    return "";
  }
  const visible = bytes.every((value) => value === 9 || value === 10 || value === 13 || (value >= 32 && value <= 126));
  if (!visible) {
    return "";
  }
  return String.fromCharCode(...bytes).slice(0, 4096);
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

export function renderRemediations(remediations, selectedRemediationId = "") {
  if (!Array.isArray(remediations) || remediations.length === 0) {
    return '<div class="empty">No remediation requests.</div>';
  }
  return remediations
    .map((remediation) => {
      const selectedClass = remediation.id === selectedRemediationId ? " selected" : "";
      const updated = formatUnixMillis(remediation.updated_at_ms) || "unknown time";
      const job = remediation.job_id ? `job ${remediation.job_id}` : "no job";
      const verification = remediation.verification_assignment_status
        ? `verification ${remediation.verification_assignment_status}`
        : "verification not scheduled";
      const evidence = remediation.verification_evidence_status
        ? `evidence ${remediation.verification_evidence_status}`
        : "no verification evidence";
      const legacy = remediation.legacy_state || remediation.lifecycle_source !== "persisted"
        ? remediation.legacy_state || "legacy read model"
        : "persisted lifecycle";
      return `
        <button class="compact-row${selectedClass}" type="button" data-remediation-id="${escapeHtml(remediation.id)}">
          <span class="status-pill ${escapeHtml(remediation.status || "unknown")}">${escapeHtml(remediation.status || "unknown")}</span>
          <span>
            <strong>${escapeHtml(remediation.policy_id || remediation.policy_name || "policy")}</strong>
            <small>${escapeHtml(remediation.agent_id || "agent")} · ${escapeHtml(job)} · ${escapeHtml(updated)}</small>
            <small>${escapeHtml(remediation.runbook_ref || "no runbook ref")}</small>
            <small>${escapeHtml(remediation.risk_summary || "approval required")}</small>
            <small>${escapeHtml(verification)} · ${escapeHtml(evidence)} · ${escapeHtml(legacy)}</small>
          </span>
        </button>
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

export function buildRunbookJobRequest({ agentId, selector = "", document, confirmed }) {
  const targetSelector = String(selector ?? "").trim();
  if (!agentId && !targetSelector) {
    throw new Error("Select an agent or enter a target selector before creating a runbook job.");
  }
  const runbookDocument = String(document ?? "").trim();
  if (!runbookDocument) {
    throw new Error("Paste a runbook document before creating a job.");
  }
  if (!confirmed) {
    throw new Error("Check Confirm runbook execution before creating the job.");
  }
  const jobId = `job-runbook-ui-${Date.now()}`;
  const request = {
    job_id: jobId,
    target_agent_ids: targetSelector ? [] : [agentId],
    runbook_document: runbookDocument,
    timeout_seconds: 180,
    confirmed_high_risk: true,
    confirmed_by: "web-admin",
    expires_in_seconds: 300,
    nonce_prefix: jobId,
  };
  if (targetSelector) {
    request.selector = targetSelector;
  }
  return request;
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
    `fleet agent init --url ${url} --token ${result.token} --name ${name}`,
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

function parseIntegerControl(value, label, minimum) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed) || parsed < minimum) {
    throw new Error(`${label} must be at least ${minimum}.`);
  }
  return parsed;
}

function parseAgentIds(value) {
  return String(value ?? "")
    .split(/[,\s]+/)
    .map((part) => part.trim())
    .filter(Boolean);
}

export function buildStagedTrustBundleRequest({
  previousPublicKeyPath = "",
  agentIds = "",
  batchSize,
  maxFailures,
  ackTimeoutSeconds,
}) {
  const request = {
    batch_size: parseIntegerControl(batchSize, "Batch size", 1),
    max_failures: parseIntegerControl(maxFailures, "Max failures", 0),
    ack_timeout_seconds: parseIntegerControl(ackTimeoutSeconds, "Ack timeout", 1),
  };
  const previous_public_key_path = String(previousPublicKeyPath ?? "").trim();
  if (previous_public_key_path) {
    request.previous_public_key_path = previous_public_key_path;
  }
  const parsedAgentIds = parseAgentIds(agentIds);
  if (parsedAgentIds.length > 0) {
    request.agent_ids = parsedAgentIds;
  }
  return request;
}

export function renderControllerSigningRotationStatus(status) {
  if (!status || typeof status !== "object") {
    return '<div class="empty">No signing status loaded.</div>';
  }
  const rows = [
    ["Controller", status.controller_id],
    ["Persisted", status.persisted_record_present ? "yes" : "no"],
    ["State", status.persisted_state],
    ["Readiness", status.readiness],
    ["Bootstrap guard", status.bootstrap_guard],
    ["Agent trust", status.agent_trust_rollout],
    ["Active signing", status.active_signing_fingerprint_prefix],
    ["Selected signing", status.selected_signing_fingerprint_prefix],
    ["Old fingerprint", status.old_fingerprint_prefix],
    ["New fingerprint", status.new_fingerprint_prefix],
    ["Requested", formatUnixMillis(status.requested_at_ms)],
    ["Validated", formatUnixMillis(status.validated_at_ms)],
    ["Activated", formatUnixMillis(status.activated_at_ms)],
    ["Old verifies until", formatUnixMillis(status.old_key_verifies_until_ms)],
    ["Retired", formatUnixMillis(status.retired_at_ms)],
    ["Failed", formatUnixMillis(status.failed_at_ms)],
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

export function renderStagedTrustBundleResult(result) {
  if (!result || typeof result !== "object") {
    return '<div class="empty">No staged rollout result.</div>';
  }
  const counts = [
    ["Targets", result.target_count ?? 0],
    ["Planned", result.planned_count ?? 0],
    ["Attempted", result.attempted_count ?? 0],
    ["Updated", result.updated_count ?? 0],
    ["Skipped", result.skipped_count ?? 0],
    ["Failed", result.failed_count ?? 0],
    ["Current", result.already_current_count ?? 0],
    ["Unavailable", result.unavailable_count ?? 0],
    ["Pending", result.pending_count ?? 0],
    ["Entries", result.entries_count ?? 0],
  ];
  const agentRows = Array.isArray(result.agent_results)
    ? result.agent_results
        .map(
          (agent) => `
            <tr>
              <td>${escapeHtml(agent.agent_id || "")}</td>
              <td>${escapeHtml(agent.status || "unknown")}</td>
            </tr>
          `,
        )
        .join("")
    : "";
  return `
    <div class="detail-grid signing-grid">
      <div><small>Controller</small><strong>${escapeHtml(formatOptional(result.controller_id))}</strong></div>
      <div><small>Persisted state</small><strong>${escapeHtml(formatOptional(result.persisted_state))}</strong></div>
      <div><small>Rollout state</small><strong>${escapeHtml(formatOptional(result.rollout_state))}</strong></div>
      <div><small>Current fingerprint</small><strong>${escapeHtml(formatOptional(result.current_fingerprint_prefix))}</strong></div>
      <div><small>Previous fingerprint</small><strong>${escapeHtml(formatOptional(result.previous_fingerprint_prefix))}</strong></div>
    </div>
    <div class="preview-counts">
      ${counts.map(([label, value]) => `<span><strong>${escapeHtml(value)}</strong><small>${escapeHtml(label)}</small></span>`).join("")}
    </div>
    ${
      agentRows
        ? `<table class="data-table"><thead><tr><th>Agent</th><th>Status</th></tr></thead><tbody>${agentRows}</tbody></table>`
        : '<div class="empty">No agent results returned.</div>'
    }
  `;
}

export function parseCommandArgs(value) {
  return String(value ?? "")
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean);
}

export function buildCommandJobRequest({ agentId, selector = "", program, args, confirmed }) {
  const targetSelector = String(selector ?? "").trim();
  if (!agentId && !targetSelector) {
    throw new Error("Select an agent or enter a target selector before running a command.");
  }
  if (!program || !String(program).trim()) {
    throw new Error("Enter a program to run, for example uptime.");
  }
  if (!confirmed) {
    throw new Error("Check Confirm high-risk execution before running the command.");
  }
  const jobId = `job-ui-${Date.now()}`;
  const request = {
    job_id: jobId,
    target_agent_ids: targetSelector ? [] : [agentId],
    program: String(program).trim(),
    args: Array.isArray(args) ? args : parseCommandArgs(args),
    timeout_seconds: 30,
    confirmed_high_risk: true,
    confirmed_by: "web-admin",
    expires_in_seconds: COMMAND_JOB_EXPIRES_IN_SECONDS,
    nonce_prefix: jobId,
  };
  if (targetSelector) {
    request.selector = targetSelector;
  }
  return request;
}

export function buildSelectorPreviewRequest({ selector }) {
  const targetSelector = String(selector ?? "").trim();
  if (!targetSelector) {
    throw new Error("Enter a target selector before previewing targets.");
  }
  return { selector: targetSelector };
}

export function renderSelectorPreview(preview) {
  if (!preview || typeof preview !== "object") {
    return '<div class="empty">No selector preview.</div>';
  }
  const counts = [
    ["Matched", preview.matched_count ?? 0],
    ["Selected", preview.selected_count ?? 0],
    ["Disabled", preview.disabled_count ?? 0],
    ["Offline", preview.offline_count ?? 0],
  ];
  const warnings = Array.isArray(preview.warnings) ? preview.warnings : [];
  const agents = Array.isArray(preview.agents) ? preview.agents : [];
  const rows = agents
    .map((agent) => {
      const selected = Boolean(agent.selected_for_dispatch);
      const labels = Array.isArray(agent.labels)
        ? agent.labels.map((label) => `${label.key}=${label.value}`).join(", ")
        : "";
      return `
        <tr>
          <td>${escapeHtml(agent.name || agent.agent_id || "")}</td>
          <td>${escapeHtml(agent.agent_id || "")}</td>
          <td>${escapeHtml(labels || "none")}</td>
          <td>${escapeHtml(agent.status || "unknown")}</td>
          <td><span class="status-pill ${selected ? "active" : "revoked"}">${selected ? "selected" : "excluded"}</span></td>
        </tr>
      `;
    })
    .join("");
  return `
    <div class="preview-counts">
      ${counts.map(([label, value]) => `<span><strong>${escapeHtml(value)}</strong><small>${escapeHtml(label)}</small></span>`).join("")}
    </div>
    ${warnings.map((warning) => `<div class="preview-warning">${escapeHtml(warning.message || warning.code || "preview warning")}</div>`).join("")}
    ${
      rows
        ? `<table class="data-table"><thead><tr><th>Name</th><th>Agent</th><th>Labels</th><th>Status</th><th>Dispatch</th></tr></thead><tbody>${rows}</tbody></table>`
        : '<div class="empty">No matching agents.</div>'
    }
  `;
}

export function selectorPreviewSelectedCount(preview) {
  const selectedCount = Number(preview?.selected_count ?? 0);
  return Number.isFinite(selectedCount) && selectedCount > 0 ? selectedCount : 0;
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
    if (isApprovalPendingJob(job)) {
      lines.push("Open Approvals and approve or reject this job before output can appear.");
    } else {
      lines.push(`Polling job output${progress}.`);
    }
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

export function isApprovalPendingJob(job) {
  const status = String(job?.status || "").toLowerCase();
  const dispatchState = String(job?.dispatch_state || "").toLowerCase();
  return status === "pending_approval" || dispatchState === "pending_approval";
}

export function approvalDecisionJobToPoll(action, decision) {
  if (action !== "approve") {
    return "";
  }
  return String(decision?.job_id || "").trim();
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

async function loadAuditPage({ append = false } = {}) {
  syncAdminTokenFromInput({ requireToken: true });
  const category = document.querySelector("#audit-category")?.value || "";
  const page = await api.exportAudit({ category, limit: 50, before: append ? state.auditCursor : "" });
  const items = Array.isArray(page?.items) ? page.items : [];
  state.auditItems = append ? [...state.auditItems, ...items] : items;
  state.auditCursor = String(page?.next_cursor || "");
  document.querySelector("#audit-list").innerHTML = renderAudit(state.auditItems);
  const more = document.querySelector("#load-more-audit");
  if (more) more.disabled = !state.auditCursor;
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

async function loadSigningRotationStatus() {
  const status = await api.getControllerSigningRotationStatus();
  const element = document.querySelector("#signing-rotation-status");
  if (element) {
    element.innerHTML = renderControllerSigningRotationStatus(status);
  }
  return status;
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
  await loadCatalogSources();
  const [jobs, enrollmentTokens, approvals, policies, remediations, signingStatus] = await Promise.all([
    api.listJobs(),
    api.listEnrollmentTokens(),
    api.listApprovals("pending"),
    api.listPolicies(),
    api.listRemediations({ limit: DETAIL_PAGE_LIMIT }),
    api.getControllerSigningRotationStatus(),
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
  state.remediations = Array.isArray(remediations) ? remediations : [];
  renderRemediationList();
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
  await loadAuditPage();
  document.querySelector("#signing-rotation-status").innerHTML =
    renderControllerSigningRotationStatus(signingStatus);
  const overview = document.querySelector("#overview-summary");
  if (overview) {
    overview.classList.remove("empty");
    overview.innerHTML = renderOverviewSummary({
      agents: state.agents,
      jobs,
      approvals,
      remediations: state.remediations,
    });
  }
  setStatus("Loaded latest controller data.", "ok");
}

export function renderCatalogSources(sources, selectedSourceId = "") {
  if (!Array.isArray(sources) || sources.length === 0) {
    return '<div class="empty">No catalog sources registered.</div>';
  }
  return sources
    .map((source) => {
      const selected = source.id === selectedSourceId ? " selected" : "";
      const active = source.active_commit
        ? `<span class="status-pill active">active ${escapeHtml(source.active_commit.slice(0, 12))}</span>`
        : '<span class="status-pill">no active revision</span>';
      return `<button class="catalog-row${selected}" type="button" data-catalog-source-id="${escapeHtml(source.id)}"><span><strong>${escapeHtml(source.id)}</strong><small>${escapeHtml(source.reference)}</small></span>${active}</button>`;
    })
    .join("");
}

export function renderCatalogRevisions(revisions, selectedCommit = "", activeCommit = "") {
  if (!Array.isArray(revisions) || revisions.length === 0) {
    return '<div class="empty">No synced revisions for this source.</div>';
  }
  return revisions
    .map((revision) => {
      const selected = revision.commit === selectedCommit ? " selected" : "";
      const active = revision.commit === activeCommit ? '<span class="status-pill active">active</span>' : "";
      const failure = revision.failure ? `<small>${escapeHtml(revision.failure)}</small>` : "";
      return `<button class="catalog-row${selected}" type="button" data-catalog-commit="${escapeHtml(revision.commit)}"><span><strong>${escapeHtml(revision.commit.slice(0, 12))}</strong><small>${escapeHtml(revision.state)} · ${escapeHtml(revision.document_count)} documents</small>${failure}</span>${active}</button>`;
    })
    .join("");
}

export function renderCatalogDocuments(documents) {
  if (!Array.isArray(documents) || documents.length === 0) {
    return '<div class="empty">No validated runbook or policy documents in this revision.</div>';
  }
  return documents
    .map(
      (document) =>
        `<div class="catalog-document"><span class="status-pill">${escapeHtml(document.kind)}</span><span><strong>${escapeHtml(document.path)}</strong><small>checksum ${escapeHtml(document.checksum.slice(0, 12))}</small></span></div>`,
    )
    .join("");
}

function catalogSourceById(sourceId) {
  return state.catalogSources.find((source) => source.id === sourceId) || null;
}

function setCatalogActionPending(form, pending) {
  form.querySelectorAll('button[name="catalog-action"]').forEach((button) => {
    button.disabled = pending;
  });
}

function renderCatalogDetails() {
  const revisions = document.querySelector("#catalog-revisions");
  const documents = document.querySelector("#catalog-documents");
  const source = catalogSourceById(state.selectedCatalogSourceId);
  if (revisions) {
    revisions.innerHTML = renderCatalogRevisions(
      state.catalogRevisions,
      state.selectedCatalogCommit,
      source?.active_commit || "",
    );
  }
  if (documents) {
    documents.innerHTML = renderCatalogDocuments(state.catalogDocuments);
  }
}

async function loadCatalogDocuments(sourceId, commit) {
  const documents = document.querySelector("#catalog-documents");
  if (!documents || !sourceId || !commit) {
    return;
  }
  documents.innerHTML = '<div class="empty">Loading document metadata…</div>';
  try {
    const page = await api.listCatalogDocuments(sourceId, commit);
    if (state.selectedCatalogSourceId !== sourceId || state.selectedCatalogCommit !== commit) {
      return;
    }
    state.catalogDocuments = Array.isArray(page.items) ? page.items : [];
    documents.innerHTML = renderCatalogDocuments(state.catalogDocuments);
  } catch (error) {
    documents.innerHTML = '<div class="empty">Document metadata could not be loaded. Refresh or check permissions.</div>';
  }
}

async function loadCatalogRevisions(sourceId) {
  const revisions = document.querySelector("#catalog-revisions");
  const documents = document.querySelector("#catalog-documents");
  if (!revisions || !documents || !sourceId) {
    return;
  }
  revisions.innerHTML = '<div class="empty">Loading revisions…</div>';
  documents.innerHTML = '<div class="empty">Choose a revision to load document metadata.</div>';
  state.catalogDocuments = [];
  try {
    const page = await api.listCatalogRevisions(sourceId);
    if (state.selectedCatalogSourceId !== sourceId) {
      return;
    }
    state.catalogRevisions = Array.isArray(page.items) ? page.items : [];
    const source = catalogSourceById(sourceId);
    if (!state.catalogRevisions.some((revision) => revision.commit === state.selectedCatalogCommit)) {
      state.selectedCatalogCommit = source?.active_commit || state.catalogRevisions[0]?.commit || "";
    }
    renderCatalogDetails();
    await loadCatalogDocuments(sourceId, state.selectedCatalogCommit);
  } catch (error) {
    state.catalogRevisions = [];
    state.selectedCatalogCommit = "";
    revisions.innerHTML = '<div class="empty">Revisions could not be loaded. Refresh or check permissions.</div>';
  }
}

async function loadCatalogSources() {
  const list = document.querySelector("#catalog-list");
  if (!list || !state.token) return;
  list.innerHTML = '<div class="empty">Loading catalog sources…</div>';
  try {
    const page = await api.listCatalogSources();
    state.catalogSources = Array.isArray(page.items) ? page.items : [];
    if (!state.catalogSources.some((source) => source.id === state.selectedCatalogSourceId)) {
      state.selectedCatalogSourceId = state.catalogSources[0]?.id || "";
      state.selectedCatalogCommit = "";
    }
    list.innerHTML = renderCatalogSources(state.catalogSources, state.selectedCatalogSourceId);
    if (state.selectedCatalogSourceId) {
      await loadCatalogRevisions(state.selectedCatalogSourceId);
    } else {
      state.catalogRevisions = [];
      state.catalogDocuments = [];
      renderCatalogDetails();
    }
  } catch (error) {
    list.innerHTML = '<div class="empty">Catalog could not be loaded. Refresh or check permissions.</div>';
  }
}

async function submitCatalogAction(form, submitter) {
  if (state.catalogActionInFlight) return;
  const data = new FormData(form);
  const action = submitter instanceof HTMLButtonElement ? submitter.value : "";
  const sourceId = String(data.get("source-id") || "").trim();
  if (!sourceId) throw new Error("Catalog source ID is required.");
  state.catalogActionInFlight = true;
  setCatalogActionPending(form, true);
  setStatus(`Catalog ${action} request in progress…`);
  try {
    if (action === "register") {
      const url = String(data.get("url") || "").trim();
      const reference = String(data.get("reference") || "").trim();
      if (!url || !reference) throw new Error("A public HTTPS URL and reference are required to register a source.");
      await api.registerCatalogSource({ source_id: sourceId, url, reference });
    } else if (action === "sync") {
      const operationId = String(data.get("operation-id") || "").trim();
      if (!operationId) throw new Error("A sync operation ID is required.");
      await api.startCatalogSync(sourceId, { operation_id: operationId });
    } else if (action === "activate") {
      const commit = String(data.get("commit") || "").trim();
      if (!commit) throw new Error("A ready revision commit is required for activation.");
      await api.activateCatalogRevision(sourceId, { commit });
    } else {
      throw new Error("Choose a catalog action.");
    }
    state.selectedCatalogSourceId = sourceId;
    await loadCatalogSources();
    setStatus(`Catalog ${action} request completed.`, "ok");
  } finally {
    state.catalogActionInFlight = false;
    setCatalogActionPending(form, false);
  }
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
  const [approvals, jobs, remediations] = await Promise.all([
    api.listApprovals("pending"),
    api.listJobs(),
    api.listRemediations({ limit: DETAIL_PAGE_LIMIT }),
  ]);
  document.querySelector("#approvals-list").innerHTML = renderApprovals(approvals, jobs);
  wireApprovalButtons();
  document.querySelector("#jobs-list").innerHTML = renderJobs(jobs);
  state.remediations = Array.isArray(remediations) ? remediations : [];
  renderRemediationList();
  await loadAuditPage();
}

function renderRemediationList() {
  const list = document.querySelector("#remediations-list");
  if (!list) {
    return;
  }
  list.innerHTML = renderRemediations(state.remediations, state.selectedRemediationId);
}

function selectedRemediation() {
  return state.remediations.find((item) => item.id === state.selectedRemediationId) || null;
}

function syncRemediationForm(remediation) {
  if (!remediation) {
    return;
  }
  document.querySelector("#remediation-id").value = remediation.id || "";
  if (remediation.job_id) {
    document.querySelector("#remediation-job-id").value = remediation.job_id;
  }
  document.querySelector("#remediation-result").textContent = renderRemediationActionResult(remediation);
}

function readRemediationForm() {
  return {
    remediationId: document.querySelector("#remediation-id")?.value.trim() || "",
    approvalId: document.querySelector("#remediation-approval-id")?.value.trim() || "",
    jobId: document.querySelector("#remediation-job-id")?.value.trim() || "",
    runbookDocument: document.querySelector("#remediation-runbook-document")?.value || "",
  };
}

function optionalString(value) {
  const text = String(value ?? "").trim();
  return text ? text : undefined;
}

function requireRemediationId(form) {
  if (!form.remediationId) {
    throw new Error("Select or enter a remediation request first.");
  }
}

function requireRemediationJobId(form) {
  if (!form.jobId) {
    throw new Error("Enter the remediation job id.");
  }
}

export function renderRemediationActionResult(response) {
  const remediation = response?.remediation || response || {};
  const lines = [
    `remediation_id=${remediation.id || ""}`,
    `policy_id=${remediation.policy_id || ""}`,
    `agent_id=${remediation.agent_id || ""}`,
    `status=${remediation.status || ""}`,
    `runbook_ref=${remediation.runbook_ref || ""}`,
    `job_id=${remediation.job_id || response?.job_id || ""}`,
  ];
  if (response?.approval) {
    lines.push(`approval_id=${response.approval.id || ""}`);
    lines.push(`approval_status=${response.approval.status || ""}`);
  }
  if (response?.assignment_count !== undefined) {
    lines.push(`assignment_count=${response.assignment_count}`);
  }
  return lines.join("\n");
}

async function loadRemediations() {
  syncAdminTokenFromInput({ requireToken: true });
  const remediations = await api.listRemediations({ limit: DETAIL_PAGE_LIMIT });
  state.remediations = Array.isArray(remediations) ? remediations : [];
  if (
    state.selectedRemediationId &&
    !state.remediations.some((remediation) => remediation.id === state.selectedRemediationId)
  ) {
    state.selectedRemediationId = "";
  }
  renderRemediationList();
  if (state.selectedRemediationId) {
    syncRemediationForm(selectedRemediation());
  }
}

async function requestRemediationApproval() {
  syncAdminTokenFromInput({ requireToken: true });
  const form = readRemediationForm();
  requireRemediationId(form);
  const response = await api.createRemediationApprovalRequest(form.remediationId, {
    approval_id: optionalString(form.approvalId),
    job_id: optionalString(form.jobId),
    reason: "requested from Web Admin",
    expires_in_seconds: 300,
  });
  state.selectedRemediationId = response?.remediation?.id || form.remediationId;
  document.querySelector("#remediation-result").textContent = renderRemediationActionResult(response);
  setStatus(`Requested remediation approval for ${state.selectedRemediationId}.`, "ok");
  await loadApprovalsJobsAndAudit();
}

async function approveRemediationJob() {
  syncAdminTokenFromInput({ requireToken: true });
  const form = readRemediationForm();
  requireRemediationId(form);
  requireRemediationJobId(form);
  if (!form.approvalId) {
    throw new Error("Enter the remediation approval id.");
  }
  if (!form.runbookDocument.trim()) {
    throw new Error("Enter the approved remediation runbook YAML.");
  }
  const response = await api.approveRemediationJob(form.remediationId, {
    approval_id: form.approvalId,
    job_id: form.jobId,
    runbook_document: form.runbookDocument,
    timeout_seconds: 30,
    expires_in_seconds: 300,
    reason: "approved from Web Admin",
  });
  state.selectedRemediationId = response?.remediation?.id || form.remediationId;
  state.lastJobId = response?.job_id || form.jobId;
  document.querySelector("#remediation-result").textContent = renderRemediationActionResult(response);
  setStatus(`Created remediation job ${state.lastJobId}.`, "ok");
  await loadApprovalsJobsAndAudit();
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
  let decision = null;
  if (action === "approve") {
    decision = await api.approveApproval(id, { reason: reason || "" });
  } else {
    decision = await api.rejectApproval(id, { reason: reason || "" });
  }
  setStatus(`${action === "approve" ? "Approved" : "Rejected"} ${id}.`, "ok");
  await loadApprovalsJobsAndAudit();
  const jobIdToPoll = approvalDecisionJobToPoll(action, decision);
  if (jobIdToPoll) {
    state.lastJobId = jobIdToPoll;
    document.querySelector("#job-output").textContent = renderJobOutputStatus(
      {
        id: jobIdToPoll,
        status: "queued",
        dispatch_state: "created",
        target_agents: [],
      },
      { jobId: jobIdToPoll },
    );
    document.querySelector("#job-targets").innerHTML = renderJobTargetTable(null);
    await pollJobOutput(jobIdToPoll);
  }
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
  await loadAuditPage();
  setStatus(`Revoked agent ${label}.`, "ok");
}

async function submitStagedTrustBundle(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const request = buildStagedTrustBundleRequest({
    previousPublicKeyPath: data.get("previous-public-key-path"),
    agentIds: data.get("agent-ids"),
    batchSize: data.get("batch-size"),
    maxFailures: data.get("max-failures"),
    ackTimeoutSeconds: data.get("ack-timeout-seconds"),
  });
  const response = await api.stageControllerSigningTrustBundle(request);
  const resultElement = document.querySelector("#staged-trust-bundle-result");
  if (resultElement) {
    resultElement.classList.remove("empty");
    resultElement.innerHTML = renderStagedTrustBundleResult(response);
  }
  setStatus(
    `Ran staged signing tick: ${response?.rollout_state || "unknown"} (${response?.attempted_count ?? 0} attempted).`,
    "ok",
  );
  await Promise.all([loadSigningRotationStatus(), loadAuditPage()]);
}

async function previewTargets(selector, targetSelector) {
  syncAdminTokenFromInput({ requireToken: true });
  const request = buildSelectorPreviewRequest({ selector });
  const target = document.querySelector(targetSelector);
  if (target) {
    target.classList.remove("empty");
    target.innerHTML = '<div class="empty">Loading selector preview...</div>';
  }
  try {
    const preview = await api.previewSelector(request);
    if (target) {
      target.innerHTML = renderSelectorPreview(preview);
    }
    return preview;
  } catch (error) {
    if (target) {
      target.innerHTML = `<div class="preview-warning">${escapeHtml(error.message || "Selector preview failed.")}</div>`;
    }
    throw error;
  }
}

function requireDispatchablePreviewTargets(preview) {
  if (selectorPreviewSelectedCount(preview) < 1) {
    throw new Error("Selector preview returned no dispatchable targets.");
  }
}

async function submitCommand(form) {
  syncAdminTokenFromInput({ requireToken: true });
  const data = new FormData(form);
  const selector = data.get("selector");
  const targetSelector = String(selector ?? "").trim();
  if (targetSelector) {
    requireDispatchablePreviewTargets(await previewTargets(targetSelector, "#command-target-preview"));
  }
  const request = buildCommandJobRequest({
    agentId: state.selectedAgentId,
    selector,
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
  const selector = data.get("selector");
  const targetSelector = String(selector ?? "").trim();
  if (targetSelector) {
    requireDispatchablePreviewTargets(await previewTargets(targetSelector, "#runbook-target-preview"));
  }
  const request = buildRunbookJobRequest({
    agentId: state.selectedAgentId,
    selector,
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
  const artifactElement = document.querySelector("#job-artifacts");
  for (let attempt = 1; attempt <= JOB_OUTPUT_POLL_ATTEMPTS; attempt += 1) {
    const [job, chunks] = await Promise.all([api.getJob(jobId), api.getJobOutput(jobId)]);
    if (targetElement) {
      targetElement.innerHTML = renderJobTargetTable(job);
    }
    if (artifactElement) {
      artifactElement.innerHTML = renderJobArtifacts(job);
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
      if (isTerminalJob(job) || isApprovalPendingJob(job)) {
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
  if (artifactElement) {
    artifactElement.innerHTML = renderJobArtifacts(job);
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

async function handleJobArtifactClick(event) {
  const button = event.target?.closest?.("[data-artifact-id]");
  if (!button) {
    return;
  }
  const artifactId = button.dataset.artifactId || "";
  const jobId = button.dataset.artifactJobId || state.lastJobId || "";
  if (!artifactId || !jobId) {
    return;
  }
  const outputElement = document.querySelector("#job-output");
  if (outputElement) {
    outputElement.textContent = `Loading artifact ${artifactId}.`;
  }
  try {
    const artifact = await api.getJobArtifact(jobId, artifactId);
    if (outputElement) {
      outputElement.textContent = renderArtifactBody(artifact);
    }
  } catch (error) {
    if (outputElement) {
      outputElement.textContent = `Could not load artifact ${artifactId}. ${error.message}`;
    }
    setStatus(error.message, "error");
  }
}

function boot() {
  applyAdminRoute();
  globalThis.addEventListener?.("hashchange", () => applyAdminRoute({ focus: true }));
  const form = document.querySelector("#admin-auth");
  if (!form) {
    return;
  }
  const warning = document.querySelector("#transport-warning");
  if (warning && globalThis.location?.protocol === "http:") {
    warning.hidden = false;
  }
  document.querySelector("#job-artifacts")?.addEventListener("click", (event) => {
    handleJobArtifactClick(event).catch((error) => setStatus(error.message, "error"));
  });
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
    document.querySelector("#preview-command-targets")?.addEventListener("click", () => {
      const data = new FormData(runForm);
      previewTargets(data.get("selector"), "#command-target-preview")
        .then((preview) => {
          setStatus(
            `Previewed ${selectorPreviewSelectedCount(preview)} dispatchable target(s) from ${preview?.matched_count ?? 0} match(es).`,
            "ok",
          );
        })
        .catch((error) => setStatus(error.message, "error"));
    });
  }
  const runbookForm = document.querySelector("#runbook-form");
  if (runbookForm) {
    runbookForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitRunbook(runbookForm).catch((error) => setStatus(error.message, "error"));
    });
    document.querySelector("#preview-runbook-targets")?.addEventListener("click", () => {
      const data = new FormData(runbookForm);
      previewTargets(data.get("selector"), "#runbook-target-preview")
        .then((preview) => {
          setStatus(
            `Previewed ${selectorPreviewSelectedCount(preview)} dispatchable target(s) from ${preview?.matched_count ?? 0} match(es).`,
            "ok",
          );
        })
        .catch((error) => setStatus(error.message, "error"));
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
  document.querySelector("#refresh-signing-rotation")?.addEventListener("click", () => {
    try {
      syncAdminTokenFromInput({ requireToken: true });
      loadSigningRotationStatus()
        .then(() => setStatus("Refreshed controller signing status.", "ok"))
        .catch((error) => setStatus(error.message, "error"));
    } catch (error) {
      setStatus(error.message, "error");
    }
  });
  const stagedTrustBundleForm = document.querySelector("#staged-trust-bundle-form");
  if (stagedTrustBundleForm) {
    stagedTrustBundleForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitStagedTrustBundle(stagedTrustBundleForm).catch((error) => setStatus(error.message, "error"));
    });
  }
  document.querySelector("#expire-approvals")?.addEventListener("click", () => {
    expireDueApprovals().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#refresh-catalog")?.addEventListener("click", () => {
    loadCatalogSources().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#catalog-list")?.addEventListener("click", (event) => {
    const button = event.target?.closest?.("[data-catalog-source-id]");
    if (!(button instanceof HTMLButtonElement)) return;
    state.selectedCatalogSourceId = button.dataset.catalogSourceId || "";
    state.selectedCatalogCommit = "";
    document.querySelector("#catalog-list").innerHTML = renderCatalogSources(
      state.catalogSources,
      state.selectedCatalogSourceId,
    );
    loadCatalogRevisions(state.selectedCatalogSourceId).catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#catalog-revisions")?.addEventListener("click", (event) => {
    const button = event.target?.closest?.("[data-catalog-commit]");
    if (!(button instanceof HTMLButtonElement) || !state.selectedCatalogSourceId) return;
    state.selectedCatalogCommit = button.dataset.catalogCommit || "";
    renderCatalogDetails();
    loadCatalogDocuments(state.selectedCatalogSourceId, state.selectedCatalogCommit).catch((error) =>
      setStatus(error.message, "error"),
    );
  });
  const catalogActionForm = document.querySelector("#catalog-action-form");
  if (catalogActionForm instanceof HTMLFormElement) {
    catalogActionForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const submitter = event.submitter instanceof HTMLButtonElement ? event.submitter : null;
      submitCatalogAction(catalogActionForm, submitter).catch((error) => setStatus(error.message, "error"));
    });
  }
  document.querySelector("#refresh-audit")?.addEventListener("click", () => {
    loadAuditPage().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#load-more-audit")?.addEventListener("click", () => {
    loadAuditPage({ append: true }).catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#remediations-list")?.addEventListener("click", (event) => {
    const button = event.target?.closest?.("[data-remediation-id]");
    if (!button?.dataset?.remediationId) {
      return;
    }
    state.selectedRemediationId = button.dataset.remediationId;
    renderRemediationList();
    syncRemediationForm(selectedRemediation());
  });
  document.querySelector("#refresh-remediations")?.addEventListener("click", () => {
    loadRemediations()
      .then(() => setStatus("Refreshed remediation requests.", "ok"))
      .catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#request-remediation-approval")?.addEventListener("click", () => {
    requestRemediationApproval().catch((error) => setStatus(error.message, "error"));
  });
  document.querySelector("#approve-remediation")?.addEventListener("click", () => {
    approveRemediationJob().catch((error) => setStatus(error.message, "error"));
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
