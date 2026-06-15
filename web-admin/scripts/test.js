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
assert(index.includes("id=\"revoke-agent-key\""), "index must expose agent key revocation");
assert(index.includes(">Revoke Agent</button>"), "index must label agent revocation by agent");
assert(index.includes("id=\"refresh-telemetry\""), "index must expose selected agent telemetry refresh");
assert(index.includes("id=\"facts-chart\""), "index must expose facts trend charts");
assert(index.includes("id=\"facts-panel\""), "index must expose the facts surface");
assert(index.includes("id=\"metrics-chart\""), "index must expose metrics trend charts");
assert(index.includes("id=\"metrics-panel\""), "index must expose the metrics surface");
assert(index.includes("id=\"drift-panel\""), "index must expose the drift surface");
assert(index.includes("id=\"audit-list\""), "index must expose the audit surface");
assert(index.includes("id=\"run-command-form\""), "index must expose command execution");
assert(index.includes('value="uptime"'), "run command form must default to a safe probe command");
assert(index.includes("id=\"job-output\""), "index must expose job output");
assert(index.includes("Run a command or select a job"), "job output placeholder must not look like a completed empty result");
assert(index.includes("id=\"jobs-list\""), "index must expose job history");
assert(index.includes("id=\"enrollment-token-form\""), "index must expose enrollment token creation");
assert(index.includes("id=\"enrollment-tokens-list\""), "index must expose enrollment token summaries");
assert(index.includes("id=\"created-enrollment-token\""), "index must expose one-time token output");
assert(index.includes("/admin/app.js"), "index must load the dependency-free app script from the admin base path");
assert(index.includes("/admin/styles.css"), "index must load styles from the admin base path");
assert(index.includes('method="post"'), "admin auth form must not leak tokens through a query string fallback");
assert(!index.includes("localStorage"), "UI must not store tokens in localStorage");
assert(!index.includes("runtime config"), "UI must not expose runtime config mutation");
assert(styles.includes(".layout"), "styles must include the admin layout");
assert(styles.includes(".snapshot-time"), "styles must include snapshot time metadata");
assert(styles.includes(".chart-grid"), "styles must include telemetry chart grid");
assert(styles.includes(".sparkline"), "styles must include telemetry sparkline styling");
assert(app.includes("./api-client.js"), "app must use the shared API client");
assert(app.includes("handleAgentsListClick"), "app must use delegated agent selection handling");
assert(app.includes("TELEMETRY_PAGE_LIMIT"), "app must fetch bounded telemetry history pages");
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
  "getLatestDrift",
  "listDrift",
  "revokeAgentKey",
  "listJobs",
  "getJob",
  "getJobOutput",
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
  renderSnapshot,
  renderFactsInventory,
  renderDrift,
  renderAudit,
  formatApiError,
  parseCommandArgs,
  buildCommandJobRequest,
  renderJobOutput,
  renderJobOutputWaiting,
  renderJobOutputEmpty,
  renderJobOutputStatus,
  renderJobOutputAfterPolling,
  jobStatusMessage,
  isTerminalJob,
  pollJobOutputOnce,
  renderJobs,
  renderEnrollmentTokens,
  renderCreatedEnrollmentToken,
  buildEnrollmentTokenRequest,
  formatUnixMillis,
  renderTelemetryCharts,
  recentSnapshots,
  snapshotTimeMs,
  formatKilobytes,
  memoryUsedPercent,
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
await client.getLatestDrift("agent/1");
await client.listDrift("agent/1", { before: "2:9" });
await client.revokeAgentKey("agent/1");
await client.listJobs();
await client.getJob("job/1");
await client.getJobOutput("job/1");
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
  findCall("/api/agents/agent%2F1/drift/latest"),
  "client must encode latest drift paths",
);
assert(
  findCall("/api/agents/agent%2F1/drift?before=2%3A9"),
  "client must encode paged drift query",
);
assert(
  findCall("/api/agents/agent%2F1/revoke-key", "POST"),
  "client must encode agent ids in key revocation paths",
);
assert(
  findCall("/api/agents/agent%2F1/revoke-key", "POST").options.method === "POST",
  "client must POST agent key revocation",
);
assert(findCall("/api/jobs"), "client must call jobs list endpoint");
assert(findCall("/api/jobs/job%2F1"), "client must call job detail endpoint");
assert(findCall("/api/jobs/job%2F1/output"), "client must call job output endpoint");
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

const driftHtml = renderDrift({
  policy_name: "nginx-running",
  status: "drifted",
  expected: "service nginx running",
  actual: "service nginx stopped",
  checked_at_ms: 1000,
  agent_system_time_ms: 2000,
});
assert(driftHtml.includes("Agent time 1970-01-01T00:00:02.000Z"), "drift renderer must include agent time");
assert(driftHtml.includes("Expected"), "drift renderer must include expected section");
assert(driftHtml.includes("service nginx stopped"), "drift renderer must include actual detail");

const auditHtml = renderAudit([
  { category: "security", action: "invalid_signature", actor: "system", target: "agent-1", value_kind: "redacted", value: "redacted" },
]);
assert(auditHtml.includes("invalid_signature"), "audit renderer must include event action");
const jobsHtml = renderJobs([
  {
    id: "job-1",
    status: "running",
    dispatch_state: "delivered",
    risk: "high",
    command_program: "uptime",
    command_args: ["-a"],
    target_count: 1,
    target_agents: [{ agent_id: "agent-1", status: "online", connected: true, revoked: false }],
  },
]);
assert(jobsHtml.includes("job-1"), "job renderer must include job id");
assert(jobsHtml.includes("uptime -a"), "job renderer must include command summary");
assert(jobsHtml.includes("delivered"), "job renderer must include controller dispatch state");
assert(jobsHtml.includes("1 target(s), 1 connected"), "job renderer must use API target connection state");
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
const waitingOutput = renderJobOutputWaiting({ jobId: "job-1", attempt: 2, maxAttempts: 10 });
assert(waitingOutput.includes("Job created. Checking dispatch state."), "created job output must use explicit created wording");
assert(waitingOutput.includes("Polling job output (2/10)."), "waiting output must show polling progress");
assert(!waitingOutput.includes("No job output"), "waiting output must not look like a completed empty result");
const queuedOutput = renderJobOutputStatus(
  {
    id: "job-queued-1",
    status: "queued",
    dispatch_state: "queued",
    target_agents: [{ agent_id: "agent-1", status: "offline", connected: false, revoked: false }],
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
  jobStatusMessage({ status: "canceled", dispatch_state: "rejected", last_error: "confirmation missing" }).includes(
    "Review confirmation",
  ),
  "rejected output must include next action",
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
