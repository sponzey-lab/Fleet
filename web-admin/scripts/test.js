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

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const index = readFileSync(indexPath, "utf8");
const styles = readFileSync(stylesPath, "utf8");
const app = readFileSync(appPath, "utf8");
const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
const tsconfig = JSON.parse(readFileSync(tsconfigPath, "utf8"));

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
await client.listMetrics("agent/1", { limit: 10 });
await client.listDrift("agent/1", { before: "2:9" });
await client.revokeAgentKey("agent/1");
await client.listEnrollmentTokens();
await client.createEnrollmentToken({ labels: "role=web", max_uses: 1, expires_in_seconds: 60 });
await client.revokeEnrollmentToken("et/1");
await client.createCommandJob({ job_id: "job-1" });
await client.createRunbookJob({ job_id: "job-runbook-1" });
assert(calls[0].path === "/api/agents", "client must call agents endpoint");
assert(
  calls[1].path === "/api/agents/agent%2F1/facts/latest",
  "client must encode agent ids in paths",
);
assert(
  calls[2].path === "/api/agents/agent%2F1/facts?limit=25&before=2%3A10",
  "client must encode paged facts query",
);
assert(
  calls[3].path === "/api/agents/agent%2F1/metrics?limit=10",
  "client must encode paged metrics query",
);
assert(
  calls[4].path === "/api/agents/agent%2F1/drift?before=2%3A9",
  "client must encode paged drift query",
);
assert(
  calls[5].path === "/api/agents/agent%2F1/revoke-key",
  "client must encode agent ids in key revocation paths",
);
assert(calls[5].options.method === "POST", "client must POST agent key revocation");
assert(calls[6].path === "/api/enrollment-tokens", "client must call token list endpoint");
assert(calls[7].path === "/api/enrollment-tokens", "client must call token create endpoint");
assert(calls[7].options.method === "POST", "client must POST token creation");
assert(calls[8].path === "/api/enrollment-tokens/et%2F1", "client must encode token ids in paths");
assert(calls[8].options.method === "DELETE", "client must DELETE token revocation");
assert(calls[9].path === "/api/jobs/command", "client must call command job endpoint");
assert(calls[9].options.method === "POST", "client must POST command jobs");
assert(calls[10].path === "/api/jobs/runbook", "client must call runbook job endpoint");
assert(calls[7].options.method === "POST", "client must POST runbook jobs");
assert(
  calls[6].options.headers.Authorization === "Bearer admin-token",
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
    hostname: "web-01.local",
    os: "linux",
    arch: "x86_64",
    last_seen_age_seconds: 5,
  },
]);
assert(agentsHtml.includes("web-01"), "agents renderer must include agent name");
assert(agentsHtml.includes("role=web"), "agents renderer must include labels");
assert(agentsHtml.includes("linux/x86_64"), "agents renderer must include platform summary");
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
    disk: { root_total_kb: 100 * 1024 * 1024, root_filesystem: "/dev/root" },
    network: { interfaces: ["lo", "eth0"] },
  },
});
assert(factsInventoryHtml.includes("Memory total"), "facts inventory must show static memory capacity");
assert(factsInventoryHtml.includes("16.0 GiB"), "facts inventory must format memory capacity");
assert(factsInventoryHtml.includes("Memory modules"), "facts inventory must show memory module count");
assert(factsInventoryHtml.includes("Root disk total"), "facts inventory must show static disk capacity");
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
  { id: "job-1", status: "success", risk: "high", command_program: "uptime", command_args: ["-a"], target_count: 1 },
]);
assert(jobsHtml.includes("job-1"), "job renderer must include job id");
assert(jobsHtml.includes("uptime -a"), "job renderer must include command summary");
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
  { agent_id: "agent-1", stream: "stdout", data: "ok\n" },
  { agent_id: "agent-1", stream: "stderr", data: "warn\n" },
]);
assert(output.includes("[agent-1 stdout] ok"), "job output renderer must prefix stdout");
assert(output.includes("[agent-1 stderr] warn"), "job output renderer must prefix stderr");

console.log("web-admin smoke tests passed");

if (existsSync(join(root, "dist", "index.html"))) {
  console.log("web-admin dist is present");
}
