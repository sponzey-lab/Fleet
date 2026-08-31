export const API_SCHEMA_VERSION = "mvp-1";

function encodePathValue(value) {
  return encodeURIComponent(String(value ?? ""));
}

function pageQuery({ limit, before, after } = {}) {
  const params = new URLSearchParams();
  if (limit !== undefined && limit !== null && limit !== "") {
    params.set("limit", String(limit));
  }
  if (before) {
    params.set("before", String(before));
  }
  if (after) {
    params.set("after", String(after));
  }
  const query = params.toString();
  return query ? `?${query}` : "";
}

function remediationQuery({ agentId, policyId, limit } = {}) {
  const params = new URLSearchParams();
  if (agentId) {
    params.set("agent_id", String(agentId));
  }
  if (policyId) {
    params.set("policy_id", String(policyId));
  }
  if (limit !== undefined && limit !== null && limit !== "") {
    params.set("limit", String(limit));
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
    getAgentCertificateLifecycleStatus(agentId) {
      return request(`/api/agents/${encodePathValue(agentId)}/certificate-lifecycle/status`);
    },
    requestAgentCertificateIssuance(agentId, body = {}) {
      return request(`/api/agents/${encodePathValue(agentId)}/certificate-lifecycle/request-issuance`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    getControllerSigningRotationStatus() {
      return request("/api/controller/signing-rotation/status");
    },
    stageControllerSigningTrustBundle(body = {}) {
      return request("/api/controller/signing-rotation/rollout-trust-bundle/staged", {
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
    listCatalogSources(page = {}) { return request(`/api/catalog/sources${pageQuery(page)}`); },
    registerCatalogSource(body = {}) { return request("/api/catalog/sources", { method: "POST", body: JSON.stringify(body) }); },
    startCatalogSync(sourceId, body = {}) { return request(`/api/catalog/sources/${encodePathValue(sourceId)}/sync`, { method: "POST", body: JSON.stringify(body) }); },
    activateCatalogRevision(sourceId, body = {}) { return request(`/api/catalog/sources/${encodePathValue(sourceId)}/activate`, { method: "POST", body: JSON.stringify(body) }); },
    listCatalogRevisions(sourceId, page = {}) { return request(`/api/catalog/sources/${encodePathValue(sourceId)}/revisions${pageQuery(page)}`); },
    listCatalogDocuments(sourceId, commit, page = {}) { return request(`/api/catalog/sources/${encodePathValue(sourceId)}/revisions/${encodePathValue(commit)}/documents${pageQuery(page)}`); },
    getCatalogDocumentDetail(sourceId, commit, path) { return request(`/api/catalog/sources/${encodePathValue(sourceId)}/revisions/${encodePathValue(commit)}/document?path=${encodeURIComponent(path)}`); },
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
    listRemediations(filters = {}) {
      return request(`/api/remediations${remediationQuery(filters)}`);
    },
    getRemediation(remediationId) {
      return request(`/api/remediations/${encodePathValue(remediationId)}`);
    },
    createRemediationApprovalRequest(remediationId, body = {}) {
      return request(`/api/remediations/${encodePathValue(remediationId)}/approval-request`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    approveRemediationJob(remediationId, body = {}) {
      return request(`/api/remediations/${encodePathValue(remediationId)}/approve`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    markRemediationRunning(remediationId, body = {}) {
      return request(`/api/remediations/${encodePathValue(remediationId)}/running`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    recordRemediationResult(remediationId, body = {}) {
      return request(`/api/remediations/${encodePathValue(remediationId)}/result`, {
        method: "POST",
        body: JSON.stringify(body),
      });
    },
    verifyRemediation(remediationId, body = {}) {
      return request(`/api/remediations/${encodePathValue(remediationId)}/verify`, {
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
    getJobArtifact(jobId, artifactId) {
      return request(`/api/jobs/${encodePathValue(jobId)}/artifacts/${encodePathValue(artifactId)}`);
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
    exportAudit({ category = "", limit = 50, before = "" } = {}) {
      const query = new URLSearchParams();
      if (category) query.set("category", category);
      query.set("limit", String(limit));
      if (before) query.set("before", before);
      return request(`/api/audit/export?${query.toString()}`);
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
