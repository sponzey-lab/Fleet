import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const indexPath = join(root, "index.html");
const stylesPath = join(root, "styles.css");
const appPath = join(root, "app.js");
const clientPath = join(root, "api-client.js");
const schemaPath = join(root, "api.schema.json");
const tsconfigPath = join(root, "tsconfig.json");
const openapiPath = join(root, "..", "docs", "openapi.json");

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function normalizeSchemaPath(path) {
  return String(path).replaceAll(/\{[^}]+\}/g, "{param}");
}

function findCall(path, method = "GET") {
  return calls.find((call) => call.path === path && (call.options.method || "GET") === method);
}

const index = readFileSync(indexPath, "utf8");
const styles = readFileSync(stylesPath, "utf8");
const app = readFileSync(appPath, "utf8");
const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
const tsconfig = JSON.parse(readFileSync(tsconfigPath, "utf8"));
const openapi = JSON.parse(readFileSync(openapiPath, "utf8"));

assert(index.includes("Sponzey Fleet Admin"), "index must name the admin UI");
assert(index.includes("id=\"agents-list\""), "index must expose the agents surface");
assert(index.includes("id=\"agent-detail\""), "index must expose selected agent detail");
assert(index.includes("id=\"revoke-agent-key\""), "index must expose agent key revocation");
assert(index.includes(">Revoke Agent</button>"), "index must label agent revocation by agent");
assert(index.includes("id=\"transport-warning\""), "index must expose HTTP transport warning");
assert(index.includes("id=\"signing-rotation-panel\""), "index must expose controller signing surface");
assert(index.includes("id=\"signing-rotation-status\""), "index must expose signing rotation status");
assert(index.includes("id=\"refresh-signing-rotation\""), "index must expose signing rotation refresh action");
assert(index.includes("id=\"staged-trust-bundle-form\""), "index must expose staged trust bundle form");
assert(index.includes("id=\"staged-trust-bundle-result\""), "index must expose staged trust bundle result");
assert(index.includes("id=\"refresh-telemetry\""), "index must expose selected agent telemetry refresh");
assert(index.includes("id=\"facts-chart\""), "index must expose facts trend charts");
assert(index.includes("id=\"disk-inventory\""), "index must expose disk and mount inventory");
assert(index.includes("id=\"facts-panel\""), "index must expose the facts surface");
assert(index.includes("id=\"metrics-range\""), "index must expose metrics range selector");
assert(index.includes("id=\"metrics-chart\""), "index must expose metrics trend charts");
assert(index.includes("id=\"metrics-panel\""), "index must expose the metrics surface");
assert(index.includes("id=\"drift-panel\""), "index must expose the drift surface");
assert(index.includes("id=\"drift-history\""), "index must expose drift history");
assert(index.includes("id=\"audit-list\""), "index must expose the audit surface");
assert(index.includes("id=\"run-command-form\""), "index must expose command execution");
assert(index.includes('value="uptime"'), "run command form must default to a safe probe command");
assert(index.includes("id=\"command-selector\""), "run command form must expose selector input");
assert(index.includes("id=\"preview-command-targets\""), "run command form must expose target preview action");
assert(index.includes("id=\"command-target-preview\""), "run command form must expose target preview result");
assert(index.includes("id=\"job-targets\""), "index must expose target assignment table");
assert(index.includes("id=\"job-artifacts\""), "index must expose rendered artifact list");
assert(index.includes("id=\"job-output\""), "index must expose job output");
assert(index.includes("Run a command or select a job"), "job output placeholder must not look like a completed empty result");
assert(index.includes("id=\"jobs-list\""), "index must expose job history");
assert(index.includes("id=\"approvals-list\""), "index must expose approval queue");
assert(index.includes("id=\"remediations-list\""), "index must expose remediation queue");
assert(index.includes("id=\"remediation-form\""), "index must expose remediation action form");
assert(index.includes("id=\"remediation-runbook-document\""), "remediation approve form must expose request-only runbook input");
assert(index.includes("id=\"remediation-result\""), "index must expose remediation action result");
assert(index.includes("id=\"runbook-form\""), "index must expose runbook job form");
assert(index.includes("id=\"runbook-selector\""), "runbook form must expose selector input");
assert(index.includes("id=\"preview-runbook-targets\""), "runbook form must expose target preview action");
assert(index.includes("id=\"runbook-target-preview\""), "runbook form must expose target preview result");
assert(index.includes("id=\"runbook-result\""), "index must expose runbook result status");
assert(index.includes("id=\"policy-form\""), "index must expose policy save form");
assert(index.includes("id=\"policies-list\""), "index must expose policy list");
assert(index.includes("id=\"agent-policies\""), "index must expose selected agent policies");
assert(index.includes("id=\"agent-logs\""), "index must expose agent log viewer");
assert(index.includes("id=\"enrollment-token-form\""), "index must expose enrollment token creation");
assert(index.includes("id=\"enrollment-tokens-list\""), "index must expose enrollment token summaries");
assert(index.includes("id=\"created-enrollment-token\""), "index must expose one-time token output");
assert(index.includes("/admin/app.js"), "index must load the dependency-free app script from the admin base path");
assert(index.includes("/admin/styles.css"), "index must load styles from the admin base path");
assert(index.includes('method="post"'), "admin auth form must not leak tokens through a query string fallback");
assert(!index.includes("localStorage"), "UI must not store tokens in localStorage");
assert(!index.includes("runtime config"), "UI must not expose runtime config mutation");
assert(styles.includes(".layout"), "styles must include the admin layout");
assert(styles.includes(".warning-banner"), "styles must include HTTP warning banner");
assert(styles.includes(".snapshot-time"), "styles must include snapshot time metadata");
assert(styles.includes(".data-table"), "styles must include tabular assignment/inventory views");
assert(styles.includes(".artifact-list"), "styles must include artifact list styling");
assert(styles.includes(".signing-grid"), "styles must include signing status grid styling");
assert(styles.includes(".signing-form"), "styles must include staged signing form styling");
assert(styles.includes(".target-preview"), "styles must include selector preview surface");
assert(styles.includes(".preview-warning"), "styles must include selector preview warnings");
assert(styles.includes(".approval-row"), "styles must include approval queue rows");
assert(styles.includes(".chart-grid"), "styles must include telemetry chart grid");
assert(styles.includes(".sparkline"), "styles must include telemetry sparkline styling");
assert(app.includes("./api-client.js"), "app must use the shared API client");
assert(app.includes("handleAgentsListClick"), "app must use delegated agent selection handling");
assert(app.includes("TELEMETRY_PAGE_LIMIT"), "app must fetch bounded telemetry history pages");
assert(app.includes("Loading selector preview"), "app must render an explicit selector preview loading state");
assert(
  !app.includes('querySelectorAll("[data-agent-id]")'),
  "app must not attach per-render agent button handlers",
);
assert(tsconfig.compilerOptions.checkJs, "tsconfig must enable JS type checking");
assert(schema.schema_version === "mvp-1", "API schema version must match MVP client");
for (const endpoint of [
  "listAgents",
  "getLatestFacts",
  "listFacts",
  "getLatestMetrics",
  "listMetrics",
  "listAgentLogs",
  "getLatestDrift",
  "listDrift",
  "listPolicies",
  "savePolicy",
  "assignPolicy",
  "schedulePolicyDrift",
  "listAgentPolicies",
  "listDueScheduledDrift",
  "revokeAgentKey",
  "getControllerSigningRotationStatus",
  "stageControllerSigningTrustBundle",
  "listJobs",
  "listApprovals",
  "approveApproval",
  "rejectApproval",
  "expireApprovals",
  "listRemediations",
  "getRemediation",
  "createRemediationApprovalRequest",
  "approveRemediationJob",
  "markRemediationRunning",
  "recordRemediationResult",
  "verifyRemediation",
  "previewSelector",
  "getJob",
  "getJobOutput",
  "getJobArtifact",
  "cancelJob",
  "listAudit",
  "listEnrollmentTokens",
  "createEnrollmentToken",
  "revokeEnrollmentToken",
  "createCommandJob",
  "createDriftCheckJob",
  "createRunbookJob",
]) {
  assert(
    schema.endpoints.some((entry) => entry.name === endpoint),
    `API schema must include ${endpoint}`,
  );
}
const openapiOperations = new Set();
for (const [path, operations] of Object.entries(openapi.paths || {})) {
  for (const method of Object.keys(operations || {})) {
    openapiOperations.add(`${method.toUpperCase()} ${normalizeSchemaPath(path)}`);
  }
}
for (const endpoint of schema.endpoints) {
  const key = `${endpoint.method} ${normalizeSchemaPath(endpoint.path)}`;
  assert(openapiOperations.has(key), `OpenAPI must document ${endpoint.method} ${endpoint.path}`);
}

const {
  renderAgents,
  renderAgentDetail,
  renderSnapshot,
  renderFactsInventory,
  renderDiskInventory,
  renderDrift,
  renderDriftHistory,
  renderAudit,
  formatApiError,
  parseCommandArgs,
  buildCommandJobRequest,
  buildRunbookJobRequest,
  buildSelectorPreviewRequest,
  renderSelectorPreview,
  selectorPreviewSelectedCount,
  buildPolicySaveRequest,
  buildPolicyAssignmentRequest,
  buildPolicyScheduleRequest,
  renderJobOutput,
  renderJobOutputWaiting,
  renderJobOutputEmpty,
  renderJobOutputStatus,
  renderJobOutputAfterPolling,
  renderJobTargetTable,
  renderJobArtifacts,
  renderArtifactBody,
  jobStatusMessage,
  isTerminalJob,
  isApprovalPendingJob,
  approvalDecisionJobToPoll,
  renderRemediations,
  renderRemediationActionResult,
  pollJobOutputOnce,
  renderJobs,
  renderApprovals,
  renderPolicies,
  renderAgentPolicies,
  renderAgentLogs,
  renderEnrollmentTokens,
  renderCreatedEnrollmentToken,
  buildEnrollmentTokenRequest,
  renderControllerSigningRotationStatus,
  buildStagedTrustBundleRequest,
  renderStagedTrustBundleResult,
  formatUnixMillis,
  renderTelemetryCharts,
  recentSnapshots,
  snapshotTimeMs,
  formatKilobytes,
  memoryUsedPercent,
  agentDisplayStatus,
} = await import(appPath);
const { API_SCHEMA_VERSION, createApiClient, normalizeAdminToken } = await import(clientPath);
assert(API_SCHEMA_VERSION === schema.schema_version, "API client and schema versions must match");
assert(normalizeAdminToken(" admin-token \n") === "admin-token", "client must trim admin tokens");
assert(
  normalizeAdminToken("Bearer admin-token") === "admin-token",
  "client must accept pasted bearer tokens",
);

const calls = [];
const client = createApiClient({
  tokenProvider: () => "admin-token",
  fetchImpl: async (path, options) => {
    calls.push({ path, options });
    return {
      ok: true,
      status: 200,
      json: async () => ({ path }),
    };
  },
});
await client.listAgents();
await client.getLatestFacts("agent/1");
await client.listFacts("agent/1", { limit: 25, before: "2:10" });
await client.getLatestMetrics("agent/1");
await client.listMetrics("agent/1", { limit: 10 });
await client.listAgentLogs("agent/1", { limit: 5, before: "3:8" });
await client.getLatestDrift("agent/1");
await client.listDrift("agent/1", { before: "2:9" });
await client.listPolicies();
await client.savePolicy({ source: "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Policy\n" });
await client.assignPolicy("policy/1", { agent_id: "agent/1" });
await client.schedulePolicyDrift("policy/1", { agent_id: "agent/1", interval_seconds: 300 });
await client.listAgentPolicies("agent/1");
await client.listDueScheduledDrift();
await client.revokeAgentKey("agent/1");
await client.getControllerSigningRotationStatus();
await client.stageControllerSigningTrustBundle({
  previous_public_key_path: "/var/lib/sponzey-fleet/controller/controller_public.key.bak",
  agent_ids: ["agent-1"],
  batch_size: 1,
  max_failures: 1,
  ack_timeout_seconds: 30,
});
await client.listJobs();
await client.listApprovals("pending");
await client.approveApproval("approval/1", { actor: "manager-1", reason: "ok" });
await client.rejectApproval("approval/2", { actor: "manager-1", reason: "not ok" });
await client.expireApprovals();
await client.listRemediations({ agentId: "agent/1", policyId: "nginx running", limit: 10 });
await client.getRemediation("remediation/1");
await client.createRemediationApprovalRequest("remediation/1", { approval_id: "approval-1" });
await client.approveRemediationJob("remediation/1", { approval_id: "approval-1", job_id: "job-1" });
await client.markRemediationRunning("remediation/1", { job_id: "job-1" });
await client.recordRemediationResult("remediation/1", { job_id: "job-1", status: "succeeded" });
await client.verifyRemediation("remediation/1", {
  agent_id: "agent/1",
  policy_id: "nginx-running",
  policy_name: "nginx-running",
  job_id: "job-1",
});
await client.previewSelector({ matchLabels: { role: "web" } });
await client.getJob("job/1");
await client.getJobOutput("job/1");
await client.getJobArtifact("job/1", "artifact/1");
await client.cancelJob("job/1", { reason: "operator requested cancel" });
await client.listAudit();
await client.listEnrollmentTokens();
await client.createEnrollmentToken({ labels: "role=web", max_uses: 1, expires_in_seconds: 60 });
await client.revokeEnrollmentToken("et/1");
await client.createCommandJob({ job_id: "job-1" });
await client.createDriftCheckJob({ job_id: "job-drift-1" });
await client.createRunbookJob({ job_id: "job-runbook-1" });
assert(findCall("/api/agents"), "client must call agents endpoint");
assert(
  findCall("/api/agents/agent%2F1/facts/latest"),
  "client must encode agent ids in paths",
);
assert(
  findCall("/api/agents/agent%2F1/facts?limit=25&before=2%3A10"),
  "client must encode paged facts query",
);
assert(
  findCall("/api/agents/agent%2F1/metrics/latest"),
  "client must encode latest metrics paths",
);
assert(
  findCall("/api/agents/agent%2F1/metrics?limit=10"),
  "client must encode paged metrics query",
);
assert(
  findCall("/api/agents/agent%2F1/logs?limit=5&before=3%3A8"),
  "client must encode paged agent log query",
);
assert(
  findCall("/api/agents/agent%2F1/drift/latest"),
  "client must encode latest drift paths",
);
assert(
  findCall("/api/agents/agent%2F1/drift?before=2%3A9"),
  "client must encode paged drift query",
);
assert(findCall("/api/policies"), "client must call policy list endpoint");
assert(findCall("/api/policies", "POST"), "client must call policy save endpoint");
assert(findCall("/api/policies", "POST").options.method === "POST", "client must POST policy saves");
assert(
  findCall("/api/policies/policy%2F1/assignments", "POST"),
  "client must encode policy ids in assignment paths",
);
assert(
  findCall("/api/policies/policy%2F1/schedules", "POST"),
  "client must encode policy ids in schedule paths",
);
assert(
  findCall("/api/agents/agent%2F1/policies"),
  "client must encode agent ids in policy assignment list paths",
);
assert(findCall("/api/drift/scheduled"), "client must call scheduled drift endpoint");
assert(findCall("/api/selectors/preview", "POST"), "client must call selector preview endpoint");
assert(
  findCall("/api/agents/agent%2F1/revoke-key", "POST"),
  "client must encode agent ids in key revocation paths",
);
assert(
  findCall("/api/agents/agent%2F1/revoke-key", "POST").options.method === "POST",
  "client must POST agent key revocation",
);
assert(
  findCall("/api/controller/signing-rotation/status"),
  "client must call signing rotation status endpoint",
);
assert(
  findCall("/api/controller/signing-rotation/rollout-trust-bundle/staged", "POST"),
  "client must call staged signing rollout endpoint",
);
assert(
  findCall("/api/controller/signing-rotation/rollout-trust-bundle/staged", "POST").options.method === "POST",
  "client must POST staged signing rollout ticks",
);
assert(
  JSON.parse(findCall("/api/controller/signing-rotation/rollout-trust-bundle/staged", "POST").options.body)
    .batch_size === 1,
  "client must serialize staged signing rollout body",
);
assert(findCall("/api/jobs"), "client must call jobs list endpoint");
assert(findCall("/api/approvals?status=pending"), "client must call filtered approvals endpoint");
assert(
  findCall("/api/approvals/approval%2F1/approve", "POST"),
  "client must encode approval ids in approve paths",
);
assert(
  findCall("/api/approvals/approval%2F1/approve", "POST").options.method === "POST",
  "client must POST approval decisions",
);
assert(
  findCall("/api/approvals/approval%2F2/reject", "POST"),
  "client must encode approval ids in reject paths",
);
assert(
  findCall("/api/approvals/approval%2F2/reject", "POST").options.method === "POST",
  "client must POST rejection decisions",
);
assert(findCall("/api/approvals/expire", "POST"), "client must call approval expiry endpoint");
assert(
  findCall("/api/remediations?agent_id=agent%2F1&policy_id=nginx+running&limit=10"),
  "client must call filtered remediation list endpoint",
);
assert(
  findCall("/api/remediations/remediation%2F1"),
  "client must encode remediation ids in detail paths",
);
assert(
  findCall("/api/remediations/remediation%2F1/approval-request", "POST"),
  "client must POST remediation approval requests",
);
assert(
  findCall("/api/remediations/remediation%2F1/approve", "POST"),
  "client must POST remediation job approval",
);
assert(
  findCall("/api/remediations/remediation%2F1/running", "POST"),
  "client must POST remediation running transitions",
);
assert(
  findCall("/api/remediations/remediation%2F1/result", "POST"),
  "client must POST remediation result",
);
assert(
  findCall("/api/remediations/remediation%2F1/verify", "POST"),
  "client must POST remediation verification",
);
assert(findCall("/api/jobs/job%2F1"), "client must call job detail endpoint");
assert(findCall("/api/jobs/job%2F1/output"), "client must call job output endpoint");
assert(findCall("/api/jobs/job%2F1/artifacts/artifact%2F1"), "client must call encoded artifact endpoint");
assert(findCall("/api/jobs/job%2F1/cancel", "POST"), "client must call job cancel endpoint");
assert(findCall("/api/jobs/job%2F1/cancel", "POST").options.method === "POST", "client must POST job cancel");
assert(findCall("/api/audit"), "client must call audit endpoint");
assert(findCall("/api/enrollment-tokens"), "client must call token list endpoint");
assert(findCall("/api/enrollment-tokens", "POST"), "client must call token create endpoint");
assert(findCall("/api/enrollment-tokens", "POST").options.method === "POST", "client must POST token creation");
assert(findCall("/api/enrollment-tokens/et%2F1", "DELETE"), "client must encode token ids in paths");
assert(
  findCall("/api/enrollment-tokens/et%2F1", "DELETE").options.method === "DELETE",
  "client must DELETE token revocation",
);
assert(findCall("/api/jobs/command", "POST"), "client must call command job endpoint");
assert(findCall("/api/jobs/command", "POST").options.method === "POST", "client must POST command jobs");
assert(findCall("/api/jobs/drift-check", "POST"), "client must call drift check job endpoint");
assert(findCall("/api/jobs/drift-check", "POST").options.method === "POST", "client must POST drift check jobs");
assert(findCall("/api/jobs/runbook", "POST"), "client must call runbook job endpoint");
assert(findCall("/api/jobs/runbook", "POST").options.method === "POST", "client must POST runbook jobs");
assert(
  findCall("/api/enrollment-tokens").options.headers.Authorization === "Bearer admin-token",
  "client must attach bearer token",
);

const bearerCalls = [];
const bearerClient = createApiClient({
  tokenProvider: () => " Bearer admin-token \n",
  fetchImpl: async (path, options) => {
    bearerCalls.push({ path, options });
    return {
      ok: true,
      status: 200,
      json: async () => ({ path }),
    };
  },
});
await bearerClient.listEnrollmentTokens();
assert(
  bearerCalls[0].options.headers.Authorization === "Bearer admin-token",
  "client must normalize pasted bearer tokens before sending",
);

const unauthenticatedCalls = [];
const unauthenticatedClient = createApiClient({
  tokenProvider: () => "",
  fetchImpl: async (path, options) => {
    unauthenticatedCalls.push({ path, options });
    return {
      ok: false,
      status: 401,
      json: async () => ({ error: "unauthorized" }),
    };
  },
});
try {
  await unauthenticatedClient.listEnrollmentTokens();
} catch {
  // The request is expected to fail, but it must not send a blank bearer header.
}
assert(
  !("Authorization" in unauthenticatedCalls[0].options.headers),
  "client must not attach a blank bearer token",
);

const notFoundClient = createApiClient({
  tokenProvider: () => "admin-token",
  fetchImpl: async () => ({
    ok: false,
    status: 404,
    json: async () => ({ error: "not_found" }),
  }),
});
assert(
  (await notFoundClient.getLatestDrift("agent-1")) === null,
  "client must treat missing optional agent data as null",
);

const agentsHtml = renderAgents([
  {
    id: "agent-1",
    name: "web-01",
    status: "online",
    revoked: false,
    labels: [{ key: "role", value: "web" }],
    connected: true,
    hostname: "web-01.local",
    os: "linux",
    arch: "x86_64",
    last_seen_age_seconds: 5,
  },
]);
assert(agentsHtml.includes("web-01"), "agents renderer must include agent name");
assert(agentsHtml.includes("role=web"), "agents renderer must include labels");
assert(agentsHtml.includes("linux/x86_64"), "agents renderer must include platform summary");
assert(agentsHtml.includes("session connected"), "agents renderer must include session state");
assert(agentsHtml.includes("last seen 5s ago"), "agents renderer must include last seen age");

const revokedAgentHtml = renderAgents([
  {
    id: "agent-revoked",
    name: "revoked-agent",
    status: "disabled",
    revoked: true,
    labels: [],
  },
]);
assert(
  revokedAgentHtml.includes('status-pill offline">offline'),
  "revoked agents must be displayed as offline",
);
assert(
  revokedAgentHtml.includes('status-pill revoked">revoked'),
  "revoked agents must include a revoked badge",
);
const staleAgentHtml = renderAgents([
  {
    id: "agent-stale",
    name: "stale-agent",
    status: "reconnecting",
    revoked: false,
    connected: false,
    labels: [],
  },
]);
assert(agentDisplayStatus({ status: "reconnecting" }) === "stale", "reconnecting agents must render as stale");
assert(staleAgentHtml.includes('status-pill stale">stale'), "stale agents must use an explicit stale badge");
const detailHtml = renderAgentDetail({
  id: "agent-1",
  status: "online",
  connected: true,
  revoked: false,
  hostname: "web-01",
  os: "linux",
  arch: "x86_64",
  assigned_policy_ids: ["nginx-running"],
  capabilities: ["persistent_session", "command_execution"],
  capability_reported_at_ms: 2000,
});
assert(detailHtml.includes("Assigned policies"), "agent detail must include policy assignment summary");
assert(detailHtml.includes("nginx-running"), "agent detail must render assigned policy ids");
assert(detailHtml.includes("Capabilities"), "agent detail must include capability summary");
assert(detailHtml.includes("command_execution"), "agent detail must render reported capabilities");
assert(
  detailHtml.includes("1970-01-01T00:00:02.000Z"),
  "agent detail must render capability reported time",
);

const factsText = renderSnapshot(
  {
    collected_at_ms: 1000,
    agent_system_time_ms: 2000,
    body: { system_time_ms: 2000, os: "linux", disk: { root_capacity_known: true } },
  },
  "",
);
assert(factsText.includes("Agent time: 1970-01-01T00:00:02.000Z"), "facts renderer must show agent time");
assert(factsText.includes("Stored at: 1970-01-01T00:00:01.000Z"), "facts renderer must show stored time");
assert(factsText.includes("\"os\": \"linux\""), "facts renderer must show snapshot JSON");
assert(
  formatUnixMillis(2000) === "1970-01-01T00:00:02.000Z (2000 ms)",
  "time formatter must render epoch millis as ISO text",
);
assert(
  snapshotTimeMs({ agent_system_time_ms: 3000, collected_at_ms: 2000, body: { system_time_ms: 1000 } }) === 3000,
  "snapshot time must prefer agent system time",
);
const factsInventoryHtml = renderFactsInventory({
  body: {
    os: "linux",
    arch: "x64",
    hostname: "agent-01",
    cpu: { logical_count: 8 },
    memory: { total_kb: 16 * 1024 * 1024, module_count_known: true, module_count: 2 },
    disk: {
      device_count: 2,
      mount_count: 4,
      root_total_kb: 100 * 1024 * 1024,
      root_filesystem: "/dev/root",
      root_fs_type: "ext4",
    },
    network: { interfaces: ["lo", "eth0"] },
  },
});
assert(factsInventoryHtml.includes("Memory total"), "facts inventory must show static memory capacity");
assert(factsInventoryHtml.includes("16.0 GiB"), "facts inventory must format memory capacity");
assert(factsInventoryHtml.includes("Memory modules"), "facts inventory must show memory module count");
assert(factsInventoryHtml.includes("Disk devices"), "facts inventory must show disk device count");
assert(factsInventoryHtml.includes("Mounts"), "facts inventory must show mount count");
assert(factsInventoryHtml.includes("Root disk total"), "facts inventory must show static disk capacity");
assert(factsInventoryHtml.includes("Root FS type"), "facts inventory must show root filesystem type");
assert(!factsInventoryHtml.includes("used_percent"), "facts inventory must not render usage fields");
assert(formatKilobytes(1536) === "1.5 MiB", "kilobyte formatter must scale capacity values");
assert(
  memoryUsedPercent({ memory: { used_kb: 25, total_kb: 100 } }) === 25,
  "memory usage helper must calculate usage percent",
);
const diskInventoryHtml = renderDiskInventory({
  body: {
    disk: {
      devices: [
        {
          name: "sda",
          kind: "disk",
          size_kb: 1024 * 1024,
          partitions: [{ name: "sda1", size_kb: 512 * 1024 }],
        },
      ],
      mounts: [
        { source: "/dev/sda1", mount_point: "/", fs_type: "ext4" },
        { source: "tmpfs", mount_point: "/run", fs_type: "tmpfs" },
      ],
    },
  },
});
assert(diskInventoryHtml.includes("sda"), "disk inventory must show block devices");
assert(diskInventoryHtml.includes("sda1"), "disk inventory must show partitions");
assert(diskInventoryHtml.includes("/run"), "disk inventory must show mount points");
const recent = recentSnapshots(
  [
    { agent_system_time_ms: 0, body: { memory: { used_kb: 1, total_kb: 2 } } },
    { agent_system_time_ms: 180_000, body: { memory: { used_kb: 1, total_kb: 2 } } },
    { agent_system_time_ms: 420_000, body: { memory: { used_kb: 1, total_kb: 2 } } },
  ],
  300_000,
);
assert(recent.length === 2, "recent snapshot filter must keep only the latest five minute window");
const chartHtml = renderTelemetryCharts(
  [
    {
      agent_system_time_ms: 1000,
      body: {
        cpu: { usage_percent: 20 },
        memory: { used_kb: 50, total_kb: 100 },
        disk: { used_percent: 30 },
      },
    },
    {
      agent_system_time_ms: 2000,
      body: {
        cpu: { usage_percent: 25 },
        memory: { used_kb: 60, total_kb: 100 },
        disk: { used_percent: 40 },
      },
    },
  ],
  [
    { label: "CPU used", unit: "%", read: (body) => body.cpu.usage_percent },
    { label: "Memory used", unit: "%", read: (body) => (body.memory.used_kb / body.memory.total_kb) * 100 },
    { label: "Disk used", unit: "%", read: (body) => body.disk.used_percent },
  ],
);
assert(chartHtml.includes("Last 5 minutes"), "telemetry chart must label the five minute window");
assert(chartHtml.includes("CPU used"), "telemetry chart must include CPU usage");
assert(chartHtml.includes("Memory used"), "telemetry chart must include memory usage");
assert(chartHtml.includes("<svg"), "telemetry chart must render SVG sparklines");
const chartRangeHtml = renderTelemetryCharts(
  [
    { agent_system_time_ms: 1000, body: { cpu: { usage_percent: 10 } } },
    { agent_system_time_ms: 600000, body: { cpu: { usage_percent: 20 } } },
  ],
  [{ label: "CPU used", unit: "%", read: (body) => body.cpu.usage_percent }],
  "empty",
  900000,
);
assert(chartRangeHtml.includes("Last 15 minutes"), "metrics chart must reflect selected range");

const driftHtml = renderDrift({
  policy_name: "nginx-running",
  status: "drifted",
  expected: "service nginx running",
  actual: "service nginx stopped",
  checked_at_ms: 1000,
  agent_system_time_ms: 2000,
  acknowledged: false,
  resolved: false,
});
assert(driftHtml.includes("Agent time 1970-01-01T00:00:02.000Z"), "drift renderer must include agent time");
assert(driftHtml.includes("Expected"), "drift renderer must include expected section");
assert(driftHtml.includes("service nginx stopped"), "drift renderer must include actual detail");
assert(driftHtml.includes("not resolved"), "drift renderer must show remediation state");
const driftHistoryHtml = renderDriftHistory({
  items: [
    {
      policy_name: "nginx-running",
      status: "drifted",
      checked_at_ms: 1000,
      agent_system_time_ms: 2000,
      resolved: true,
      resolution_job_id: "job-remediate-1",
    },
  ],
});
assert(driftHistoryHtml.includes("nginx-running"), "drift history must include policy names");
assert(driftHistoryHtml.includes("drifted"), "drift history must include status");
assert(driftHistoryHtml.includes("job-remediate-1"), "drift history must show remediation resolution job");

const auditHtml = renderAudit([
  { category: "security", action: "invalid_signature", actor: "system", target: "agent-1", value_kind: "redacted", value: "redacted" },
]);
assert(auditHtml.includes("invalid_signature"), "audit renderer must include event action");
const signingStatusHtml = renderControllerSigningRotationStatus({
  controller_id: "default-controller",
  persisted_record_present: true,
  persisted_state: "dual_trust_active",
  readiness: "dual_trust_active_agents_migrating",
  active_signing_fingerprint_prefix: "new-fp-12345678",
  selected_signing_fingerprint_prefix: "new-fp-12345678",
  old_fingerprint_prefix: "old-fp-12345678",
  new_fingerprint_prefix: "new-fp-12345678",
  requested_at_ms: 1000,
  validated_at_ms: 2000,
  activated_at_ms: 3000,
  old_key_verifies_until_ms: 4000,
  retired_at_ms: null,
  failed_at_ms: null,
  bootstrap_guard: "active_matches_selected",
  agent_trust_rollout: "agents_migrating",
  previous_public_key_path: "/var/lib/sponzey-fleet/controller/controller_public.key.bak",
  controller_public_key: "-----BEGIN PUBLIC KEY-----secret-marker",
  admin_token: "admin-token-secret",
});
assert(signingStatusHtml.includes("dual_trust_active"), "signing status must render persisted state");
assert(signingStatusHtml.includes("agents_migrating"), "signing status must render agent rollout state");
assert(signingStatusHtml.includes("new-fp-12345678"), "signing status must render fingerprint prefixes");
assert(!signingStatusHtml.includes("BEGIN PUBLIC KEY"), "signing status must not render key bodies");
assert(!signingStatusHtml.includes("controller_public.key.bak"), "signing status must not render local key paths");
assert(!signingStatusHtml.includes("admin-token-secret"), "signing status must not render admin tokens");
const stagedRequest = buildStagedTrustBundleRequest({
  previousPublicKeyPath: " /var/lib/sponzey-fleet/controller/controller_public.key.bak ",
  agentIds: "agent-1, agent-2\nagent-3",
  batchSize: "2",
  maxFailures: "1",
  ackTimeoutSeconds: "30",
});
assert(stagedRequest.previous_public_key_path?.endsWith("controller_public.key.bak"), "staged request must trim previous public key path");
assert(stagedRequest.agent_ids.length === 3, "staged request must parse explicit agent ids");
assert(stagedRequest.batch_size === 2, "staged request must parse batch size");
assert(stagedRequest.max_failures === 1, "staged request must parse max failures");
assert(stagedRequest.ack_timeout_seconds === 30, "staged request must parse ack timeout");
let invalidStagedRequestFailed = false;
try {
  buildStagedTrustBundleRequest({ batchSize: "0", maxFailures: "0", ackTimeoutSeconds: "30" });
} catch {
  invalidStagedRequestFailed = true;
}
assert(invalidStagedRequestFailed, "staged request must reject invalid batch size");
const stagedResultHtml = renderStagedTrustBundleResult({
  controller_id: "default-controller",
  persisted_state: "dual_trust_active",
  rollout_state: "waiting_for_ack",
  target_count: 3,
  planned_count: 1,
  attempted_count: 1,
  updated_count: 1,
  skipped_count: 1,
  failed_count: 0,
  already_current_count: 1,
  unavailable_count: 0,
  pending_count: 1,
  entries_count: 2,
  current_fingerprint_prefix: "new-fp-12345678",
  previous_fingerprint_prefix: "old-fp-12345678",
  agent_results: [{ agent_id: "agent-2", status: "sent" }],
  previous_public_key_path: "/var/lib/sponzey-fleet/controller/controller_public.key.bak",
  controller_public_key: "-----BEGIN PUBLIC KEY-----secret-marker",
  admin_token: "admin-token-secret",
});
assert(stagedResultHtml.includes("waiting_for_ack"), "staged result must render rollout state");
assert(stagedResultHtml.includes("agent-2"), "staged result must render agent result ids");
assert(stagedResultHtml.includes("sent"), "staged result must render agent result status");
assert(!stagedResultHtml.includes("BEGIN PUBLIC KEY"), "staged result must not render key bodies");
assert(!stagedResultHtml.includes("controller_public.key.bak"), "staged result must not render local key paths");
assert(!stagedResultHtml.includes("admin-token-secret"), "staged result must not render admin tokens");
const jobsHtml = renderJobs([
  {
    id: "job-1",
    status: "running",
    dispatch_state: "delivered",
    risk: "high",
    command_program: "uptime",
    command_args: ["-a"],
    target_count: 1,
    assignment_summary: {
      queued: 0,
      dispatched: 0,
      accepted: 0,
      started: 1,
      succeeded: 0,
      failed: 0,
      rejected: 0,
      canceled: 0,
      expired: 0,
      skipped: 0,
      unknown: 0,
    },
    target_agents: [
      {
        agent_id: "agent-1",
        status: "online",
        connected: true,
        revoked: false,
        task_id: "task-1",
        assignment_status: "started",
        last_error: "",
      },
    ],
  },
]);
assert(jobsHtml.includes("job-1"), "job renderer must include job id");
assert(jobsHtml.includes("uptime -a"), "job renderer must include command summary");
assert(jobsHtml.includes("delivered"), "job renderer must include controller dispatch state");
assert(jobsHtml.includes("1 target(s), 1 connected"), "job renderer must use API target connection state");
assert(jobsHtml.includes("1 started"), "job renderer must include per-target assignment state");
const artifactsHtml = renderJobArtifacts({
  id: "job-1",
  rendered_artifacts: [
    {
      artifact_id: "artifact-1",
      task_id: "task-1",
      agent_id: "agent-1",
      retention_class: "rendered_template",
      checksum_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      size_bytes: 13,
      destination: "/etc/secret.conf",
      content_bytes: [115, 101, 99, 114, 101, 116],
    },
  ],
});
assert(artifactsHtml.includes("artifact-1"), "artifact renderer must include artifact id");
assert(artifactsHtml.includes("aaaaaaaaaaaa"), "artifact renderer must include checksum prefix");
assert(artifactsHtml.includes("Open"), "artifact renderer must include retrieval action");
assert(!artifactsHtml.includes("/etc/secret.conf"), "artifact renderer must not show destination path");
assert(!artifactsHtml.includes("secret"), "artifact renderer must not show rendered body");
const artifactBody = renderArtifactBody({
  job_id: "job-1",
  artifact_id: "artifact-1",
  task_id: "task-1",
  agent_id: "agent-1",
  retention_class: "rendered_template",
  checksum_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  size_bytes: 5,
  content_bytes: [104, 101, 108, 108, 111],
});
assert(artifactBody.includes("Artifact: artifact-1"), "artifact body renderer must identify artifact");
assert(artifactBody.includes("Preview:"), "artifact body renderer must show printable preview");
assert(artifactBody.includes("hello"), "artifact body renderer must render printable content");
assert(!artifactBody.includes("/etc/"), "artifact body renderer must not include local paths");
const targetTableHtml = renderJobTargetTable({
  target_agents: [
    {
      agent_id: "agent-1",
      status: "online",
      connected: true,
      revoked: false,
      assignment_status: "started",
      task_id: "task-1",
      last_error: "",
    },
    {
      agent_id: "agent-2",
      status: "disabled",
      connected: false,
      revoked: true,
      assignment_status: "rejected",
      task_id: "task-2",
      last_error: "revoked",
    },
  ],
});
assert(targetTableHtml.includes("agent-1"), "target table must include agent ids");
assert(targetTableHtml.includes("started"), "target table must include assignment status");
assert(targetTableHtml.includes("revoked"), "target table must include revoked target state");
const approvalsHtml = renderApprovals([
  {
    id: "approval-1",
    job_id: "job-approval-1",
    status: "pending",
    reason: "high-risk command requires manual approval",
    expires_at_ms: 2000,
  },
], [
  {
    id: "job-approval-1",
    target_count: 1,
    target_agents: [{ agent_id: "agent-1", assignment_status: "queued" }],
  },
]);
assert(approvalsHtml.includes("job-approval-1"), "approval queue must include job id");
assert(approvalsHtml.includes("agent-1:queued"), "approval queue must include target snapshot summary");
assert(approvalsHtml.includes("Approve"), "approval queue must expose approve action");
assert(approvalsHtml.includes("Reject"), "approval queue must expose reject action");
const remediationHtml = renderRemediations([
  {
    id: "rem-1",
    policy_id: "nginx-running",
    policy_name: "nginx-running",
    agent_id: "agent-1",
    runbook_ref: "runbooks/remediate.yml",
    status: "proposed",
    risk_summary: "drifted policy requires approved remediation",
    job_id: null,
    updated_at_ms: 2000,
    runbook_document: "kind: Runbook\n# secret-value-should-not-leak",
    command_output: "secret-value-should-not-leak",
  },
], "rem-1");
assert(remediationHtml.includes("rem-1") || remediationHtml.includes("nginx-running"), "remediation list must render metadata");
assert(remediationHtml.includes("selected"), "remediation list must mark selected row");
assert(remediationHtml.includes("runbooks/remediate.yml"), "remediation list must render runbook ref");
assert(!remediationHtml.includes("kind: Runbook"), "remediation list must not render raw runbook body");
assert(!remediationHtml.includes("secret-value-should-not-leak"), "remediation list must not render secret marker");
const remediationResult = renderRemediationActionResult({
  remediation: {
    id: "rem-1",
    policy_id: "nginx-running",
    agent_id: "agent-1",
    runbook_ref: "runbooks/remediate.yml",
    status: "job_created",
    job_id: "job-rem-1",
    runbook_document: "kind: Runbook\n# secret-value-should-not-leak",
  },
  approval: { id: "approval-1", status: "approved" },
  assignment_count: 1,
});
assert(remediationResult.includes("remediation_id=rem-1"), "remediation action result must include id");
assert(remediationResult.includes("approval_id=approval-1"), "remediation action result must include approval id");
assert(!remediationResult.includes("kind: Runbook"), "remediation action result must not render raw runbook body");
assert(!remediationResult.includes("secret-value-should-not-leak"), "remediation action result must not render secret marker");
const policiesHtml = renderPolicies([{ id: "nginx-running", name: "nginx-running", version: 2 }], "nginx-running");
assert(policiesHtml.includes("selected"), "policy list must mark selected policy");
assert(policiesHtml.includes("v2"), "policy list must show policy version");
const agentPoliciesHtml = renderAgentPolicies(
  [{ policy_id: "nginx-running", assigned_at_ms: 2000 }],
  "agent-1",
);
assert(agentPoliciesHtml.includes("assigned"), "agent policy list must show assignment state");
const logsHtml = renderAgentLogs({
  items: [{ collected_at_ms: 2000, line: "level=warn event=agent_log_uploaded" }],
});
assert(logsHtml.includes("level=warn"), "agent log viewer must render log chunks");
const tokenRequest = buildEnrollmentTokenRequest({
  labels: "role=web",
  maxUses: "2",
  expiresInSeconds: "900",
});
assert(tokenRequest.labels === "role=web", "token request must keep label scope");
assert(tokenRequest.max_uses === 2, "token request must parse max uses");
assert(tokenRequest.expires_in_seconds === 900, "token request must parse expiry");
let invalidTokenScopeFailed = false;
try {
  buildEnrollmentTokenRequest({ labels: "", maxUses: "0", expiresInSeconds: "900" });
} catch {
  invalidTokenScopeFailed = true;
}
assert(invalidTokenScopeFailed, "token request must reject invalid max uses");
const tokenSecretText = renderCreatedEnrollmentToken(
  { id: "et-1", token: "enroll-secret", expires_in_seconds: 900 },
  "https://fleet.example.com",
  "prod-web-01",
);
assert(tokenSecretText.includes("enroll-secret"), "one-time token renderer must show created token");
assert(tokenSecretText.includes("sponzey agent init"), "one-time token renderer must include init command");
const tokenListHtml = renderEnrollmentTokens([
  {
    id: "et-1",
    default_labels: "role=web",
    max_uses: 2,
    used_count: 1,
    remaining_uses: 1,
    revoked: false,
    expires_at_epoch: 1900000000,
  },
]);
assert(tokenListHtml.includes("role=web"), "token summary must include label scope");
assert(!tokenListHtml.includes("enroll-secret"), "token summary must never include the raw token");
assert(
  formatApiError("/api/agents", 401).includes("admin token"),
  "forbidden renderer must guide operator toward authorization",
);
assert(
  formatApiError("/api/agents/agent-1/revoke-key", 403).includes("permissions"),
  "forbidden renderer must tell operator to check permissions",
);
assert(formatApiError("/api/jobs/job-missing", 404).includes("not found"), "404 errors must be explicit");
assert(formatApiError("/api/jobs/command", 409).includes("conflicted"), "409 errors must be explicit");
assert(
  JSON.stringify(parseCommandArgs(" -a  -b ")) === JSON.stringify(["-a", "-b"]),
  "command argument parser must split whitespace",
);
const jobRequest = buildCommandJobRequest({
  agentId: "agent-1",
  program: "uptime",
  args: "-a",
  confirmed: true,
});
assert(jobRequest.confirmed_high_risk, "job request must include high-risk confirmation");
assert(jobRequest.target_agent_ids.includes("agent-1"), "job request must target selected agent");
assert(jobRequest.expires_in_seconds === 300, "command jobs must leave enough time for approval");
const selectorJobRequest = buildCommandJobRequest({
  agentId: "",
  selector: " role=web ",
  program: "uptime",
  args: "",
  confirmed: true,
});
assert(selectorJobRequest.selector === "role=web", "selector command requests must trim selector input");
assert(selectorJobRequest.target_agent_ids.length === 0, "selector command requests must not duplicate target ids");
const runbookRequest = buildRunbookJobRequest({
  agentId: "agent-1",
  document: "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nsteps: []",
  confirmed: true,
});
assert(runbookRequest.runbook_document.includes("Runbook"), "runbook request must include document");
assert(runbookRequest.target_agent_ids.includes("agent-1"), "runbook request must target selected agent");
const selectorRunbookRequest = buildRunbookJobRequest({
  agentId: "",
  selector: " label:role=web ",
  document: "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nsteps: []",
  confirmed: true,
});
assert(selectorRunbookRequest.selector === "label:role=web", "selector runbook requests must trim selector input");
assert(selectorRunbookRequest.target_agent_ids.length === 0, "selector runbook requests must not duplicate target ids");
const selectorPreviewRequest = buildSelectorPreviewRequest({ selector: " role=web " });
assert(selectorPreviewRequest.selector === "role=web", "selector preview requests must trim selector input");
const selectorPreviewHtml = renderSelectorPreview({
  matched_count: 2,
  selected_count: 1,
  disabled_count: 1,
  offline_count: 0,
  warnings: [{ code: "disabled_agents_excluded", message: "1 disabled agent was excluded" }],
  agents: [
    {
      agent_id: "agent-1",
      name: "web-01",
      status: "online",
      labels: [{ key: "role", value: "web" }],
      selected_for_dispatch: true,
    },
    {
      agent_id: "agent-2",
      name: "web-02",
      status: "disabled",
      labels: [{ key: "role", value: "web" }],
      selected_for_dispatch: false,
    },
  ],
});
assert(selectorPreviewHtml.includes("Matched"), "selector preview must render matched count");
assert(selectorPreviewHtml.includes("Selected"), "selector preview must render selected count");
assert(selectorPreviewHtml.includes("Disabled"), "selector preview must render disabled count");
assert(selectorPreviewHtml.includes("1 disabled agent was excluded"), "selector preview must render warnings");
assert(selectorPreviewHtml.includes("role=web"), "selector preview must render agent labels from the API response");
assert(selectorPreviewHtml.includes("selected"), "selector preview must show dispatch selection from the API response");
assert(selectorPreviewHtml.includes("excluded"), "selector preview must show excluded matches from the API response");
assert(selectorPreviewSelectedCount({ selected_count: 2 }) === 2, "selector preview helper must read selected count");
assert(buildPolicySaveRequest({ source: "kind: Policy" }).source === "kind: Policy", "policy save request must trim source");
assert(
  buildPolicyAssignmentRequest({ policyId: "policy-1", agentId: "agent-1" }).agent_id === "agent-1",
  "policy assignment request must target selected agent",
);
assert(
  buildPolicyScheduleRequest({ policyId: "policy-1", agentId: "agent-1", intervalSeconds: "300" })
    .interval_seconds === 300,
  "policy schedule request must parse interval",
);
let confirmationFailed = false;
try {
  buildCommandJobRequest({ agentId: "agent-1", program: "uptime", args: "", confirmed: false });
} catch {
  confirmationFailed = true;
}
assert(confirmationFailed, "run command form must require high-risk confirmation");
const output = renderJobOutput([
  { agent_id: "agent-1", stream: "stdout", sequence: 0, data: "ok\n" },
  { agent_id: "agent-1", stream: "stderr", sequence: 1, data: "warn\n" },
], { jobId: "job-1", job: { status: "running", dispatch_state: "delivered" } });
assert(output.includes("Job: job-1"), "job output renderer must show the selected job id");
assert(output.includes("Dispatch: delivered"), "job output renderer must show dispatch state");
assert(output.includes("Output chunks: 2"), "job output renderer must show chunk count");
assert(output.includes("[agent-1 stdout #0]"), "job output renderer must prefix stdout with sequence");
assert(output.includes("ok"), "job output renderer must include stdout body");
assert(output.includes("[agent-1 stderr #1]"), "job output renderer must prefix stderr with sequence");
assert(output.includes("warn"), "job output renderer must include stderr body");
const multilineOutput = renderJobOutput([
  { agent_id: "agent-1", stream: "stdout", sequence: 0, data: "line 1\nline 2\nline 3\n" },
  { agent_id: "agent-1", stream: "stderr", sequence: 1, data: "real 0m0.001s\nuser 0m0.000s\nsys 0m0.001s\n" },
], { jobId: "job-time-1", job: { status: "success", dispatch_state: "completed" } });
assert(multilineOutput.includes("line 1\nline 2\nline 3"), "multi-line stdout chunks must render all lines");
assert(multilineOutput.includes("real 0m0.001s"), "multi-line stderr chunks must render timing output");
assert(multilineOutput.includes("[agent-1 stderr #1]"), "multi-line stderr chunks must keep stream prefix");
const waitingOutput = renderJobOutputWaiting({ jobId: "job-1", attempt: 2, maxAttempts: 10 });
assert(waitingOutput.includes("Job created. Checking dispatch state."), "created job output must use explicit created wording");
assert(waitingOutput.includes("Polling job output (2/10)."), "waiting output must show polling progress");
assert(!waitingOutput.includes("No job output"), "waiting output must not look like a completed empty result");
const queuedOutput = renderJobOutputStatus(
  {
    id: "job-queued-1",
    status: "queued",
    dispatch_state: "queued",
    assignment_summary: {
      queued: 1,
      dispatched: 0,
      accepted: 0,
      started: 0,
      succeeded: 0,
      failed: 0,
      rejected: 0,
      canceled: 0,
      expired: 0,
      skipped: 0,
      unknown: 0,
    },
    target_agents: [
      {
        agent_id: "agent-1",
        status: "offline",
        connected: false,
        revoked: false,
        task_id: "task-1",
        assignment_status: "queued",
        last_error: "",
      },
    ],
  },
  { jobId: "job-queued-1", attempt: 1, maxAttempts: 3 },
);
assert(queuedOutput.includes("Queued until agent reconnects."), "queued output must explain offline queueing");
assert(!queuedOutput.includes("No job output"), "queued output must not render completed empty wording");
const runningOutput = renderJobOutputStatus(
  { id: "job-running-1", status: "running", dispatch_state: "delivered", target_agents: [] },
  { jobId: "job-running-1" },
);
assert(runningOutput.includes("Running on agent. Waiting for output."), "running output must explain delivery");
const emptyOutput = renderJobOutputEmpty({ jobId: "job-1", maxAttempts: 10 });
assert(emptyOutput.includes("Completed with no output."), "completed empty output must be explicit");
assert(isTerminalJob({ status: "success", dispatch_state: "completed" }), "completed jobs must be terminal");
assert(
  !isTerminalJob({ status: "queued", dispatch_state: "queued" }),
  "queued jobs must not be terminal",
);
assert(
  jobStatusMessage({ status: "expired", dispatch_state: "expired", last_error: "deadline exceeded" }).includes(
    "Create a new job",
  ),
  "expired output must include next action",
);
assert(
  jobStatusMessage({ status: "canceled", dispatch_state: "canceled", last_error: "operator requested cancel" }).includes(
    "Canceled before completion",
  ),
  "canceled output must use cancel wording",
);
assert(
  jobStatusMessage({ status: "failed", dispatch_state: "rejected", last_error: "confirmation missing" }).includes(
    "Rejected by controller policy",
  ),
  "rejected output must include policy wording",
);
assert(
  jobStatusMessage({ status: "pending_approval", dispatch_state: "created" }).includes("Approval required"),
  "approval-gated jobs must not look queued",
);
const approvalPendingOutput = renderJobOutputStatus(
  { id: "job-approval-1", status: "pending_approval", dispatch_state: "created", target_agents: [] },
  { jobId: "job-approval-1", attempt: 1, maxAttempts: 3 },
);
assert(isApprovalPendingJob({ status: "pending_approval" }), "approval pending jobs must be detected");
assert(
  approvalPendingOutput.includes("Open Approvals and approve or reject this job"),
  "approval pending output must point to approval action",
);
assert(
  !approvalPendingOutput.includes("Polling job output"),
  "approval pending output must not claim output polling is active",
);
assert(
  approvalDecisionJobToPoll("approve", { job_id: "job-approval-1" }) === "job-approval-1",
  "approving an approval must resume polling for the approved job",
);
assert(
  approvalDecisionJobToPoll("reject", { job_id: "job-approval-1" }) === "",
  "rejecting an approval must not resume output polling",
);
const completedNoOutput = renderJobOutputAfterPolling({
  jobId: "job-complete-1",
  job: { id: "job-complete-1", status: "success", dispatch_state: "completed", target_agents: [] },
  chunks: [],
});
assert(
  completedNoOutput.includes("Completed with no output."),
  "completed jobs without chunks must not look pending",
);
const pendingNoOutput = renderJobOutputAfterPolling({
  jobId: "job-pending-1",
  job: { id: "job-pending-1", status: "queued", dispatch_state: "queued", target_agents: [] },
  chunks: [],
});
assert(
  pendingNoOutput.includes("Queued until agent reconnects."),
  "pending jobs without chunks must stay pending",
);
assert(!pendingNoOutput.includes("Completed with no output."), "pending jobs must not show completed no-output text");
const singlePoll = await pollJobOutputOnce(
  {
    getJob: async () => ({
      id: "job-poll-1",
      status: "running",
      dispatch_state: "delivered",
      target_agents: [],
    }),
    getJobOutput: async () => [],
  },
  "job-poll-1",
);
assert(singlePoll.text.includes("Running on agent. Waiting for output."), "single poll must combine job detail and output");
assert(!singlePoll.terminal, "single poll must keep running jobs non-terminal");

console.log("web-admin smoke tests passed");

if (existsSync(join(root, "dist", "index.html"))) {
  console.log("web-admin dist is present");
}
