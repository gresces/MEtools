import assert from "node:assert/strict";
import { test } from "node:test";
import {
  PROFILE_RESERVED_KEYS,
  VERSION_SCOPE,
  actionBudget,
  buildEnvelope,
  describeError,
  formatMinutes,
  newRequestId,
  profileDiff,
  profileRows,
  versionFor,
} from "./dashboard-core.js";

test("action create and queue use the collection version", () => {
  assert.equal(VERSION_SCOPE["personal.action.create"], "collection");
  assert.equal(VERSION_SCOPE["personal.action.queue"], "collection");
  assert.equal(
    versionFor("personal.action.create", { collectionVersion: 9, recordVersion: 2 }),
    9,
  );
});

test("action budget splits duration and elastic buffer", () => {
  assert.deepEqual(actionBudget({ duration_minutes: 30, buffer_minutes: 30 }), {
    duration: 30,
    buffer: 30,
    total: 60,
    durationPercent: 50,
    bufferPercent: 50,
  });
  assert.equal(actionBudget({ duration_minutes: 45, buffer_minutes: 0 }).bufferPercent, 0);
  assert.deepEqual(actionBudget({}), {
    duration: 30,
    buffer: 30,
    total: 60,
    durationPercent: 50,
    bufferPercent: 50,
  });
  assert.equal(formatMinutes(30), "30m");
  assert.equal(formatMinutes(90), "1h 30m");
  assert.equal(formatMinutes(120), "2h");
});

test("action update and archive use the record version", () => {
  assert.equal(
    versionFor("personal.action.update", { collectionVersion: 9, recordVersion: 2 }),
    2,
  );
  assert.equal(
    versionFor("personal.action.complete", { collectionVersion: 9, recordVersion: 2 }),
    2,
  );
  assert.equal(
    versionFor("personal.action.archive", { collectionVersion: 9, recordVersion: 2 }),
    2,
  );
  assert.equal(
    versionFor("personal.action.delete", { collectionVersion: 9, recordVersion: 2 }),
    2,
  );
});

test("event create uses collection and event updates use record versions", () => {
  assert.equal(
    versionFor("personal.event.create", { collectionVersion: 7, recordVersion: 3 }),
    7,
  );
  assert.equal(
    versionFor("personal.event.update", { collectionVersion: 7, recordVersion: 3 }),
    3,
  );
  assert.equal(
    versionFor("personal.event.delete", { collectionVersion: 7, recordVersion: 3 }),
    3,
  );
});

test("goal commands all use the goals collection version", () => {
  for (const command of [
    "goal.goal.create",
    "goal.goal.update",
    "goal.milestone.create",
    "goal.checkpoint.update",
    "goal.action_link.set",
    "goal.progress.record",
  ]) {
    assert.equal(
      versionFor(command, { collectionVersion: 12, recordVersion: 4 }),
      12,
      command,
    );
  }
});

test("profile and life/health scopes follow the server contract", () => {
  assert.equal(
    versionFor("personal.profile.update", { collectionVersion: 5, recordVersion: 1 }),
    5,
  );
  assert.equal(
    versionFor("life.clothes.update", { collectionVersion: 5, recordVersion: 3 }),
    3,
  );
  assert.equal(
    versionFor("life.clothes.add", { collectionVersion: 5, recordVersion: 3 }),
    5,
  );
  assert.equal(
    versionFor("health.water.update", { collectionVersion: 5, recordVersion: 3 }),
    3,
  );
  assert.equal(
    versionFor("health.water.log", { collectionVersion: 5, recordVersion: 3 }),
    5,
  );
});

test("unknown commands and missing versions are rejected", () => {
  assert.throws(() => versionFor("personal.unknown", { collectionVersion: 1, recordVersion: 1 }));
  assert.throws(() =>
    versionFor("personal.action.update", { collectionVersion: 1, recordVersion: null }),
  );
});

test("envelope keeps the three exact keys", () => {
  assert.deepEqual(buildEnvelope("req-1", 3, { title: "x" }), {
    request_id: "req-1",
    expected_version: 3,
    input: { title: "x" },
  });
});

test("error description surfaces code, message, details and field", () => {
  const error = describeError({
    error: {
      code: "VERSION_CONFLICT",
      message: "version has changed",
      details: { field: "title" },
    },
  });
  assert.equal(error.code, "VERSION_CONFLICT");
  assert.equal(error.message, "version has changed");
  assert.deepEqual(error.details, { field: "title" });
  assert.equal(error.field, "title");
});

test("request ids stay non-empty without secure context", () => {
  assert.match(newRequestId(), /^req-/);
});

test("profile rows keep retained keys first and exclude internal fields", () => {
  const rows = profileRows({
    version: 3,
    recent_requests: [],
    occupation: "Engineer",
    display_name: "ME",
    skills: ["Rust"],
    age: 30,
    remote: true,
    nickname: null,
    note: "123",
  });

  assert.deepEqual(
    rows.map((row) => row.key),
    ["display_name", "timezone", "occupation", "skills", "age", "remote", "nickname", "note"],
  );
  assert.deepEqual(rows.find((row) => row.key === "note"), {
    key: "note",
    type: "text",
    value: "123",
  });
  assert.deepEqual(rows.find((row) => row.key === "age"), {
    key: "age",
    type: "number",
    value: 30,
  });
  assert.equal(rows.find((row) => row.key === "remote").type, "boolean");
  assert.equal(rows.find((row) => row.key === "skills").type, "json");
  assert.equal(rows.find((row) => row.key === "nickname").type, "json");
  assert.equal(rows.find((row) => row.key === "nickname").value, "null");
  assert.equal(PROFILE_RESERVED_KEYS.has("version"), true);
});

test("profile diff only patches changes and removes missing keys", () => {
  const baseline = {
    display_name: "ME",
    timezone: "Asia/Shanghai",
    old: "x",
    keep: "same",
    count: 1,
  };
  const rows = [
    { key: "display_name", type: "text", value: "ME" },
    { key: "timezone", type: "text", value: "Asia/Shanghai" },
    { key: "new", type: "text", value: "y" },
    { key: "keep", type: "text", value: "same" },
    { key: "count", type: "number", value: 2 },
  ];

  const diff = profileDiff(rows, baseline);
  assert.deepEqual({ ...diff.patch }, { new: "y", count: 2 });
  assert.deepEqual(diff.remove, ["old"]);
});

test("profile diff rejects duplicate and blank keys", () => {
  assert.throws(
    () =>
      profileDiff(
        [
          { key: "display_name", type: "text", value: "ME" },
          { key: "timezone", type: "text", value: "Asia/Shanghai" },
          { key: "dup", type: "text", value: "a" },
          { key: "dup", type: "text", value: "b" },
        ],
        {},
      ),
    /重复/,
  );
  assert.throws(
    () =>
      profileDiff(
        [
          { key: "display_name", type: "text", value: "ME" },
          { key: "timezone", type: "text", value: "Asia/Shanghai" },
          { key: "  ", type: "text", value: "a" },
        ],
        {},
      ),
    /为空/,
  );
});

test("profile diff rejects whitespace-only retained values", () => {
  assert.throws(
    () =>
      profileDiff(
        [
          { key: "display_name", type: "text", value: "   " },
          { key: "timezone", type: "text", value: "Asia/Shanghai" },
        ],
        {},
      ),
    /不能为空/,
  );
});

test("profile diff supports __proto__ profile keys", () => {
  const baseline = Object.create(null);
  baseline.display_name = "ME";
  baseline.timezone = "Asia/Shanghai";
  baseline["__proto__"] = "old";
  const rows = [
    { key: "display_name", type: "text", value: "ME" },
    { key: "timezone", type: "text", value: "Asia/Shanghai" },
    { key: "__proto__", type: "text", value: "new" },
  ];

  const diff = profileDiff(rows, baseline);
  assert.deepEqual(Object.keys(diff.patch), ["__proto__"]);
  assert.equal(
    Object.getOwnPropertyDescriptor(diff.patch, "__proto__").value,
    "new",
  );
  assert.deepEqual(diff.remove, []);
});

test("profile diff keeps empty text and null values", () => {
  const diff = profileDiff(
    [
      { key: "display_name", type: "text", value: "ME" },
      { key: "timezone", type: "text", value: "Asia/Shanghai" },
      { key: "note", type: "text", value: "" },
      { key: "nickname", type: "json", value: null },
    ],
    {},
  );
  assert.deepEqual({ ...diff.patch }, {
    display_name: "ME",
    timezone: "Asia/Shanghai",
    note: "",
    nickname: null,
  });
  assert.deepEqual(diff.remove, []);
});
