export const API_SCHEMA_VERSION = "mvp-1";

function encodePathValue(value) {
  return encodeURIComponent(String(value ?? ""));
}

function pageQuery({ limit, before } = {}) {
  const params = new URLSearchParams();
  if (limit !== undefined && limit !== null && limit !== "") {
    params.set("limit", String(limit));
  }
  if (before) {
    params.set("before", String(before));
  }
  const query = params.toString();
  return query ? `?${query}` : "";
}

function defaultFormatApiError(path, status) {
  if (status === 401 || status === 403) {
    return "Controller rejected this request. Check the admin token and permissions.";
  }
  return `${path} returned ${status}`;
}

export function normalizeAdminToken(value) {
  let token = String(value ?? "").trim();
  if (token.toLowerCase().startsWith("bearer ")) {
    token = token.slice("bearer ".length).trim();
  }
  return token;
}

export function createApiClient({ fetchImpl = globalThis.fetch, tokenProvider = () => "", formatError = defaultFormatApiError } = {}) {
  if (typeof fetchImpl !== "function") {
    throw new Error("fetch implementation is required.");
  }

  async function request(path, options = {}) {
    const token = normalizeAdminToken(tokenProvider());
    const response = await fetchImpl(path, {
      ...options,
      headers: {
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        Accept: "application/json",
        ...(options.body ? { "Content-Type": "application/json" } : {}),
        ...(options.headers || {}),
      },
    });
    if (response.status === 404) {
      return null;
    }
    if (!response.ok) {
      throw new Error(formatError(path, response.status));
    }
    if (response.status === 204) {
      return null;
    }
    return response.json();
  }

  return {
    listAgents() {
      return request("/api/agents");
    },
    getLatestFacts(agentId) {
      return request(`/api/agents/${encodePathValue(agentId)}/facts/latest`);
    },
    listFacts(agentId, page = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/facts${pageQuery(page)}`);
    },
    getLatestMetrics(agentId) {
      return request(`/api/agents/${encodePathValue(agentId)}/metrics/latest`);
    },
    listMetrics(agentId, page = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/metrics${pageQuery(page)}`);
    },
    listAgentLogs(agentId, page = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/logs${pageQuery(page)}`);
    },
    getLatestDrift(agentId) {
      return request(`/api/agents/${encodePathValue(agentId)}/drift/latest`);
    },
    listDrift(agentId, page = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/drift${pageQuery(page)}`);
    },
    revokeAgentKey(agentId, body = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/revoke-key`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    listJobs() {
      return request("/api/jobs");
    },
    listApprovals(status = "") {
      const query = status ? `?status=${encodeURIComponent(status)}` : "";
      return request(`/api/approvals${query}`);
    },
    listPolicies() {
      return request("/api/policies");
    },
    savePolicy(body = {}) {
      return request("/api/policies", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    assignPolicy(policyId, body = {}) {
      return request(`/api/policies/${encodePathValue(policyId)}/assignments`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    schedulePolicyDrift(policyId, body = {}) {
      return request(`/api/policies/${encodePathValue(policyId)}/schedules`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    listAgentPolicies(agentId) {
      return request(`/api/agents/${encodePathValue(agentId)}/policies`);
    },
    listDueScheduledDrift() {
      return request("/api/drift/scheduled");
    },
    approveApproval(approvalId, body = {}) {
      return request(`/api/approvals/${encodePathValue(approvalId)}/approve`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    rejectApproval(approvalId, body = {}) {
      return request(`/api/approvals/${encodePathValue(approvalId)}/reject`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    expireApprovals(body = {}) {
      return request("/api/approvals/expire", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    previewSelector(body) {
      return request("/api/selectors/preview", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    getJob(jobId) {
      return request(`/api/jobs/${encodePathValue(jobId)}`);
    },
    getJobOutput(jobId) {
      return request(`/api/jobs/${encodePathValue(jobId)}/output`);
    },
    cancelJob(jobId, body = {}) {
      return request(`/api/jobs/${encodePathValue(jobId)}/cancel`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    listAudit() {
      return request("/api/audit");
    },
    listEnrollmentTokens() {
      return request("/api/enrollment-tokens");
    },
    createEnrollmentToken(body) {
      return request("/api/enrollment-tokens", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    revokeEnrollmentToken(id) {
      return request(`/api/enrollment-tokens/${encodePathValue(id)}`, {
        method: "DELETE",
      });
    },
    createCommandJob(body) {
      return request("/api/jobs/command", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    createDriftCheckJob(body) {
      return request("/api/jobs/drift-check", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    createRunbookJob(body) {
      return request("/api/jobs/runbook", {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
  };
}
