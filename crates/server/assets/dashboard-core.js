const COLLECTION = "collection";
const RECORD = "record";
const PROFILE_RETAINED_KEYS = ["display_name", "timezone"];

export const PROFILE_RESERVED_KEYS = Object.freeze(
  new Set(["version", "updated_at", "updated_by", "recent_requests"]),
);

export const VERSION_SCOPE = Object.freeze({
  "personal.action.create": COLLECTION,
  "personal.action.update": RECORD,
  "personal.action.complete": RECORD,
  "personal.action.archive": RECORD,
  "personal.action.delete": RECORD,
  "personal.action.queue": COLLECTION,
  "personal.event.create": COLLECTION,
  "personal.event.update": RECORD,
  "personal.event.archive": RECORD,
  "personal.event.delete": RECORD,
  "personal.profile.update": COLLECTION,
  "goal.goal.create": COLLECTION,
  "goal.goal.update": COLLECTION,
  "goal.milestone.create": COLLECTION,
  "goal.milestone.update": COLLECTION,
  "goal.checkpoint.create": COLLECTION,
  "goal.checkpoint.update": COLLECTION,
  "goal.action_link.set": COLLECTION,
  "goal.action_link.remove": COLLECTION,
  "goal.migrate": COLLECTION,
  "goal.review.set": COLLECTION,
  "goal.progress.record": COLLECTION,
  "health.body.update": RECORD,
  "health.water.update": RECORD,
  "health.sleep.update": RECORD,
  "health.body.set_profile": COLLECTION,
  "health.body.log": COLLECTION,
  "health.water.set_goal": COLLECTION,
  "health.water.log": COLLECTION,
  "health.diet.set_week": COLLECTION,
  "health.diet.set_meal": COLLECTION,
  "health.sleep.set_goal": COLLECTION,
  "health.sleep.log": COLLECTION,
  "life.clothes.update": RECORD,
  "life.clothes.wear": RECORD,
  "life.clothes.mark_dirty": RECORD,
  "life.clothes.archive": RECORD,
  "life.clothes.add": COLLECTION,
  "life.clothes.wash": COLLECTION,
});

export function versionFor(command, { collectionVersion, recordVersion } = {}) {
  const scope = VERSION_SCOPE[command];
  if (!scope) throw new Error(`no version scope for ${command}`);
  const version = scope === COLLECTION ? collectionVersion : recordVersion;
  if (!Number.isInteger(version) || version < 0) {
    throw new Error(`missing ${scope} version for ${command}`);
  }
  return version;
}

export function formatMinutes(minutes) {
  const value = Number(minutes);
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value < 60) return `${Math.round(value)}m`;
  const hours = Math.floor(value / 60);
  const rest = Math.round(value % 60);
  return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

export function actionBudget(action) {
  const rawDuration = action && action.duration_minutes;
  const duration = Number(rawDuration ?? 30);
  const safeDuration = Number.isFinite(duration) && duration > 0 ? duration : 0;
  const rawBuffer = action && action.buffer_minutes;
  const buffer = Number(rawBuffer ?? (safeDuration > 0 ? safeDuration : 30));
  const safeBuffer = Number.isFinite(buffer) && buffer >= 0 ? buffer : 0;
  const total = safeDuration + safeBuffer;
  return {
    duration: safeDuration,
    buffer: safeBuffer,
    total,
    durationPercent: total > 0 ? (safeDuration / total) * 100 : 100,
    bufferPercent: total > 0 ? (safeBuffer / total) * 100 : 0
  };
}

export function buildEnvelope(requestId, expectedVersion, input) {
  return {
    request_id: requestId,
    expected_version: expectedVersion,
    input,
  };
}

export function describeError(body) {
  const error = body && typeof body === "object" && body.error ? body.error : {};
  const details = error.details ?? null;
  const field =
    (details && typeof details === "object" && details.field) ||
    error.field ||
    undefined;
  return {
    code: typeof error.code === "string" ? error.code : "UNKNOWN_ERROR",
    message: typeof error.message === "string" ? error.message : "请求失败",
    details,
    ...(field ? { field } : {}),
  };
}

function profileRow(key, value) {
  if (typeof value === "string") return { key, type: "text", value };
  if (typeof value === "boolean") return { key, type: "boolean", value };
  if (typeof value === "number") return { key, type: "number", value };
  return { key, type: "json", value: JSON.stringify(value, null, 2) };
}

export function profileRows(data = {}) {
  const fields =
    data && typeof data === "object" && !Array.isArray(data) ? data : {};
  const rows = PROFILE_RETAINED_KEYS.map((key) => {
    const value = fields[key];
    return {
      key,
      type: "text",
      value:
        typeof value === "string"
          ? value
          : value == null
            ? ""
            : JSON.stringify(value),
    };
  });
  for (const [key, value] of Object.entries(fields)) {
    if (PROFILE_RESERVED_KEYS.has(key) || PROFILE_RETAINED_KEYS.includes(key)) {
      continue;
    }
    rows.push(profileRow(key, value));
  }
  return rows;
}

export function profileDiff(rows, baseline = {}) {
  const next = Object.create(null);
  for (const row of rows) {
    const key = String(row.key ?? "").trim();
    if (!key) throw new Error("资料键不能为空");
    if (PROFILE_RESERVED_KEYS.has(key)) {
      throw new Error(`资料键 ${key} 为系统保留键`);
    }
    if (Object.prototype.hasOwnProperty.call(next, key)) {
      throw new Error(`资料键重复：${key}`);
    }

    let value = row.value;
    if (row.type === "number") {
      value =
        typeof value === "string" && value.trim() !== ""
          ? Number(value)
          : value;
      if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new Error(`资料 ${key} 必须是数字`);
      }
    } else if (row.type === "boolean") {
      value = value === true || value === "true";
    } else if (row.type !== "json") {
      value = value == null ? "" : String(value);
    }
    if (
      PROFILE_RETAINED_KEYS.includes(key) &&
      typeof value === "string" &&
      value.trim() === ""
    ) {
      throw new Error(`${key === "display_name" ? "展示名称" : "时区"}不能为空`);
    }
    next[key] = value;
  }

  for (const key of PROFILE_RETAINED_KEYS) {
    if (!Object.prototype.hasOwnProperty.call(next, key)) {
      throw new Error(`资料必须包含 ${key}`);
    }
  }

  const patch = Object.create(null);
  for (const [key, value] of Object.entries(next)) {
    if (
      !Object.prototype.hasOwnProperty.call(baseline, key) ||
      JSON.stringify(baseline[key]) !== JSON.stringify(value)
    ) {
      patch[key] = value;
    }
  }

  const remove = [];
  for (const key of Object.keys(baseline)) {
    if (
      PROFILE_RESERVED_KEYS.has(key) ||
      PROFILE_RETAINED_KEYS.includes(key) ||
      Object.prototype.hasOwnProperty.call(next, key)
    ) {
      continue;
    }
    remove.push(key);
  }

  return { patch, remove };
}

export function newRequestId() {
  const randomUUID = globalThis.crypto && globalThis.crypto.randomUUID;
  if (typeof randomUUID === "function") {
    return `req-${randomUUID.call(globalThis.crypto)}`;
  }
  return `req-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
