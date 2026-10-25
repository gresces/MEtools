  const assetVersion = document.querySelector('meta[name="me-asset-version"]')?.content || "";
  const {
    PROFILE_RESERVED_KEYS,
    actionBudget,
    buildEnvelope,
    describeError,
    formatMinutes,
    newRequestId,
    profileDiff,
    profileRows
  } = await import(
    `./dashboard-core.js${assetVersion ? `?v=${encodeURIComponent(assetVersion)}` : ""}`
  );

  (() => {
    "use strict";

    const CALENDAR_LEVELS = [
      { key: "actions", label: "行动" },
      { key: "checkpoints", label: "检查点" },
      { key: "milestones", label: "里程碑" },
      { key: "goals", label: "目标" },
      { key: "groups", label: "目标分组" }
    ];

    const state = {
      summary: null,
      profile: null,
      actions: null,
      goals: null,
      health: null,
      life: null,
      view: "overview",
      actionFilter: "open",
      actionSearch: "",
      calendarMode: "list",
      calendarAnchor: fmtDateKey(new Date()),
      calendarLevels: {
        list: new Set(CALENDAR_LEVELS.map((level) => level.key)),
        week: new Set(["actions"]),
        month: new Set(["checkpoints"]),
        year: new Set(["milestones"])
      },
      milestoneOpen: new Set(),
      milestoneSeen: new Set(),
      ganttCollapsed: new Set(),
      pendingActionIds: new Set()
    };
    let commandQueue = Promise.resolve();
    let refreshQueue = Promise.resolve();
    let viewLoadToken = 0;
    let refreshInFlight = false;
    let editorContext = null;
    let pendingCommands = 0;
    let lastAssetCheck = 0;
    let summaryLoadToken = 0;
    const getCache = new Map();
    const getInFlight = new Map();
    const GET_CACHE_LIMIT = 32;
    const REQUEST_TIMEOUT_MS = 20000;
    const REQUEST_RETRIES = 2;

    const viewTitles = {
      overview: "总览",
      actions: "行动",
      goals: "目标",
      calendar: "日程",
      health: "健康",
      life: "生活"
    };

    const $ = (selector) => document.querySelector(selector);
    const $$ = (selector) => Array.from(document.querySelectorAll(selector));

    function esc(value) {
      return String(value ?? "").replace(/[&<>"']/g, (char) => ({
        "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"
      }[char]));
    }

    function tipAttrs(title, meta, detail) {
      return ` data-tip-title="${esc(title)}" data-tip-meta="${esc(meta || "")}" data-tip-detail="${esc(detail || "")}"`;
    }

    function pad(value) { return String(value).padStart(2, "0"); }

    function parseDate(value) {
      if (!value) return null;
      if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
        const [y, m, d] = value.split("-").map(Number);
        return new Date(y, m - 1, d);
      }
      const date = new Date(value);
      return Number.isNaN(date.getTime()) ? null : date;
    }

    function fmtTime(value) {
      const date = value instanceof Date ? value : parseDate(value);
      if (!date) return "—";
      return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
    }

    function fmtDate(value) {
      const date = value instanceof Date ? value : parseDate(value);
      if (!date) return "—";
      return `${date.getMonth() + 1}月${date.getDate()}日`;
    }

    function fmtDateFull(value) {
      const date = value instanceof Date ? value : parseDate(value);
      if (!date) return "—";
      const week = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][date.getDay()];
      return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日 ${week}`;
    }

    function fmtDateKey(value) {
      if (!value) return "";
      if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
      const date = parseDate(value);
      if (!date) return "";
      return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
    }

    function localStamp(date) {
      return `${fmtDateKey(date)}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
    }

    function fmtMinutes(minutes) {
      if (minutes === null || minutes === undefined) return "—";
      const value = Number(minutes);
      if (!Number.isFinite(value)) return "—";
      const h = Math.floor(value / 60);
      const m = Math.round(value % 60);
      return h > 0 ? `${h}h ${pad(m)}m` : `${m}m`;
    }

    function dayLabel(value) {
      const date = parseDate(value);
      if (!date) return String(value ?? "");
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      const target = new Date(date);
      target.setHours(0, 0, 0, 0);
      const diff = Math.round((target - today) / 86400000);
      if (diff === 0) return "今天";
      if (diff === 1) return "明天";
      if (diff === -1) return "昨天";
      if (diff > 1 && diff <= 7) return `${diff} 天后`;
      return fmtDate(date);
    }

    function dueLabel(action) {
      const raw = action.due_at || action.due_date;
      if (!raw) return "无截止";
      const date = parseDate(raw);
      if (!date) return raw;
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      const target = new Date(date);
      target.setHours(0, 0, 0, 0);
      const diff = Math.round((target - today) / 86400000);
      if (diff < 0) return `逾期 ${Math.abs(diff)} 天`;
      if (diff === 0) return "今天到期";
      if (diff === 1) return "明天到期";
      return `${fmtDate(date)} 到期`;
    }

    function dueClass(action) {
      const raw = action.due_at || action.due_date;
      if (!raw) return "";
      const date = parseDate(raw);
      if (!date) return "";
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      const target = new Date(date);
      target.setHours(0, 0, 0, 0);
      const diff = Math.round((target - today) / 86400000);
      if (diff < 0) return "red";
      if (diff === 0) return "amber";
      return "";
    }

    function actionBudgetBar(action) {
      const budget = actionBudget(action);
      const duration = formatMinutes(budget.duration);
      const buffer = formatMinutes(budget.buffer);
      return `<div class="time-budget" title="行动 ${esc(duration)} + 弹性 ${esc(buffer)}">
        <span class="time-budget-action" style="width:${budget.durationPercent}%"></span>
        <span class="time-budget-flex" style="width:${budget.bufferPercent}%"></span>
      </div>
      <div class="time-budget-labels"><span>${esc(duration)}</span><span class="time-budget-flex-label">弹性 ${esc(buffer)}</span></div>`;
    }

    function actionBudgetChip(action) {
      const budget = actionBudget(action);
      return `<span class="time-chip"><span class="time-chip-duration">${esc(formatMinutes(budget.duration))}</span><span class="time-chip-flex">弹性 ${esc(formatMinutes(budget.buffer))}</span></span>`;
    }

    function actionRecurrenceChip(action) {
      if (!action || !action.recurrence || typeof action.recurrence !== "object") return "";
      const total = Number(action.recurrence.count || 0);
      const completed = Number(action.recurrence_completed_count || 0);
      const open = action.status === "open";
      const rule = recurrenceLabel(action.recurrence);
      const progress = total
        ? open && completed < total
          ? `循环 ${total} 次 · 第 ${completed + 1}/${total} 次`
          : `循环 ${total} 次 · 已完成 ${completed} 次`
        : `循环 · 已完成 ${completed} 次`;
      const label = rule ? `${rule} · ${progress}` : progress;
      return `<span class="time-chip"><span class="time-chip-flex">${esc(label)}</span></span>`;
    }

    async function completeRecurringAction(actionId) {
      const document = await actionsDocument(true);
      const action = actionInDocument(document, actionId);
      if (!action) throw new Error("行动不存在或已变化");
      await command("personal", "action.complete", { action_id: actionId }, action.version);
      await refreshResource("actions");
      await refreshResource("goals");
    }

    function priorityClass(priority) {
      if (priority === "high" || priority === "urgent") return "high";
      if (priority === "low") return "low";
      return "normal";
    }

    function priorityLabel(priority) {
      return ({ urgent: "紧急", high: "高", normal: "普通", medium: "普通", low: "低" })[priority] || priority || "普通";
    }

    function statusLabel(status) {
      return ({
        open: "待办", in_progress: "进行中", doing: "进行中", done: "已完成", cancelled: "已取消", archived: "已归档",
        active: "进行中", proposed: "计划中", paused: "暂停", achieved: "已达成", abandoned: "已放弃",
        planned: "计划中", completed: "已完成", skipped: "已跳过", blocked: "阻塞",
        clean: "可穿", wearing: "穿着中", worn: "已穿过", dirty: "待清洗", washing: "清洗中", drying: "晾晒中",
        tentative: "待确认", confirmed: "已确认"
      })[status] || status || "未知";
    }

    function statusBadge(status) {
      const tone = ({
        open: "accent", in_progress: "accent", doing: "accent", done: "green", cancelled: "",
        active: "accent", proposed: "blue", paused: "amber", achieved: "green", abandoned: "",
        planned: "blue", completed: "green", skipped: "", blocked: "red",
        clean: "green", wearing: "accent", dirty: "red", washing: "amber", drying: "amber",
        tentative: "amber", confirmed: "blue"
      })[status] || "";
      return `<span class="badge ${tone}">${esc(statusLabel(status))}</span>`;
    }

    function healthBadge(health) {
      const tone = ({ on_track: "green", at_risk: "amber", blocked: "red", unknown: "" })[health] || "";
      const label = ({ on_track: "正常", at_risk: "有风险", blocked: "阻塞", unknown: "未知" })[health] || health || "未知";
      return `<span class="badge ${tone}">${esc(label)}</span>`;
    }

    function recurrenceLabel(recurrence) {
      if (!recurrence || typeof recurrence !== "object") return "";
      const base = ({ daily: "每天", weekly: "每周", monthly: "每月", yearly: "每年" })[recurrence.frequency];
      if (!base) return "";
      const interval = Number(recurrence.interval || 1);
      return interval > 1 ? `每 ${interval} ${base.replace("每", "")}` : base;
    }

    function pct(value) {
      const number = Number(value);
      return Number.isFinite(number) ? Math.max(0, Math.min(100, number)) : 0;
    }

    function sleep(ms) {
      return new Promise((resolve) => setTimeout(resolve, ms));
    }

    function retryableStatus(status) {
      return [408, 425, 429, 502, 503, 504].includes(status);
    }

    async function fetchWithRetry(path, options = {}) {
      let attempt = 0;
      for (;;) {
        const controller = new AbortController();
        const external = options.signal;
        const onAbort = () => controller.abort();
        const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
        if (external) external.addEventListener("abort", onAbort, { once: true });
        let response;
        try {
          response = await fetch(path, { ...options, signal: controller.signal });
        } catch (error) {
          if (external && external.aborted) throw error;
          if (attempt >= REQUEST_RETRIES) throw error;
          await sleep(300 * 2 ** attempt);
          attempt += 1;
          continue;
        } finally {
          clearTimeout(timeout);
          if (external) external.removeEventListener("abort", onAbort);
        }
        if (retryableStatus(response.status) && attempt < REQUEST_RETRIES) {
          try { await response.body?.cancel(); } catch (_) {}
          await sleep(300 * 2 ** attempt);
          attempt += 1;
          continue;
        }
        return response;
      }
    }

    function rememberGet(path, etag, body) {
      getCache.delete(path);
      getCache.set(path, { etag, body: structuredClone(body) });
      if (getCache.size > GET_CACHE_LIMIT) {
        getCache.delete(getCache.keys().next().value);
      }
    }

    async function api(path, options = {}) {
      const method = (options.method || "GET").toUpperCase();
      const cacheable = method === "GET" && options.cache !== false;
      if (cacheable && getInFlight.has(path)) return getInFlight.get(path);

      const run = (async () => {
        const cached = cacheable ? getCache.get(path) : null;
        const headers = { "Accept": "application/json", ...(options.headers || {}) };
        if (cached && cached.etag) headers["If-None-Match"] = cached.etag;
        const { cache: _cache, ...fetchOptions } = options;
        let response;
        try {
          response = await fetchWithRetry(path, {
            ...fetchOptions,
            method,
            credentials: "same-origin",
            headers
          });
        } catch (error) {
          if (options.signal && options.signal.aborted) throw error;
          throw new Error("网络错误，请检查连接后重试");
        }
        if (response.status === 304) {
          if (cached) return structuredClone(cached.body);
          throw new Error("缓存校验失败，请刷新页面");
        }
        let body = null;
        try { body = await response.json(); } catch (_) { body = null; }
        if (response.status === 401) {
          showLogin();
          const error = new Error("会话已失效，请重新登录");
          error.status = 401;
          throw error;
        }
        if (!response.ok) {
          const error = new Error(
            (body && body.error && body.error.message) || `HTTP ${response.status}`
          );
          error.status = response.status;
          throw error;
        }
        const etag = response.headers.get("etag");
        if (cacheable && etag) rememberGet(path, etag, body);
        return body;
      })();

      if (cacheable) {
        getInFlight.set(path, run);
        const clear = () => {
          if (getInFlight.get(path) === run) getInFlight.delete(path);
        };
        run.then(clear, clear);
      }
      return run;
    }

    function command(program, commandName, input, expectedVersion, requestId = newRequestId()) {
      pendingCommands += 1;
      const run = commandQueue.then(
        () => sendCommand(program, commandName, input, expectedVersion, requestId),
        () => sendCommand(program, commandName, input, expectedVersion, requestId)
      );
      commandQueue = run.then(() => undefined, () => undefined);
      return run.finally(() => {
        pendingCommands = Math.max(0, pendingCommands - 1);
      });
    }

    async function sendCommand(program, commandName, input, expectedVersion, requestId) {
      let response;
      try {
        response = await fetchWithRetry(`/api/v1/programs/${program}/commands/${commandName}`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json", "X-ME-CSRF": "1" },
          body: JSON.stringify(buildEnvelope(requestId, expectedVersion, input))
        });
      } catch (_) {
        throw describeError({
          error: {
            code: "NETWORK_ERROR",
            message: "网络错误，草稿已保留，请检查连接后重试"
          }
        });
      }
      let body = null;
      try { body = await response.json(); } catch (_) { body = null; }
      if (response.status === 401) {
        showLogin();
        throw describeError({ error: { code: "UNAUTHORIZED", message: "会话已失效，请重新登录" } });
      }
      if (!response.ok) {
        const error = describeError(body);
        error.status = response.status;
        throw error;
      }
      // A write may have committed while an older GET was still in flight.
      // Do not let that GET be shared by the post-write refresh.
      getInFlight.clear();
      return body;
    }

    function installCalendarTooltip() {
      const tip = $("#cal-tip");
      if (!tip) return;
      let active = null;
      const place = (event) => {
        if (tip.hidden) return;
        const pad = 14;
        const rect = tip.getBoundingClientRect();
        let x = event.clientX + pad;
        let y = event.clientY + pad;
        if (x + rect.width > window.innerWidth - 8) x = event.clientX - rect.width - pad;
        if (y + rect.height > window.innerHeight - 8) y = event.clientY - rect.height - pad;
        tip.style.left = `${Math.max(8, x)}px`;
        tip.style.top = `${Math.max(8, y)}px`;
      };
      const fill = (target) => {
        tip.replaceChildren();
        const title = document.createElement("div");
        title.className = "cal-tip-title";
        title.textContent = target.dataset.tipTitle || "";
        tip.append(title);
        if (target.dataset.tipMeta) {
          const meta = document.createElement("div");
          meta.className = "cal-tip-meta";
          meta.textContent = target.dataset.tipMeta;
          tip.append(meta);
        }
        if (target.dataset.tipDetail) {
          const detail = document.createElement("div");
          detail.className = "cal-tip-detail";
          detail.textContent = target.dataset.tipDetail;
          tip.append(detail);
        }
        tip.hidden = false;
      };
      document.addEventListener("pointerover", (event) => {
        const target = event.target.closest("[data-tip-title]");
        if (!target) return;
        active = target;
        fill(target);
        place(event);
      });
      document.addEventListener("pointermove", (event) => {
        if (active && !tip.hidden) place(event);
      });
      document.addEventListener("pointerout", (event) => {
        if (!active) return;
        const next = event.relatedTarget;
        if (next && next.closest && next.closest("[data-tip-title]")) return;
        active = null;
        tip.hidden = true;
      });
      window.addEventListener(
        "scroll",
        () => {
          active = null;
          tip.hidden = true;
        },
        true
      );
    }

    function hideToast() {
      const element = $("#toast");
      element.classList.remove("show");
      element.replaceChildren();
      clearTimeout(toast._timer);
      clearTimeout(toastAction._timer);
    }

    function toast(message) {
      const element = $("#toast");
      hideToast();
      const text = document.createElement("span");
      text.textContent = message;
      element.append(text);
      element.classList.add("show");
      toast._timer = setTimeout(hideToast, 2400);
    }

    function toastAction(message, actionLabel, onAction) {
      const element = $("#toast");
      hideToast();
      const text = document.createElement("span");
      text.textContent = message;
      const action = document.createElement("button");
      action.type = "button";
      action.className = "toast-action";
      action.textContent = actionLabel;
      action.addEventListener("click", async () => {
        hideToast();
        try {
          await onAction();
        } catch (error) {
          toast(error.message || "撤销失败");
        }
      });
      element.append(text, action);
      element.classList.add("show");
      toastAction._timer = setTimeout(hideToast, 8000);
    }

    function renderEditorField(field) {
      const id = `editor-field-${field.name}`;
      const required = field.required ? " required" : "";
      const help = field.help ? `<span class="field-help">${esc(field.help)}</span>` : "";
      if (field.type === "checkbox") {
        return `<div class="field checkbox">
          <input id="${id}" name="${esc(field.name)}" type="checkbox">
          <label for="${id}">${esc(field.label)}</label>
          ${help}
        </div>`;
      }
      let control;
      if (field.type === "textarea" || field.type === "json") {
        control = `<textarea id="${id}" name="${esc(field.name)}" rows="${field.rows || 3}"${required}${field.readonly ? " readonly" : ""} placeholder="${esc(field.placeholder || "")}"></textarea>`;
      } else if (field.type === "select") {
        const options = (field.options || []).map((option) =>
          `<option value="${esc(option.value)}">${esc(option.label ?? option.value)}</option>`
        ).join("");
        control = `<select id="${id}" name="${esc(field.name)}"${required}>${options}</select>`;
      } else {
        const attrs = [
          `id="${id}"`,
          `name="${esc(field.name)}"`,
          `type="${esc(field.type || "text")}"`,
          `placeholder="${esc(field.placeholder || "")}"`,
          required,
          field.min !== undefined ? `min="${esc(field.min)}"` : "",
          field.max !== undefined ? `max="${esc(field.max)}"` : "",
          field.step !== undefined ? `step="${esc(field.step)}"` : "",
          field.readonly ? "readonly" : ""
        ].filter(Boolean).join(" ");
        control = `<input ${attrs}>`;
      }
      return `<div class="field">
        <label for="${id}">${esc(field.label)}${field.required ? " *" : ""}</label>
        ${control}
        ${help}
      </div>`;
    }

    function fieldElement(field) {
      return document.getElementById(`editor-field-${field.name}`);
    }

    function fillEditor(values) {
      if (!editorContext) return;
      if (typeof editorContext.options.fillValues === "function") {
        editorContext.options.fillValues(values);
        return;
      }
      for (const field of editorContext.fields) {
        const element = fieldElement(field);
        if (!element) continue;
        const value = values[field.name];
        if (field.type === "checkbox") {
          element.checked = Boolean(value);
        } else if (value === null || value === undefined) {
          element.value = "";
        } else if (Array.isArray(value)) {
          element.value = value.join(", ");
        } else {
          element.value = String(value);
        }
      }
    }

    function readEditorValues(validate = false) {
      if (!editorContext) return {};
      if (typeof editorContext.options.readValues === "function") {
        return editorContext.options.readValues(validate);
      }
      const values = {};
      for (const field of editorContext.fields) {
        const element = fieldElement(field);
        if (!element) continue;
        let value;
        if (field.type === "checkbox") {
          value = element.checked;
        } else if (field.type === "number") {
          value = element.value === "" ? null : Number(element.value);
          if (value !== null && !Number.isFinite(value)) {
            throw new Error(`${field.label}必须是数字`);
          }
        } else if (field.type === "json") {
          const raw = element.value.trim();
          if (!raw) {
            value = null;
          } else {
            try {
              value = JSON.parse(raw);
            } catch (_) {
              throw new Error(`${field.label}不是有效 JSON`);
            }
          }
        } else {
          value = element.value;
        }
        if (validate && field.required && (value === "" || value === null || value === undefined)) {
          throw new Error(`${field.label}不能为空`);
        }
        values[field.name] = value;
      }
      return values;
    }

    function editorIsDirty() {
      if (!editorContext) return false;
      try {
        return JSON.stringify(readEditorValues(false)) !== editorContext.initial;
      } catch (_) {
        return true;
      }
    }

    function setEditorMessage(message, tone = "error") {
      const element = $("#editor-message");
      element.textContent = message || "";
      element.classList.toggle("info", tone === "info");
      element.hidden = !message;
    }

    function setEditorActions(actions = []) {
      const element = $("#editor-actions");
      element.replaceChildren();
      if (!actions.length) {
        element.hidden = true;
        return;
      }
      for (const action of actions) {
        const button = document.createElement("button");
        button.type = "button";
        button.textContent = action.label;
        button.addEventListener("click", async () => {
          try {
            await action.onClick();
          } catch (error) {
            setEditorMessage(error.message || "操作失败");
          }
        });
        element.append(button);
      }
      element.hidden = false;
    }

    function configureEditorDelete(options) {
      const button = $("#editor-delete");
      const hasDelete = typeof options.onDelete === "function";
      button.classList.toggle("hidden", !hasDelete);
      button.textContent = options.deleteLabel || "删除";
      button.disabled = Boolean(options.deleteDisabled);
      if (editorContext) editorContext.deleteDisabled = Boolean(options.deleteDisabled);
      button.onclick = async () => {
        if (!editorContext || !options.onDelete) return;
        const label = button.textContent;
        button.disabled = true;
        button.textContent = "删除中…";
        try {
          await options.onDelete(editorApi(readEditorValues(false)));
        } catch (error) {
          setEditorMessage(error.message || "删除失败");
        } finally {
          if (editorContext) {
            button.disabled = Boolean(editorContext.deleteDisabled);
            button.textContent = label;
          }
        }
      };
    }

    function configureEditorSecondary(options) {
      const button = $("#editor-secondary");
      const label = options.secondaryLabel || "";
      button.classList.toggle("hidden", !label);
      button.textContent = label;
      button.title = options.secondaryTitle || "";
      if (editorContext) editorContext.secondaryDisabled = Boolean(options.secondaryDisabled);
      button.disabled = Boolean(options.secondaryDisabled);
      button.onclick = async () => {
        if (!editorContext || !options.onSecondary) return;
        try {
          await options.onSecondary(editorApi(readEditorValues(false)));
        } catch (error) {
          setEditorMessage(error.message || "操作失败");
        }
      };
    }

    function setEditorBusy(busy) {
      if (!editorContext) return;
      editorContext.busy = busy;
      const submit = $("#editor-submit");
      submit.disabled = busy;
      submit.textContent = busy ? "保存中…" : (editorContext.options.submitLabel || "保存");
      $("#editor-secondary").disabled = busy || Boolean(editorContext.secondaryDisabled);
      $("#editor-delete").disabled = busy || Boolean(editorContext.deleteDisabled);
    }

    function editorApi(values) {
      return {
        values,
        close: () => closeEditor(true),
        setInfo: (message) => setEditorMessage(message, "info"),
        setError: (message) => setEditorMessage(message, "error"),
        setActions: setEditorActions,
        setValues: (next) => {
          fillEditor(next);
          if (editorContext) {
            editorContext.initial = JSON.stringify(readEditorValues(false));
          }
          setEditorActions([]);
        }
      };
    }

    async function submitEditor(event) {
      event.preventDefault();
      if (!editorContext || editorContext.busy) return;
      let values;
      try {
        values = readEditorValues(true);
      } catch (error) {
        setEditorMessage(error.message);
        return;
      }
      setEditorMessage("");
      setEditorActions([]);
      setEditorBusy(true);
      try {
        const keepOpen = await editorContext.options.onSubmit(values, editorApi(values));
        if (!keepOpen && editorContext) closeEditor(true);
      } catch (error) {
        setEditorMessage(error.message || "保存失败");
        if (error.field) {
          const field = editorContext && editorContext.fields.find((item) => item.name === error.field);
          if (field) fieldElement(field)?.focus();
        }
      } finally {
        if (editorContext) setEditorBusy(false);
      }
    }

    function openDrawer(options) {
      openEditor(options, "drawer");
    }

    function openModal(options) {
      openEditor(options, "modal");
    }

    function openEditor(options, mode) {
      if (editorContext) closeEditor(true);
      const fields = options.fields || [];
      editorContext = {
        options,
        fields,
        mode,
        trigger: document.activeElement,
        initial: "{}",
        busy: false
      };
      const layer = $("#editor-layer");
      const panel = $("#editor-panel");
      layer.classList.remove("hidden");
      layer.classList.toggle("modal", mode === "modal");
      layer.setAttribute("aria-hidden", "false");
      panel.setAttribute("tabindex", "-1");
      $("#editor-title").textContent = options.title || "";
      $("#editor-sub").textContent = options.subtitle || "";
      const fieldHost = $("#editor-fields");
      fieldHost.replaceChildren();
      if (typeof options.render === "function") {
        options.render(fieldHost);
      } else {
        fieldHost.innerHTML = fields.map(renderEditorField).join("");
      }
      $("#editor-submit").textContent = options.submitLabel || "保存";
      $("#editor-submit").disabled = false;
      $("#editor-hint").textContent = options.hint || "";
      setEditorMessage("");
      setEditorActions([]);
      configureEditorDelete(options);
      configureEditorSecondary(options);
      fillEditor(options.initial || {});
      editorContext.initial = JSON.stringify(readEditorValues(false));
      requestAnimationFrame(() => {
        const first = panel.querySelector("input:not([type=hidden]), select, textarea");
        (first || panel).focus();
        if (options.onOpen) options.onOpen();
      });
    }

    function closeEditor(force = false) {
      if (!editorContext) return true;
      if (!force && editorIsDirty()) {
        if (!window.confirm("有未保存的修改，确定关闭？")) return false;
      }
      const trigger = editorContext.trigger;
      editorContext = null;
      $("#editor-message").hidden = true;
      $("#editor-actions").hidden = true;
      $("#editor-fields").replaceChildren();
      const layer = $("#editor-layer");
      layer.classList.add("hidden");
      layer.classList.remove("modal");
      layer.setAttribute("aria-hidden", "true");
      if (trigger && typeof trigger.focus === "function") trigger.focus();
      return true;
    }

    function trapEditorFocus(event) {
      if (!editorContext || event.key !== "Tab") return;
      const panel = $("#editor-panel");
      const focusable = Array.from(panel.querySelectorAll(
        "button, input, select, textarea, [href], [tabindex]:not([tabindex='-1'])"
      )).filter((element) => !element.disabled && element.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    function refreshResource(resource) {
      const run = refreshQueue.then(
        () => doRefreshResource(resource),
        () => doRefreshResource(resource)
      );
      refreshQueue = run.then(() => undefined, () => undefined);
      return run;
    }

    function snapshotDataState() {
      return {
        summary: state.summary,
        profile: state.profile,
        actions: state.actions,
        goals: state.goals,
        health: state.health,
        life: state.life
      };
    }

    function restoreDataState(snapshot) {
      Object.assign(state, snapshot);
    }

    function invalidateResource(resource) {
      if (resource === "profile") state.profile = null;
      if (resource === "actions") state.actions = null;
      if (resource === "goals") {
        state.goals = null;
        state.actions = null;
      }
      if (resource === "health") state.health = null;
      if (resource === "life") state.life = null;
    }

    function invalidateAllData() {
      state.summary = null;
      for (const resource of ["profile", "actions", "goals", "health", "life"]) {
        invalidateResource(resource);
      }
    }

    async function doRefreshResource(resource) {
      const snapshot = snapshotDataState();
      invalidateResource(resource);
      try {
        const work = [loadSummary()];
        if (state.view !== "overview") work.push(loadView(state.view));
        await Promise.all(work);
      } catch (error) {
        restoreDataState(snapshot);
        throw error;
      }
    }

    async function profileDocument(force = false) {
      if (force || !state.profile) state.profile = await api("/api/v1/data/profile");
      return state.profile;
    }

    const PROFILE_RETAINED_KEYS = ["display_name", "timezone"];
    const PROFILE_TYPES = [
      { value: "text", label: "文本" },
      { value: "number", label: "数字" },
      { value: "boolean", label: "布尔" },
      { value: "json", label: "JSON" }
    ];

    function profileBaseline(data = {}) {
      const baseline = Object.create(null);
      for (const [key, value] of Object.entries(data || {})) {
        if (!PROFILE_RESERVED_KEYS.has(key)) baseline[key] = value;
      }
      return baseline;
    }

    function profileTypeSelect(row, locked) {
      const select = document.createElement("select");
      select.className = "profile-kv-type";
      for (const type of PROFILE_TYPES) {
        const option = document.createElement("option");
        option.value = type.value;
        option.textContent = type.label;
        select.append(option);
      }
      select.value = row.type;
      select.disabled = locked;
      return select;
    }

    function profileValueControl(row) {
      if (row.type === "boolean") {
        const select = document.createElement("select");
        select.className = "profile-kv-value";
        for (const value of ["true", "false"]) {
          const option = document.createElement("option");
          option.value = value;
          option.textContent = value;
          select.append(option);
        }
        select.value = row.value === true || row.value === "true" ? "true" : "false";
        return select;
      }
      if (row.type === "json") {
        const textarea = document.createElement("textarea");
        textarea.className = "profile-kv-value";
        textarea.rows = 2;
        textarea.value = typeof row.value === "string"
          ? row.value
          : JSON.stringify(row.value, null, 2);
        textarea.placeholder = '例如 {"key": "value"}';
        return textarea;
      }
      if (row.type === "text") {
        const textarea = document.createElement("textarea");
        textarea.className = "profile-kv-value profile-kv-text";
        textarea.rows = 1;
        textarea.value = row.value == null ? "" : String(row.value);
        return textarea;
      }
      const input = document.createElement("input");
      input.className = "profile-kv-value";
      input.type = "number";
      input.step = "any";
      input.value = row.value == null ? "" : String(row.value);
      return input;
    }

    function syncProfileRows(host, rows) {
      const elements = Array.from(host.children);
      for (const [index, row] of rows.entries()) {
        const element = elements[index];
        if (!element) continue;
        const keyInput = element.querySelector(".profile-kv-key");
        const typeSelect = element.querySelector(".profile-kv-type");
        const valueControl = element.querySelector(".profile-kv-value");
        row.key = keyInput.value;
        row.type = typeSelect.value;
        if (row.type === "boolean") {
          row.value = valueControl.value === "true";
        } else if (row.type === "number") {
          const numeric = Number(valueControl.value);
          row.value =
            valueControl.value === "" || !Number.isFinite(numeric)
              ? valueControl.value
              : numeric;
        } else {
          row.value = valueControl.value;
        }
      }
    }

    function readProfileRows(host, rows, validate) {
      const elements = Array.from(host.children);
      const result = [];
      for (const [index, row] of rows.entries()) {
        const element = elements[index];
        if (!element) continue;
        const key = element.querySelector(".profile-kv-key").value.trim();
        const type = element.querySelector(".profile-kv-type").value;
        const control = element.querySelector(".profile-kv-value");
        const raw = type === "boolean" ? control.value === "true" : control.value;
        const emptyValue =
          type === "boolean" ? raw === false : String(raw).trim() === "";
        if (key === "" && emptyValue) continue;

        let value = raw;
        if (type === "number") {
          if (String(raw).trim() === "" || !Number.isFinite(Number(raw))) {
            if (validate) {
              throw new Error(`资料 ${key || "（未命名）"} 必须是数字`);
            }
            value = raw;
          } else {
            value = Number(raw);
          }
        } else if (type === "json") {
          const text = String(raw).trim();
          if (!text) {
            if (validate) {
              throw new Error(`资料 ${key || "（未命名）"} 的 JSON 值不能为空`);
            }
            value = raw;
          } else {
            try {
              value = JSON.parse(text);
            } catch (_) {
              if (validate) {
                throw new Error(`资料 ${key || "（未命名）"} 不是有效 JSON`);
              }
              value = raw;
            }
          }
        }
        result.push({ key, type, value });
      }
      if (validate) {
        for (const key of PROFILE_RETAINED_KEYS) {
          if (!result.some((row) => row.key === key)) {
            throw new Error(`资料必须包含 ${key}`);
          }
        }
      }
      return result;
    }

    function renderProfileRows(host, rows) {
      host.replaceChildren();
      for (const row of rows) {
        const locked = PROFILE_RETAINED_KEYS.includes(row.key);
        const element = document.createElement("div");
        element.className = "profile-kv-row";

        const keyInput = document.createElement("input");
        keyInput.className = "profile-kv-key";
        keyInput.type = "text";
        keyInput.value = row.key;
        keyInput.placeholder = "键";
        keyInput.disabled = locked;
        keyInput.readOnly = locked;

        const typeSelect = profileTypeSelect(row, locked);
        const valueControl = profileValueControl(row);
        const remove = document.createElement(locked ? "span" : "button");
        if (locked) {
          remove.className = "profile-kv-remove-placeholder";
          remove.setAttribute("aria-hidden", "true");
        } else {
          remove.className = "profile-kv-remove";
          remove.type = "button";
          remove.textContent = "删除";
          remove.addEventListener("click", () => {
            syncProfileRows(host, rows);
            const index = rows.indexOf(row);
            if (index >= 0) rows.splice(index, 1);
            renderProfileRows(host, rows);
          });
        }
        typeSelect.addEventListener("change", () => {
          syncProfileRows(host, rows);
          row.type = typeSelect.value;
          if (row.type === "json" && typeof row.value === "string") {
            try {
              JSON.parse(row.value);
            } catch (_) {
              row.value = JSON.stringify(row.value);
            }
          }
          renderProfileRows(host, rows);
        });

        element.append(keyInput, typeSelect, valueControl, remove);
        host.append(element);
      }
    }

    async function saveProfile(diff, version) {
      const input = {};
      if (Object.keys(diff.patch).length > 0) input.patch = diff.patch;
      if (diff.remove.length > 0) input.remove = diff.remove;
      return command("personal", "profile.update", input, version);
    }

    async function openProfileEditor() {
      let profileDoc;
      try {
        profileDoc = await profileDocument(true);
      } catch (error) {
        toast(error.message);
        return;
      }
      const data = profileDoc.data || {};
      let baseline = profileBaseline(data);
      let currentVersion = profileDoc.version;
      let rows = profileRows(data);
      let rowsHost = null;

      openDrawer({
        title: "编辑个人资料",
        subtitle: "任意键值；值支持文本、数字、布尔和 JSON",
        submitLabel: "保存资料",
        initial: rows,
        render: (host) => {
          const intro = document.createElement("p");
          intro.className = "field-help";
          intro.textContent =
            "键可自由增删；display_name 和 timezone 固定保留。值为 JSON 时按 JSON 语法编辑。";
          const list = document.createElement("div");
          list.className = "profile-kv-rows";
          const add = document.createElement("button");
          add.className = "profile-kv-add ghost";
          add.type = "button";
          add.textContent = "新增键值";
          add.addEventListener("click", () => {
            syncProfileRows(list, rows);
            rows.push({ key: "", type: "text", value: "" });
            renderProfileRows(list, rows);
            list.lastElementChild?.querySelector(".profile-kv-key")?.focus();
          });
          host.append(intro, list, add);
          rowsHost = list;
        },
        fillValues: (nextRows) => {
          rows = Array.isArray(nextRows)
            ? nextRows.map((row) => ({ ...row }))
            : profileRows({});
          if (rowsHost) renderProfileRows(rowsHost, rows);
        },
        readValues: (validate) => readProfileRows(rowsHost, rows, validate),
        onSubmit: async (values, editor) => {
          let diff;
          try {
            diff = profileDiff(values, baseline);
          } catch (error) {
            editor.setError(error.message);
            return true;
          }
          if (Object.keys(diff.patch).length === 0 && diff.remove.length === 0) {
            editor.setInfo("没有需要保存的修改。");
            return true;
          }
          try {
            await saveProfile(diff, currentVersion);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("资料已被其他会话更新。可载入最新内容，或用最新版本重试当前修改。");
            editor.setActions([
              {
                label: "载入最新",
                onClick: async () => {
                  const current = await profileDocument(true);
                  currentVersion = current.version;
                  baseline = profileBaseline(current.data || {});
                  editor.setValues(profileRows(current.data || {}));
                  editor.setInfo("已载入最新资料，请确认后重新保存。");
                }
              },
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const current = await profileDocument(true);
                  currentVersion = current.version;
                  let retryRows;
                  try {
                    retryRows = readProfileRows(rowsHost, rows, true);
                  } catch (error) {
                    editor.setError(error.message);
                    return;
                  }
                  const retryDiff = profileDiff(retryRows, baseline);
                  await saveProfile(retryDiff, currentVersion);
                  baseline = profileBaseline(current.data || {});
                  await refreshResource("profile");
                  toast("资料已更新");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("profile");
          toast("资料已更新");
          return false;
        }
      });
    }

    function showLogin() {
      $("#app").classList.add("hidden");
      $("#login").classList.remove("hidden");
      setTimeout(() => $("#password-input").focus(), 60);
    }

    async function showApp() {
      $("#login").classList.add("hidden");
      $("#app").classList.remove("hidden");
    }

    async function login(password) {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json", "Accept": "application/json" },
        body: JSON.stringify({ password })
      });
      const body = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error((body.error && body.error.message) || "登录失败");
      }
      getCache.clear();
      getInFlight.clear();
      await showApp();
      await initData();
    }

    async function logout() {
      try { await fetch("/api/v1/auth/logout", { method: "POST" }); } catch (_) {}
      invalidateAllData();
      getCache.clear();
      getInFlight.clear();
      summaryLoadToken += 1;
      viewLoadToken += 1;
      state.milestoneOpen.clear();
      state.milestoneSeen.clear();
      state.pendingActionIds.clear();
      showLogin();
    }

    function greeting() {
      const hour = new Date().getHours();
      if (hour < 6) return "夜深了";
      if (hour < 12) return "早上好";
      if (hour < 18) return "下午好";
      return "晚上好";
    }

    function renderHeader() {
      const summary = state.summary;
      const profile = summary && summary.profile ? summary.profile : {};
      const name = profile.display_name || "未设置名称";
      $("#profile-mini-name").textContent = name;
      $("#profile-mini-meta").textContent = profile.timezone || "ME data";
      $("#brand-sub").textContent = profile.timezone || "data dashboard";
      $("#updated").textContent = summary ? `更新于 ${fmtTime(summary.generated_at)}` : "尚未加载";
    }

    async function initData() {
      try {
        if (state.view === "overview") {
          // loadSummary renders the overview; do not render it a second time.
          await loadSummary();
        } else {
          await Promise.all([loadSummary(), loadView(state.view)]);
        }
      } catch (error) {
        toast(error.message);
      }
    }

    async function loadSummary() {
      const token = ++summaryLoadToken;
      const summary = await api("/api/v1/dashboard/summary");
      if (token !== summaryLoadToken) return;
      state.summary = summary;
      renderHeader();
      if (state.view === "overview") renderOverview();
    }

    async function loadView(view) {
      const token = ++viewLoadToken;
      if (view === "overview") {
        if (!state.summary) await loadSummary();
        if (token !== viewLoadToken || state.view !== view) return;
        renderOverview();
        return;
      }
      if (view === "actions") {
        if (!state.actions) state.actions = await api("/api/v1/data/actions?include_archived=true");
        if (token !== viewLoadToken || state.view !== view) return;
        renderActions();
        return;
      }
      if (view === "goals") {
        const [goals, actions] = await Promise.all([
          state.goals || api("/api/v1/data/goals"),
          state.actions || api("/api/v1/data/actions?include_archived=true")
        ]);
        if (token !== viewLoadToken || state.view !== view) return;
        state.goals = goals;
        state.actions = actions;
        renderGoals();
        return;
      }
      if (view === "calendar") {
        const [goals, actions] = await Promise.all([
          state.goals || api("/api/v1/data/goals"),
          state.actions || api("/api/v1/data/actions?include_archived=true")
        ]);
        if (token !== viewLoadToken || state.view !== view) return;
        state.goals = goals;
        state.actions = actions;
        renderCalendar();
        return;
      }
      if (view === "health") {
        if (!state.health) state.health = await api("/api/v1/data/health");
        if (token !== viewLoadToken || state.view !== view) return;
        renderHealth();
        return;
      }
      if (view === "life") {
        if (!state.life) state.life = await api("/api/v1/data/life");
        if (token !== viewLoadToken || state.view !== view) return;
        renderLife();
      }
    }

    function setView(view) {
      if (!viewTitles[view]) return;
      state.view = view;
      $$("#nav button").forEach((button) => button.classList.toggle("active", button.dataset.view === view));
      $$(".view").forEach((section) => section.classList.toggle("hidden", section.id !== `view-${view}`));
      $("#page-title").textContent = viewTitles[view];
      loadView(view).catch((error) => toast(error.message));
    }

    /* ---- overview ---- */
    function renderOverview() {
      const root = $("#view-overview");
      const summary = state.summary;
      if (!summary) {
        root.innerHTML = emptyBlock("还没有数据", "运行 me-server init 或使用 me-client 写入第一条记录。");
        return;
      }

      const actions = summary.actions || {};
      const goals = summary.goals || {};
      const health = summary.health || {};
      const life = summary.life || {};
      const profile = summary.profile || {};
      const water = health.water || {};
      const sleep = health.sleep || {};
      const name = profile.display_name ? `，${esc(profile.display_name)}` : "";
      const updated = summary.generated_at ? fmtDateFull(summary.generated_at) : "";

      const stats = [
        { label: "待办行动", value: actions.open ?? 0, hint: (actions.overdue ?? 0) > 0 ? `${actions.overdue} 个已逾期` : "没有逾期", tone: (actions.overdue ?? 0) > 0 ? "red" : "accent", go: "actions" },
        { label: "活跃目标", value: goals.active_goals ?? 0, hint: `平均进度 ${goals.average_progress ?? 0}%`, tone: "purple", go: "goals" },
        { label: "今日饮水", value: `${water.today_ml ?? 0}`, unit: `/ ${water.goal_ml ?? 2000} ml`, hint: `${Math.round((water.ratio || 0) * 100)}% 已完成`, tone: "blue", go: "health" },
        { label: "昨夜睡眠", value: fmtMinutes(sleep.last_night_minutes), hint: sleep.goal_minutes ? `目标 ${fmtMinutes(sleep.goal_minutes)}` : "尚未设置目标", tone: "blue", go: "health" },
        { label: "待洗衣物", value: life.needs_wash ?? 0, hint: `${life.wearing ?? 0} 件正在穿`, tone: (life.needs_wash ?? 0) > 0 ? "amber" : "accent", go: "life" }
      ];

      root.innerHTML = `
        <div class="hero">
          <div>
            <h2>${greeting()}${name}</h2>
            <p>${esc(overviewSummary(summary))}</p>
            <div class="hero-date">${esc(updated)}</div>
          </div>
        </div>

        <div class="stat-grid">
          ${stats.map(statCard).join("")}
        </div>

        <div class="panel">
          <div class="panel-head">
            <h3>行动队列</h3>
            <button class="panel-link" data-goto="actions">队列全部 →</button>
          </div>
          ${queueList(actions.queue || [], 6)}
        </div>

        <div class="grid-2">
          <div class="panel">
            <div class="panel-head">
              <h3>目标进度</h3>
              <button class="panel-link" data-goto="goals">全部目标 →</button>
            </div>
            ${goalPreview(goals.top || [])}
          </div>
          <div class="panel">
            <div class="panel-head">
              <h3>健康趋势</h3>
              <button class="panel-link" data-goto="health">详细数据 →</button>
            </div>
            ${healthPreview(health)}
          </div>
        </div>

        ${projectPreview(summary.projects)}
      `;

      $$("#view-overview [data-goto]").forEach((button) => {
        button.addEventListener("click", () => setView(button.dataset.goto));
      });
    }

    function overviewSummary(summary) {
      const actions = summary.actions || {};
      const goals = summary.goals || {};
      const parts = [];
      const current = Array.isArray(actions.queue) && actions.queue.length ? actions.queue[0] : null;
      if (current) parts.push(`当前行动「${current.title}」`);
      if (actions.due_today) parts.push(`${actions.due_today} 个今天到期`);
      if (goals.milestones_due_soon) parts.push(`${goals.milestones_due_soon} 个里程碑临近`);
      if (!parts.length) return "队列为空，可以安排一点长期推进。";
      return parts.join("，") + "。";
    }

    function statCard(stat) {
      const unit = stat.unit ? `<small>${esc(stat.unit)}</small>` : "";
      return `<button class="stat ${stat.tone || ""}" data-goto="${stat.go}">
        <div class="label">${esc(stat.label)}</div>
        <div class="value">${esc(stat.value)} ${unit}</div>
        <div class="hint">${esc(stat.hint)}</div>
      </button>`;
    }



    function queueList(items, limit) {
      if (!items.length) {
        return emptyBlock("队列为空", "没有排队中的行动；创建行动后会自动进入队列。");
      }
      return `<div class="list">${items.slice(0, limit).map((action, index) => {
        const position = index === 0 ? "当前" : `#${index + 1}`;
        return `<div class="list-item">
          <div class="list-time">${esc(position)}</div>
          <div class="dot ${priorityClass(action.priority)}"></div>
          <div class="list-body">
            <div class="list-title">${esc(action.title)}</div>
            <div class="list-sub"><span class="badge ${dueClass(action)}">${esc(dueLabel(action))}</span> ${actionBudgetChip(action)} ${actionRecurrenceChip(action)}</div>
          </div>
        </div>`;
      }).join("")}</div>`;
    }

    function isGoalGroup(goal) {
      return !(goal.parent_goal_ids || []).length;
    }

    function milestoneSortTime(milestone) {
      const start = dayOnly(milestone.start_date) || dayOnly(milestone.due_at);
      return start ? start.getTime() : Number.MAX_SAFE_INTEGER;
    }

    function milestonesForGoal(milestones, goalId) {
      return milestones
        .filter((milestone) => milestone.goal_id === goalId)
        .sort((left, right) => {
          const leftTime = milestoneSortTime(left);
          const rightTime = milestoneSortTime(right);
          if (leftTime !== rightTime) return leftTime - rightTime;
          const leftSequence = Number(left.sequence ?? 0);
          const rightSequence = Number(right.sequence ?? 0);
          if (leftSequence !== rightSequence) return leftSequence - rightSequence;
          return String(left.id).localeCompare(String(right.id));
        });
    }

    function goalPreview(goals) {
      if (!goals.length) return emptyBlock("还没有活跃目标", "在 goals 集合中创建目标后，这里会显示进度。");
      return goals.slice(0, 4).map((goal) => `
        <div class="goal-card">
          <div class="goal-top">
            <div>
              <div class="goal-title">${esc(goal.title)}</div>
              <div class="goal-meta">
                ${healthBadge(goal.health)}
                ${goal.deadline ? `<span class="badge">${esc(goal.deadline)}</span>` : ""}
                ${(goal.parent_goal_titles || []).slice(0, 1).map((title) => `<span class="badge blue">${esc(title)}</span>`).join("")}
              </div>
            </div>
            <span class="badge accent">${pct(goal.progress)}%</span>
          </div>
        </div>
      `).join("");
    }

    function healthPreview(health) {
      const water = health.water || {};
      const sleep = health.sleep || {};
      const waterSeries = water.series || [];
      const sleepSeries = sleep.series || [];
      return `
        <div class="metric">
          <div class="ring">
            <div><strong>${pct(Math.round((water.ratio || 0) * 100))}%</strong><span>饮水</span></div>
          </div>
          <div class="grow">
            <h4>${esc(water.today_ml ?? 0)} / ${esc(water.goal_ml ?? 2000)} ml</h4>
            <p>今日饮水目标</p>
            ${miniBars(waterSeries.map((item) => item.ml), "water")}
          </div>
        </div>
        <div class="metric">
          <div class="ring">
            <div><strong>${fmtMinutes(sleep.last_night_minutes)}</strong><span>睡眠</span></div>
          </div>
          <div class="grow">
            <h4>${sleep.quality ? `质量 ${esc(sleep.quality)}/5` : "昨夜睡眠"}</h4>
            <p>${sleep.goal_minutes ? `目标 ${esc(fmtMinutes(sleep.goal_minutes))}` : "尚未设置目标"}</p>
            ${miniBars(sleepSeries.map((item) => item.minutes), "sleep")}
          </div>
        </div>
      `;
    }

    function miniBars(values, kind) {
      const max = Math.max(...values.map((value) => Number(value) || 0), 1);
      return `<div class="chart" style="height:46px;margin-top:10px">${values.slice(-14).map((value) => {
        const height = value ? Math.max(4, Math.round((Number(value) / max) * 34)) : 2;
        return `<div class="bar ${kind} ${value ? "" : "no-data"}" title="${esc(value ?? 0)}"><div class="fill" style="height:${height}px"></div></div>`;
      }).join("")}</div>`;
    }

    function projectPreview(projects) {
      if (!projects || !projects.items || !projects.items.length) return "";
      return `<div class="panel" style="margin-top:16px">
        <div class="panel-head"><h3>项目</h3><span class="meta">${projects.active ?? 0} 个活跃</span></div>
        <div class="list">${projects.items.map((project) => `
          <div class="list-item">
            <div class="dot open"></div>
            <div class="list-body">
              <div class="list-title">${esc(project.title || project.id)}</div>
              <div class="list-sub">${esc(project.next_action || project.status || "—")}</div>
            </div>
            ${statusBadge(project.status)}
          </div>
        `).join("")}</div>
      </div>`;
    }

    /* ---- actions ---- */
    function actionQueueIndex() {
      const queue = state.actions && state.actions.data && Array.isArray(state.actions.data.queue)
        ? state.actions.data.queue
        : [];
      return new Map(queue.map((id, index) => [id, index]));
    }

    async function actionsDocument(force = false) {
      if (force || !state.actions) {
        state.actions = await api("/api/v1/data/actions?include_archived=true");
      }
      return state.actions;
    }

    function actionInDocument(document, id) {
      return document && document.data && Array.isArray(document.data.items)
        ? document.data.items.find((item) => item.id === id)
        : null;
    }

    function apiOffset() {
      const raw = Number(state.summary && state.summary.timezone_offset_hours);
      const hours = Number.isFinite(raw) ? raw : 8;
      return `${hours >= 0 ? "+" : "-"}${pad(Math.abs(hours))}:00`;
    }

    function toApiDateTime(value) {
      if (!value) return null;
      return `${value.length === 16 ? `${value}:00` : value}${apiOffset()}`;
    }

    function toLocalDateTime(value) {
      return value ? String(value).slice(0, 16) : "";
    }

    function actionFormValues(action) {
      return {
        title: action.title || "",
        notes: action.notes || "",
        status: action.status || "open",
        priority: action.priority || "normal",
        due_date: action.due_date || "",
        due_at: toLocalDateTime(action.due_at),
        tags: (action.tags || []).join(", "),
        duration_minutes: action.duration_minutes ?? 30,
        buffer_minutes: action.buffer_minutes ?? action.duration_minutes ?? 30,
        recurrence: action.recurrence ? JSON.stringify(action.recurrence, null, 2) : ""
      };
    }

    function actionValuesToInput(values, includeStatus = false) {
      const dueDate = values.due_date.trim();
      const dueAt = values.due_at.trim();
      if (dueDate && dueAt) {
        throw new Error("截止日期与截止时间只能填写一个");
      }
      const duration = Number(values.duration_minutes);
      const buffer = Number(values.buffer_minutes);
      if (!Number.isFinite(duration) || duration <= 0) {
        throw new Error("行动时长必须是大于 0 的分钟数");
      }
      if (!Number.isFinite(buffer) || buffer < 0) {
        throw new Error("弹性时间必须是非负分钟数");
      }
      let recurrence = values.recurrence;
      if (typeof recurrence === "string") {
        recurrence = recurrence.trim() ? JSON.parse(recurrence) : null;
      }
      if (recurrence !== null && recurrence !== undefined) {
        if (typeof recurrence !== "object" || Array.isArray(recurrence)) {
          throw new Error("重复规则必须是 JSON 对象");
        }
      } else {
        recurrence = null;
      }
      const input = {
        title: values.title.trim(),
        notes: values.notes,
        priority: values.priority,
        due_date: dueDate || null,
        due_at: dueAt ? toApiDateTime(dueAt) : null,
        tags: values.tags.split(",").map((tag) => tag.trim()).filter(Boolean),
        duration_minutes: duration,
        buffer_minutes: buffer,
        recurrence
      };
      if (includeStatus) input.status = values.status;
      return input;
    }

    function actionPatchFromValues(values, action) {
      const target = actionValuesToInput(values, true);
      const patch = {};
      for (const field of [
        "title",
        "notes",
        "priority",
        "status",
        "due_date",
        "due_at",
        "duration_minutes",
        "buffer_minutes",
        "recurrence"
      ]) {
        if (JSON.stringify(target[field] ?? null) !== JSON.stringify(action[field] ?? null)) {
          patch[field] = target[field];
        }
      }
      if (JSON.stringify(target.tags) !== JSON.stringify(action.tags || [])) {
        patch.tags = target.tags;
      }
      return patch;
    }

    function bindDueFields() {
      const date = fieldElement({ name: "due_date" });
      const at = fieldElement({ name: "due_at" });
      if (!date || !at) return;
      date.addEventListener("change", () => {
        if (date.value) at.value = "";
      });
      at.addEventListener("change", () => {
        if (at.value) date.value = "";
      });
    }

    function bindActionBudgetFields() {
      const duration = fieldElement({ name: "duration_minutes" });
      const buffer = fieldElement({ name: "buffer_minutes" });
      if (!duration || !buffer) return;
      let previous = Number(duration.value);
      duration.addEventListener("input", () => {
        const next = Number(duration.value);
        if (!Number.isFinite(next)) return;
        if (Number(buffer.value) === previous) buffer.value = String(next);
        previous = next;
      });
    }

    function bindActionFields() {
      bindDueFields();
      bindActionBudgetFields();
    }

    function checkpointChoiceLabel(document, checkpoint) {
      const data = document && document.data ? document.data : {};
      const milestone = (data.milestones || []).find(
        (item) => item.id === checkpoint.milestone_id
      );
      const goal = milestone
        ? (data.goals || []).find((item) => item.id === milestone.goal_id)
        : null;
      return [goal && goal.title, milestone && milestone.title, checkpoint.title]
        .filter(Boolean)
        .join(" / ");
    }

    async function openActionCreate(options = {}) {
      const goals = await loadGoalsDocument(true);
      const checkpoints = goals && goals.data && Array.isArray(goals.data.checkpoints)
        ? goals.data.checkpoints
        : [];
      if (!checkpoints.length) {
        toast("请先在目标页创建目标、里程碑和检查点，再新建行动");
        return;
      }
      const checkpointOptions = checkpoints.map((checkpoint) => ({
        value: checkpoint.id,
        label: checkpointChoiceLabel(goals, checkpoint)
      }));
      if (
        options.checkpointId &&
        !checkpointOptions.some((option) => option.value === options.checkpointId)
      ) {
        toast("所选检查点已不存在，请刷新目标页");
        return;
      }
      const selected = options.checkpointId || checkpointOptions[0].value;
      const document = await actionsDocument();
      let version = document.version;
      const finish = async (editor) => {
        await refreshResource("actions");
        await refreshResource("goals");
        toast("行动已创建并关联检查点");
        editor.close();
      };
      openModal({
        title: "新建行动",
        subtitle: "行动必须挂在一个已存在的检查点下",
        submitLabel: "创建行动",
        initial: {
          priority: "normal",
          status: "open",
          checkpoint_id: selected,
          duration_minutes: 30,
          buffer_minutes: 30,
          recurrence: ""
        },
        fields: [
          {
            name: "checkpoint_id",
            label: "所属检查点",
            type: "select",
            options: checkpointOptions,
            required: true,
            help: "行动会同时关联检查点、里程碑和目标"
          },
          { name: "title", label: "标题", required: true },
          { name: "notes", label: "备注", type: "textarea", rows: 3 },
          {
            name: "priority",
            label: "优先级",
            type: "select",
            options: [
              { value: "high", label: "高" },
              { value: "normal", label: "普通" },
              { value: "low", label: "低" }
            ]
          },
          { name: "due_date", label: "截止日期", type: "date" },
          { name: "due_at", label: "截止时间", type: "datetime-local" },
          {
            name: "duration_minutes",
            label: "行动时长（分钟）",
            type: "number",
            min: 1,
            max: 600,
            required: true
          },
          {
            name: "buffer_minutes",
            label: "弹性时间（分钟）",
            type: "number",
            min: 0,
            max: 600,
            required: true,
            help: "默认等于行动时长；修改时长时若未单独调整，会同步更新"
          },
          {
            name: "recurrence",
            label: "重复规则（JSON）",
            type: "json",
            rows: 4,
            help: "留空表示一次性；示例 {\"frequency\":\"weekly\",\"count\":8}；循环行动需要截止日期或时间"
          },
          { name: "tags", label: "标签", help: "用逗号分隔" }
        ],
        onOpen: bindActionFields,
        onSubmit: async (values, editor) => {
          const input = actionValuesToInput(values);
          input.checkpoint_id = values.checkpoint_id;
          try {
            await command("personal", "action.create", input, version);
            await finish(editor);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("行动集合已被其他会话更新。可用最新版本重试创建。");
            editor.setActions([
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await actionsDocument(true);
                  version = latest.version;
                  await command("personal", "action.create", input, version);
                  await finish(editor);
                }
              }
            ]);
            return true;
          }
        }
      });
    }

    function actionLinkValues(link = {}) {
      return {
        link_goal_ids: (link.goal_ids || []).join(", "),
        link_goal_id: link.goal_id || "",
        link_milestone_id: link.milestone_id || "",
        link_checkpoint_id: link.checkpoint_id || "",
        link_role: link.role || "core",
        link_contribution: link.contribution || ""
      };
    }

    function actionLinkTarget(values) {
      return {
        goal_ids: splitCommas(values.link_goal_ids),
        goal_id: values.link_goal_id || null,
        milestone_id: values.link_milestone_id || null,
        checkpoint_id: values.link_checkpoint_id || null,
        role: values.link_role || "core",
        contribution: values.link_contribution || ""
      };
    }

    function actionLinkHasReference(link) {
      return Boolean(
        (link.goal_ids || []).length ||
        link.goal_id ||
        link.milestone_id ||
        link.checkpoint_id
      );
    }

    function actionLinkFields(goalsDoc) {
      const options = (items) => [
        { value: "", label: "无" },
        ...items.map((item) => ({ value: item.id, label: item.title }))
      ];
      return [
        {
          name: "link_goal_ids",
          label: "关联目标/分组 ID",
          help: "多个用逗号分隔；也可以只选下方单个目标"
        },
        {
          name: "link_goal_id",
          label: "主目标 / 分组",
          type: "select",
          options: options(goalsDoc.goals || [])
        },
        {
          name: "link_milestone_id",
          label: "里程碑",
          type: "select",
          options: options(goalsDoc.milestones || [])
        },
        {
          name: "link_checkpoint_id",
          label: "检查点",
          type: "select",
          options: options(goalsDoc.checkpoints || [])
        },
        {
          name: "link_role",
          label: "关联角色",
          type: "select",
          options: [
            { value: "core", label: "核心" },
            { value: "supporting", label: "支持" },
            { value: "maintenance", label: "维护" }
          ]
        },
        { name: "link_contribution", label: "贡献说明" }
      ];
    }

    async function openActionEditor(id) {
      let document = await actionsDocument();
      let action = actionInDocument(document, id);
      if (!action) {
        toast("行动不存在或已变化");
        return;
      }
      let goals = await loadGoalsDocument();
      let goalsDoc = goals.data || {};
      let goalsVersion = goals.version;
      let existingLink = (goalsDoc.action_links || []).find((link) => link.action_id === id);
      let baselineLink = actionLinkValues(existingLink);
      let baselineLinkTarget = actionLinkTarget(baselineLink);
      let baseline = actionFormValues(action);
      let baselineAction = action;
      let version = action.version;
      const canArchive = Boolean(action.archived_at) || ["done", "cancelled"].includes(action.status);
      openDrawer({
        title: "编辑行动",
        subtitle: action.id,
        submitLabel: "保存行动",
        deleteLabel: "删除行动",
        onDelete: async (editor) => {
          if (!window.confirm(`删除行动「${action.title}」？它的目标关联会被移除，关联日程会取消关联。`)) return;
          const latest = await actionsDocument(true);
          const fresh = actionInDocument(latest, id);
          if (!fresh) throw new Error("行动已不存在");
          await command("personal", "action.delete", { action_id: id }, fresh.version);
          await refreshResource("goals");
          toast("行动已删除");
          editor.close();
        },
        initial: { ...baseline, ...baselineLink },
        fields: [
          { name: "title", label: "标题", required: true },
          { name: "notes", label: "备注", type: "textarea", rows: 3 },
          {
            name: "status",
            label: "状态",
            type: "select",
            options: [
              { value: "open", label: "待办" },
              { value: "done", label: "已完成" },
              { value: "cancelled", label: "已取消" }
            ]
          },
          {
            name: "priority",
            label: "优先级",
            type: "select",
            options: [
              { value: "high", label: "高" },
              { value: "normal", label: "普通" },
              { value: "low", label: "低" }
            ]
          },
          { name: "due_date", label: "截止日期", type: "date" },
          { name: "due_at", label: "截止时间", type: "datetime-local" },
          {
            name: "duration_minutes",
            label: "行动时长（分钟）",
            type: "number",
            min: 1,
            max: 600,
            required: true
          },
          {
            name: "buffer_minutes",
            label: "弹性时间（分钟）",
            type: "number",
            min: 0,
            max: 600,
            required: true,
            help: "默认等于行动时长；修改时长时若未单独调整，会同步更新"
          },
          {
            name: "recurrence",
            label: "重复规则（JSON）",
            type: "json",
            rows: 4,
            help: "留空表示一次性；修改规则会重置完成次数；循环行动需要截止日期或时间"
          },
          { name: "tags", label: "标签", help: "用逗号分隔" },
          ...actionLinkFields(goalsDoc)
        ],
        secondaryLabel: action.archived_at ? "恢复" : "归档",
        secondaryDisabled: !canArchive,
        secondaryTitle: canArchive ? "" : "只有已完成或已取消的行动可以归档",
        onOpen: bindActionFields,
        onSecondary: async (editor) => {
          const latest = await actionsDocument(true);
          const fresh = actionInDocument(latest, id);
          if (!fresh) throw new Error("行动已不存在");
          const archived = !fresh.archived_at;
          await command(
            "personal",
            "action.archive",
            { action_id: id, archived },
            fresh.version
          );
          await refreshResource("actions");
          editor.close();
          if (archived) {
            toastAction("行动已归档", "撤销", async () => {
              const undone = await actionsDocument(true);
              const current = actionInDocument(undone, id);
              if (!current) throw new Error("行动已不存在");
              await command(
                "personal",
                "action.archive",
                { action_id: id, archived: false },
                current.version
              );
              await refreshResource("actions");
              toast("已恢复");
            });
          } else {
            toast("行动已恢复");
          }
        },
        onSubmit: async (values, editor) => {
          const patch = actionPatchFromValues(values, baselineAction);
          const targetLink = actionLinkTarget(values);
          const linkChanged = JSON.stringify(targetLink) !== JSON.stringify(baselineLinkTarget);
          if (Object.keys(patch).length === 0 && !linkChanged) {
            editor.setInfo("没有需要保存的修改。");
            return true;
          }
          const persist = async (actionVersion) => {
            if (Object.keys(patch).length > 0) {
              await command("personal", "action.update", { action_id: id, patch }, actionVersion);
            }
            if (linkChanged) {
              if (actionLinkHasReference(targetLink)) {
                await command(
                  "goal",
                  "action_link.set",
                  { action_id: id, ...targetLink },
                  goalsVersion
                );
              } else if (existingLink) {
                await command("goal", "action_link.remove", { action_id: id }, goalsVersion);
              }
            }
          };
          try {
            await persist(version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("行动或目标关联已被其他会话更新。可载入最新内容，或用最新版本重试当前修改。");
            editor.setActions([
              {
                label: "载入最新",
                onClick: async () => {
                  const latestActions = await actionsDocument(true);
                  const fresh = actionInDocument(latestActions, id);
                  if (!fresh) throw new Error("行动已不存在");
                  const latestGoals = await loadGoalsDocument(true);
                  goalsVersion = latestGoals.version;
                  existingLink = (latestGoals.data.action_links || []).find((link) => link.action_id === id);
                  baselineLink = actionLinkValues(existingLink);
                  baselineLinkTarget = actionLinkTarget(baselineLink);
                  baselineAction = fresh;
                  baseline = actionFormValues(fresh);
                  version = fresh.version;
                  editor.setValues({ ...baseline, ...baselineLink });
                  editor.setInfo("已载入最新内容，请确认后重新保存。");
                }
              },
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latestActions = await actionsDocument(true);
                  const fresh = actionInDocument(latestActions, id);
                  if (!fresh) throw new Error("行动已不存在");
                  const latestGoals = await loadGoalsDocument(true);
                  version = fresh.version;
                  goalsVersion = latestGoals.version;
                  await persist(version);
                  await refreshResource("goals");
                  toast("行动已更新");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("行动已更新");
          return false;
        }
      });
    }

    async function undoActionStatus(id, status) {
      const document = await actionsDocument(true);
      const action = actionInDocument(document, id);
      if (!action) throw new Error("行动已不存在");
      await command(
        "personal",
        "action.update",
        { action_id: id, patch: { status } },
        action.version
      );
      await refreshResource("actions");
      toast("已撤销");
    }

    async function toggleActionStatus(id, done, onChange) {
      const document = await actionsDocument();
      const action = actionInDocument(document, id);
      if (!action) return;
      const previous = action.status;
      const status = done ? "done" : "open";
      action.status = status;
      if (onChange) onChange();
      else renderActions();
      try {
        await command("personal", "action.update", { action_id: id, patch: { status } }, action.version);
      } catch (error) {
        await refreshResource("actions");
        toast(error.message || "更新行动失败");
        return;
      }
      await refreshResource("actions");
      toastAction(done ? "行动已完成" : "行动已重开", "撤销", async () => {
        await undoActionStatus(id, previous === "done" ? "done" : "open");
      });
    }

    async function moveQueueAction(id, move) {
      const document = await actionsDocument();
      const queue = document.data.queue || [];
      const index = queue.indexOf(id);
      let position;
      if (move === "add") position = queue.length;
      else if (move === "top") position = 0;
      else if (move === "up") position = index - 1;
      else if (move === "down") position = index + 1;
      else return;
      if (position < 0 || (move === "down" && index === queue.length - 1)) return;
      if (move !== "add" && index === -1) return;
      const original = [...queue];
      if (index !== -1) queue.splice(index, 1);
      queue.splice(Math.min(position, queue.length), 0, id);
      document.data.queue = queue;
      renderActions();
      try {
        await command(
          "personal",
          "action.queue",
          { action_id: id, position },
          document.version
        );
      } catch (error) {
        document.data.queue = original;
        await refreshResource("actions");
        toast(error.message || "调整队列失败");
        return;
      }
      await refreshResource("actions");
    }

    function renderActions() {
      const root = $("#view-actions");
      const items = state.actions && state.actions.data && state.actions.data.items ? state.actions.data.items : [];
      const queueIndex = actionQueueIndex();
      const filtered = filterActions(items);
      const activeCount = items.filter((item) => !item.archived_at).length;
      const openCount = items.filter((item) => !item.archived_at && ["open", "in_progress", "doing"].includes(item.status)).length;
      const doneCount = items.filter((item) => !item.archived_at && item.status === "done").length;
      const current = queueIndex.size ? items.find((item) => item.id === [...queueIndex.keys()][0]) : null;

      root.innerHTML = `
        <div class="hero">
          <div>
            <h2>${openCount} 个待办，${doneCount} 个已完成</h2>
            <p>队列 ${queueIndex.size} 项${current ? ` · 当前：${esc(current.title)}` : ""}</p>
          </div>
          <div class="hero-actions">
            <button id="new-action" class="hero-action" type="button">+ 行动</button>
            <div class="badge accent">${filtered.length} 条显示</div>
          </div>
        </div>
        <div class="filter-bar">
          ${actionFilterButton("open", `待办 ${openCount}`)}
          ${actionFilterButton("all", `全部 ${activeCount}`)}
          ${actionFilterButton("done", `已完成 ${doneCount}`)}
          ${actionFilterButton("archived", "已归档")}
          <input class="search" id="action-search" placeholder="搜索标题、备注、标签…" value="${esc(state.actionSearch)}">
        </div>
        ${filtered.length ? actionTable(filtered, queueIndex) : emptyBlock("没有匹配的行动", "调整筛选条件或搜索关键词。")}
      `;

      $$("#view-actions [data-filter]").forEach((button) => {
        button.addEventListener("click", () => {
          state.actionFilter = button.dataset.filter;
          renderActions();
        });
      });
      const search = $("#action-search");
      if (search) {
        search.addEventListener("input", () => {
          state.actionSearch = search.value;
          renderActions();
          const next = $("#action-search");
          if (next) {
            next.focus();
            next.setSelectionRange(next.value.length, next.value.length);
          }
        });
      }
      const create = $("#new-action");
      if (create) create.addEventListener("click", () => openActionCreate().catch((error) => toast(error.message)));

      $$("#view-actions [data-action-row]").forEach((row) => {
        const open = () => openActionEditor(row.dataset.actionRow).catch((error) => toast(error.message));
        row.addEventListener("click", (event) => {
          if (event.target.closest("button, input, label, a")) return;
          open();
        });
        row.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") {
            if (event.target.closest("button, input, label, a")) return;
            event.preventDefault();
            open();
          }
        });
      });
      $$("#view-actions [data-edit-action]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openActionEditor(button.dataset.editAction).catch((error) => toast(error.message));
        });
      });
      $$("#view-actions [data-complete-action]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          completeRecurringAction(button.dataset.completeAction)
            .then(() => toast("已完成本次"))
            .catch((error) => toast(error.message));
        });
      });
      $$("#view-actions [data-queue-move]").forEach((button) => {
        button.addEventListener("click", () => {
          moveQueueAction(button.dataset.actionId, button.dataset.queueMove)
            .catch((error) => toast(error.message));
        });
      });
    }

    function actionFilterButton(filter, label) {
      return `<button class="chip ${state.actionFilter === filter ? "active" : ""}" data-filter="${filter}">${esc(label)}</button>`;
    }

    function filterActions(items) {
      const query = state.actionSearch.trim().toLowerCase();
      const queueIndex = actionQueueIndex();
      return items.filter((item) => {
        const archived = Boolean(item.archived_at);
        if (state.actionFilter === "archived" && !archived) return false;
        if (state.actionFilter !== "archived" && archived) return false;
        if (state.actionFilter === "open" && !["open", "in_progress", "doing"].includes(item.status)) return false;
        if (state.actionFilter === "done" && item.status !== "done") return false;
        if (query) {
          const haystack = [item.title, item.notes, ...(item.tags || [])].join(" ").toLowerCase();
          if (!haystack.includes(query)) return false;
        }
        return true;
      }).sort((left, right) => {
        const leftQueue = queueIndex.has(left.id) ? queueIndex.get(left.id) : null;
        const rightQueue = queueIndex.has(right.id) ? queueIndex.get(right.id) : null;
        if (leftQueue !== null && rightQueue !== null) return leftQueue - rightQueue;
        if (leftQueue !== null) return -1;
        if (rightQueue !== null) return 1;
        const leftOpen = ["open", "in_progress", "doing"].includes(left.status) ? 0 : 1;
        const rightOpen = ["open", "in_progress", "doing"].includes(right.status) ? 0 : 1;
        if (leftOpen !== rightOpen) return leftOpen - rightOpen;
        const leftDue = parseDate(left.due_at || left.due_date);
        const rightDue = parseDate(right.due_at || right.due_date);
        if (leftDue && rightDue && leftDue.getTime() !== rightDue.getTime()) return leftDue - rightDue;
        if (leftDue && !rightDue) return -1;
        if (!leftDue && rightDue) return 1;
        const order = { urgent: 0, high: 1, normal: 2, medium: 2, low: 3 };
        return (order[left.priority] ?? 2) - (order[right.priority] ?? 2);
      });
    }

    function actionTable(items, queueIndex) {
      return `<div class="table-wrap action-table"><table>
        <thead><tr>
          <th>队列</th><th>行动</th><th>状态</th><th>优先级</th><th>截止</th><th>更新时间</th><th>操作</th>
        </tr></thead>
        <tbody>${items.map((action) => {
          const queuePosition = queueIndex.get(action.id);
          const queued = queuePosition !== undefined;
          const canQueue = !action.archived_at && action.status === "open";
          const queueHint = canQueue ? "" : "只有未归档的待办行动可以加入队列";
          return `<tr class="editable-row" data-action-row="${esc(action.id)}" tabindex="0">
            <td class="queue-cell">
              <div class="nowrap">${queued ? (queuePosition === 0 ? "当前" : `#${queuePosition + 1}`) : "—"}</div>
              <div class="queue-actions">
                ${queued ? `
                  <button type="button" data-action-id="${esc(action.id)}" data-queue-move="top" ${queuePosition === 0 ? "disabled" : ""} title="置顶" aria-label="置顶 ${esc(action.title)}">⇤</button>
                  <button type="button" data-action-id="${esc(action.id)}" data-queue-move="up" ${queuePosition === 0 ? "disabled" : ""} title="上移" aria-label="上移 ${esc(action.title)}">↑</button>
                  <button type="button" data-action-id="${esc(action.id)}" data-queue-move="down" ${queuePosition === queueIndex.size - 1 ? "disabled" : ""} title="下移" aria-label="下移 ${esc(action.title)}">↓</button>
                ` : `
                  <button type="button" data-action-id="${esc(action.id)}" data-queue-move="add" ${canQueue ? "" : "disabled"} title="${esc(queueHint || "加入队列")}" aria-label="加入队列 ${esc(action.title)}">入队</button>
                `}
              </div>
            </td>
            <td>
              <div class="action-title">
                <div>
                  <div>${esc(action.title)}</div>
                  ${action.notes ? `<div class="action-notes">${esc(action.notes)}</div>` : ""}
                  ${(action.tags || []).length ? `<div class="tag-row">${action.tags.map((tag) => `<span class="tag">${esc(tag)}</span>`).join("")}</div>` : ""}
                  ${actionBudgetBar(action)}
                  ${actionRecurrenceChip(action) ? `<div class="tag-row">${actionRecurrenceChip(action)}</div>` : ""}
                </div>
              </div>
            </td>
            <td>${statusBadge(action.archived_at ? "archived" : action.status)}</td>
            <td><span class="badge ${priorityClass(action.priority) === "high" ? "red" : priorityClass(action.priority) === "low" ? "" : "blue"}">${esc(priorityLabel(action.priority))}</span></td>
            <td><span class="badge ${dueClass(action)}">${esc(dueLabel(action))}</span></td>
            <td class="muted nowrap">${esc(action.updated_at ? fmtDate(action.updated_at) : "—")}</td>
            <td class="nowrap">
              ${action.recurrence && action.status === "open" && !action.archived_at ? `<button type="button" class="mini-action" data-complete-action="${esc(action.id)}">完成本次</button> ` : ""}
              <button type="button" class="mini-action" data-edit-action="${esc(action.id)}">编辑</button>
            </td>
          </tr>`;
        }).join("")}</tbody>
      </table></div>`;
    }

    /* ---- goals ---- */
    function horizonLabel(horizon) {
      if (["year", "years"].includes(horizon)) return "年度";
      if (["month", "months"].includes(horizon)) return "月度";
      if (horizon === "long") return "长期";
      return horizon || "目标";
    }

    async function loadGoalsDocument(force = false) {
      if (force || !state.goals) state.goals = await api("/api/v1/data/goals");
      return state.goals;
    }

    function goalInDocument(document, id) {
      return document && document.data && Array.isArray(document.data.goals)
        ? document.data.goals.find((goal) => goal.id === id)
        : null;
    }

    function milestoneInDocument(document, id) {
      return document && document.data && Array.isArray(document.data.milestones)
        ? document.data.milestones.find((item) => item.id === id)
        : null;
    }

    function checkpointInDocument(document, id) {
      return document && document.data && Array.isArray(document.data.checkpoints)
        ? document.data.checkpoints.find((item) => item.id === id)
        : null;
    }

    function splitLines(value) {
      return value.split("\n").map((item) => item.trim()).filter(Boolean);
    }

    function splitCommas(value) {
      return value.split(",").map((item) => item.trim()).filter(Boolean);
    }

    function goalFormValues(goal) {
      return {
        title: goal.title || "",
        description: goal.description || "",
        purpose: goal.purpose || "",
        horizon: goal.horizon || "long",
        status: goal.status || "active",
        health: goal.health || "unknown",
        priority: goal.priority || "normal",
        start_date: goal.start_date || "",
        deadline: goal.deadline || "",
        success_criteria: (goal.success_criteria || []).join("\n"),
        constraints: (goal.constraints || []).join("\n"),
        parent_goal_ids: (goal.parent_goal_ids || []).join(", "),
        progress: goal.progress ?? 0,
        next_action: goal.next_action || ""
      };
    }

    function goalPatchFromValues(values, goal) {
      const target = {
        title: values.title.trim(),
        description: values.description,
        purpose: values.purpose,
        horizon: values.horizon.trim(),
        status: values.status,
        health: values.health,
        priority: values.priority,
        start_date: values.start_date || null,
        deadline: values.deadline || null,
        success_criteria: splitLines(values.success_criteria),
        constraints: splitLines(values.constraints),
        parent_goal_ids: splitCommas(values.parent_goal_ids),
        progress: values.progress,
        next_action: values.next_action
      };
      const patch = {};
      for (const key of Object.keys(target)) {
        if (JSON.stringify(target[key] ?? null) !== JSON.stringify(goal[key] ?? null)) {
          patch[key] = target[key];
        }
      }
      return patch;
    }

    function goalFields() {
      return [
        { name: "title", label: "标题", required: true },
        { name: "description", label: "描述", type: "textarea", rows: 3 },
        { name: "purpose", label: "目的", type: "textarea", rows: 2 },
        {
          name: "horizon",
          label: "时间尺度",
          placeholder: "years / months / long",
          help: "可自定义，例如 years、months、long"
        },
        {
          name: "status",
          label: "状态",
          type: "select",
          options: [
            { value: "active", label: "进行中" },
            { value: "proposed", label: "计划中" },
            { value: "paused", label: "暂停" },
            { value: "achieved", label: "已达成" },
            { value: "abandoned", label: "已放弃" }
          ]
        },
        {
          name: "health",
          label: "健康度",
          type: "select",
          options: [
            { value: "unknown", label: "未知" },
            { value: "on_track", label: "正常" },
            { value: "at_risk", label: "有风险" },
            { value: "blocked", label: "受阻" }
          ]
        },
        {
          name: "priority",
          label: "优先级",
          type: "select",
          options: [
            { value: "high", label: "高" },
            { value: "normal", label: "普通" },
            { value: "low", label: "低" }
          ]
        },
        { name: "start_date", label: "开始日期", type: "date" },
        { name: "deadline", label: "截止日期", type: "date" },
        {
          name: "success_criteria",
          label: "成功标准",
          type: "textarea",
          rows: 3,
          required: true,
          help: "每行一项"
        },
        {
          name: "constraints",
          label: "约束",
          type: "textarea",
          rows: 2,
          help: "每行一项"
        },
        {
          name: "parent_goal_ids",
          label: "上级目标 ID",
          help: "多个用逗号分隔；新建目标必须填写，留空表示目标分组"
        },
        { name: "progress", label: "进度 (%)", type: "number", min: 0, max: 100 },
        { name: "next_action", label: "下一步" }
      ];
    }

    async function openGoalCreate(parentId = "") {
      const document = await loadGoalsDocument();
      let version = document.version;
      const isGroup = !parentId;
      const fields = isGroup
        ? goalFields().filter((field) => field.name !== "parent_goal_ids")
        : goalFields();
      openModal({
        title: isGroup ? "新建目标分组" : "新建目标",
        subtitle: isGroup
          ? "目标分组是顶层结构，没有上级；成功标准必填"
          : "目标必须属于一个已存在的目标；成功标准必填",
        submitLabel: isGroup ? "创建目标分组" : "创建目标",
        initial: {
          horizon: isGroup ? "years" : "months",
          status: "active",
          health: "unknown",
          priority: "normal",
          progress: 0,
          parent_goal_ids: parentId
        },
        fields,
        onSubmit: async (values, editor) => {
          const input = {
            title: values.title.trim(),
            success_criteria: splitLines(values.success_criteria),
            horizon: values.horizon,
            description: values.description,
            purpose: values.purpose,
            constraints: splitLines(values.constraints),
            start_date: values.start_date || null,
            deadline: values.deadline || null,
            status: values.status,
            health: values.health,
            priority: values.priority,
            progress: values.progress,
            next_action: values.next_action
          };
          if (isGroup) {
            input.group = true;
          } else {
            input.parent_goal_ids = splitCommas(values.parent_goal_ids);
          }
          try {
            await command("goal", "goal.create", input, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可用最新版本重试创建。");
            editor.setActions([
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "goal.create", input, version);
                  await refreshResource("goals");
                  toast(isGroup ? "目标分组已创建" : "目标已创建");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast(isGroup ? "目标分组已创建" : "目标已创建");
          return false;
        }
      });
    }

    async function openGoalEditor(id) {
      let document = await loadGoalsDocument(true);
      let goal = goalInDocument(document, id);
      if (!goal) {
        toast("目标不存在或已变化");
        return;
      }
      let baseline = goalFormValues(goal);
      let baselineGoal = goal;
      let version = document.version;
      const isGroup = !(goal.parent_goal_ids || []).length;
      openDrawer({
        title: isGroup ? "编辑目标分组" : "编辑目标",
        subtitle: goal.id,
        submitLabel: "保存目标",
        deleteLabel: isGroup ? "删除目标分组" : "删除目标",
        onDelete: async (editor) => {
          const label = isGroup ? "目标分组" : "目标";
          if (!window.confirm(`删除${label}「${goal.title}」及其所有子目标、里程碑、检查点和行动关联？行动本身会保留。`)) return;
          const latest = await loadGoalsDocument(true);
          await command("goal", "goal.delete", { goal_id: id }, latest.version);
          await refreshResource("goals");
          toast(`${label}已删除`);
          editor.close();
        },
        initial: baseline,
        fields: goalFields(),
        onSubmit: async (values, editor) => {
          const patch = goalPatchFromValues(values, baselineGoal);
          if (Object.keys(patch).length === 0) {
            editor.setInfo("没有需要保存的修改。");
            return true;
          }
          try {
            await command("goal", "goal.update", { goal_id: id, patch }, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可载入最新内容，或用最新版本重试当前修改。");
            editor.setActions([
              {
                label: "载入最新",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  const fresh = goalInDocument(latest, id);
                  if (!fresh) throw new Error("目标已不存在");
                  baselineGoal = fresh;
                  baseline = goalFormValues(fresh);
                  version = latest.version;
                  editor.setValues(baseline);
                  editor.setInfo("已载入最新内容，请确认后重新保存。");
                }
              },
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "goal.update", { goal_id: id, patch }, version);
                  await refreshResource("goals");
                  toast("目标已更新");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("目标已更新");
          return false;
        }
      });
    }

    function milestoneFields(document, selectedGoalId = "") {
      return [
        {
          name: "goal_id",
          label: "所属目标",
          type: "select",
          required: true,
          options: (document.data.goals || []).map((goal) => ({ value: goal.id, label: goal.title }))
        },
        { name: "title", label: "标题", required: true },
        { name: "start_date", label: "开始日期", type: "date" },
        { name: "due_at", label: "截止日期", type: "date" },
        {
          name: "exit_criteria",
          label: "退出标准",
          type: "textarea",
          rows: 3,
          required: true,
          help: "每行一项"
        },
        { name: "sequence", label: "顺序", type: "number", min: 1 },
        {
          name: "status",
          label: "状态",
          type: "select",
          options: [
            { value: "planned", label: "计划中" },
            { value: "active", label: "进行中" },
            { value: "blocked", label: "受阻" },
            { value: "completed", label: "已完成" },
            { value: "skipped", label: "已跳过" }
          ]
        },
        {
          name: "health",
          label: "健康度",
          type: "select",
          options: [
            { value: "unknown", label: "未知" },
            { value: "on_track", label: "正常" },
            { value: "at_risk", label: "有风险" },
            { value: "blocked", label: "受阻" }
          ]
        },
        { name: "progress", label: "进度 (%)", type: "number", min: 0, max: 100 },
        { name: "next_action", label: "下一步" },
        { name: "action_ids", label: "关联行动 ID", help: "多个用逗号分隔" }
      ];
    }

    async function openMilestoneCreate(goalId = "") {
      const document = await loadGoalsDocument();
      let version = document.version;
      const selected = goalId || (document.data.goals[0] && document.data.goals[0].id) || "";
      const nextSequence = (document.data.milestones || [])
        .filter((item) => item.goal_id === selected)
        .reduce((max, item) => Math.max(max, item.sequence || 0), 0) + 1;
      openModal({
        title: "新建里程碑",
        subtitle: "里程碑用于周/月级交付",
        submitLabel: "创建里程碑",
        initial: {
          goal_id: selected,
          status: "planned",
          health: "unknown",
          progress: 0,
          sequence: nextSequence,
          action_ids: ""
        },
        fields: milestoneFields(document, selected),
        onSubmit: async (values, editor) => {
          const input = {
            goal_id: values.goal_id,
            title: values.title.trim(),
            start_date: values.start_date || null,
            due_at: values.due_at || null,
            exit_criteria: splitLines(values.exit_criteria),
            sequence: values.sequence,
            status: values.status,
            health: values.health,
            progress: values.progress,
            next_action: values.next_action,
            action_ids: splitCommas(values.action_ids)
          };
          try {
            await command("goal", "milestone.create", input, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可用最新版本重试创建。");
            editor.setActions([
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "milestone.create", input, version);
                  await refreshResource("goals");
                  toast("里程碑已创建");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("里程碑已创建");
          return false;
        }
      });
    }

    async function openMilestoneEditor(id) {
      let document = await loadGoalsDocument(true);
      let milestone = milestoneInDocument(document, id);
      if (!milestone) {
        toast("里程碑不存在或已变化");
        return;
      }
      const baseline = {
        goal_id: milestone.goal_id,
        title: milestone.title || "",
        start_date: milestone.start_date || "",
        due_at: milestone.due_at || "",
        exit_criteria: (milestone.exit_criteria || []).join("\n"),
        sequence: milestone.sequence,
        status: milestone.status,
        health: milestone.health,
        progress: milestone.progress ?? 0,
        next_action: milestone.next_action || "",
        action_ids: (milestone.action_ids || []).join(", ")
      };
      let baselineMilestone = milestone;
      let version = document.version;
      openDrawer({
        title: "编辑里程碑",
        subtitle: milestone.id,
        submitLabel: "保存里程碑",
        deleteLabel: "删除里程碑",
        onDelete: async (editor) => {
          if (!window.confirm(`删除里程碑「${milestone.title}」及其所有检查点和行动关联？行动本身会保留。`)) return;
          const latest = await loadGoalsDocument(true);
          await command("goal", "milestone.delete", { milestone_id: id }, latest.version);
          await refreshResource("goals");
          toast("里程碑已删除");
          editor.close();
        },
        initial: baseline,
        fields: milestoneFields(document),
        onSubmit: async (values, editor) => {
          const target = {
            goal_id: values.goal_id,
            title: values.title.trim(),
            start_date: values.start_date || null,
            due_at: values.due_at || null,
            exit_criteria: splitLines(values.exit_criteria),
            sequence: values.sequence,
            status: values.status,
            health: values.health,
            progress: values.progress,
            next_action: values.next_action,
            action_ids: splitCommas(values.action_ids)
          };
          const patch = {};
          for (const key of Object.keys(target)) {
            if (JSON.stringify(target[key] ?? null) !== JSON.stringify(baselineMilestone[key] ?? null)) {
              patch[key] = target[key];
            }
          }
          if (Object.keys(patch).length === 0) {
            editor.setInfo("没有需要保存的修改。");
            return true;
          }
          try {
            await command("goal", "milestone.update", { milestone_id: id, patch }, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可载入最新内容，或用最新版本重试当前修改。");
            editor.setActions([
              {
                label: "载入最新",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  const fresh = milestoneInDocument(latest, id);
                  if (!fresh) throw new Error("里程碑已不存在");
                  baselineMilestone = fresh;
                  version = latest.version;
                  editor.setValues({
                    goal_id: fresh.goal_id,
                    title: fresh.title || "",
                    start_date: fresh.start_date || "",
                    due_at: fresh.due_at || "",
                    exit_criteria: (fresh.exit_criteria || []).join("\n"),
                    sequence: fresh.sequence,
                    status: fresh.status,
                    health: fresh.health,
                    progress: fresh.progress ?? 0,
                    next_action: fresh.next_action || "",
                    action_ids: (fresh.action_ids || []).join(", ")
                  });
                  editor.setInfo("已载入最新内容，请确认后重新保存。");
                }
              },
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "milestone.update", { milestone_id: id, patch }, version);
                  await refreshResource("goals");
                  toast("里程碑已更新");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("里程碑已更新");
          return false;
        }
      });
    }

    async function openCheckpointCreate(milestoneId = "") {
      const document = await loadGoalsDocument();
      let version = document.version;
      const selected = milestoneId || (document.data.milestones[0] && document.data.milestones[0].id) || "";
      openModal({
        title: "新建检查点",
        subtitle: "检查点挂在里程碑下，通常 1–600 分钟",
        submitLabel: "创建检查点",
        initial: { milestone_id: selected, status: "planned", estimate_minutes: 60 },
        fields: [
          {
            name: "milestone_id",
            label: "所属里程碑",
            type: "select",
            required: true,
            options: (document.data.milestones || []).map((item) => ({ value: item.id, label: item.title }))
          },
          { name: "title", label: "标题", required: true },
          { name: "description", label: "描述", type: "textarea", rows: 3 },
          {
            name: "status",
            label: "状态",
            type: "select",
            options: [
              { value: "planned", label: "计划中" },
              { value: "active", label: "进行中" },
              { value: "done", label: "已完成" },
              { value: "cancelled", label: "已取消" }
            ]
          },
          { name: "estimate_minutes", label: "预计分钟", type: "number", min: 1, max: 600 }
        ],
        onSubmit: async (values, editor) => {
          const input = {
            milestone_id: values.milestone_id,
            title: values.title.trim(),
            description: values.description,
            status: values.status,
            estimate_minutes: values.estimate_minutes
          };
          try {
            await command("goal", "checkpoint.create", input, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可用最新版本重试创建。");
            editor.setActions([
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "checkpoint.create", input, version);
                  await refreshResource("goals");
                  toast("检查点已创建");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("检查点已创建");
          return false;
        }
      });
    }

    async function openCheckpointEditor(id) {
      let document = await loadGoalsDocument(true);
      let checkpoint = checkpointInDocument(document, id);
      if (!checkpoint) {
        toast("检查点不存在或已变化");
        return;
      }
      const baseline = {
        milestone_id: checkpoint.milestone_id,
        title: checkpoint.title || "",
        description: checkpoint.description || "",
        status: checkpoint.status,
        estimate_minutes: checkpoint.estimate_minutes
      };
      let baselineCheckpoint = checkpoint;
      let version = document.version;
      openDrawer({
        title: "编辑检查点",
        subtitle: checkpoint.id,
        submitLabel: "保存检查点",
        deleteLabel: "删除检查点",
        onDelete: async (editor) => {
          if (!window.confirm(`删除检查点「${checkpoint.title}」及其所有行动关联？行动本身会保留。`)) return;
          const latest = await loadGoalsDocument(true);
          await command("goal", "checkpoint.delete", { checkpoint_id: id }, latest.version);
          await refreshResource("goals");
          toast("检查点已删除");
          editor.close();
        },
        initial: baseline,
        fields: [
          {
            name: "milestone_id",
            label: "所属里程碑",
            type: "select",
            required: true,
            options: (document.data.milestones || []).map((item) => ({ value: item.id, label: item.title }))
          },
          { name: "title", label: "标题", required: true },
          { name: "description", label: "描述", type: "textarea", rows: 3 },
          {
            name: "status",
            label: "状态",
            type: "select",
            options: [
              { value: "planned", label: "计划中" },
              { value: "active", label: "进行中" },
              { value: "done", label: "已完成" },
              { value: "cancelled", label: "已取消" }
            ]
          },
          { name: "estimate_minutes", label: "预计分钟", type: "number", min: 1, max: 600 }
        ],
        onSubmit: async (values, editor) => {
          const target = {
            milestone_id: values.milestone_id,
            title: values.title.trim(),
            description: values.description,
            status: values.status,
            estimate_minutes: values.estimate_minutes
          };
          const patch = {};
          for (const key of Object.keys(target)) {
            if (JSON.stringify(target[key] ?? null) !== JSON.stringify(baselineCheckpoint[key] ?? null)) {
              patch[key] = target[key];
            }
          }
          if (Object.keys(patch).length === 0) {
            editor.setInfo("没有需要保存的修改。");
            return true;
          }
          try {
            await command("goal", "checkpoint.update", { checkpoint_id: id, patch }, version);
          } catch (error) {
            if (error.code !== "VERSION_CONFLICT") throw error;
            editor.setError("目标集合已被其他会话更新。可载入最新内容，或用最新版本重试当前修改。");
            editor.setActions([
              {
                label: "载入最新",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  const fresh = checkpointInDocument(latest, id);
                  if (!fresh) throw new Error("检查点已不存在");
                  baselineCheckpoint = fresh;
                  version = latest.version;
                  editor.setValues({
                    milestone_id: fresh.milestone_id,
                    title: fresh.title || "",
                    description: fresh.description || "",
                    status: fresh.status,
                    estimate_minutes: fresh.estimate_minutes
                  });
                  editor.setInfo("已载入最新内容，请确认后重新保存。");
                }
              },
              {
                label: "用最新版本重试",
                onClick: async () => {
                  const latest = await loadGoalsDocument(true);
                  version = latest.version;
                  await command("goal", "checkpoint.update", { checkpoint_id: id, patch }, version);
                  await refreshResource("goals");
                  toast("检查点已更新");
                  editor.close();
                }
              }
            ]);
            return true;
          }
          await refreshResource("goals");
          toast("检查点已更新");
          return false;
        }
      });
    }

    function milestoneExpanded(milestone) {
      if (!state.milestoneSeen.has(milestone.id)) {
        state.milestoneSeen.add(milestone.id);
        if (["active", "blocked"].includes(milestone.status)) {
          state.milestoneOpen.add(milestone.id);
        }
      }
      return state.milestoneOpen.has(milestone.id);
    }

    function actionChipItems(links, actionMap) {
      return links.map((link) => {
        const action = actionMap.get(link.action_id);
        const title = action ? action.title : link.action_id;
        const status = action ? (action.archived_at ? "archived" : action.status) : "missing";
        const detail = [statusLabel(status), link.role, link.contribution].filter(Boolean).join(" · ");
        return `<button type="button" class="action-chip ${esc(status)}" data-goal-action-id="${esc(link.action_id)}" title="${esc(detail || title)}">
          <span class="action-dot"></span><span>${esc(title)}</span>
        </button>`;
      }).join("");
    }

    function actionChips(links, actionMap) {
      if (!links.length) return "";
      return `<div class="action-chips">${actionChipItems(links, actionMap)}</div>`;
    }

    function limitedActionChips(links, actionMap, key, limit = 6) {
      if (!links.length) return "";
      const shown = links.slice(0, limit);
      const rest = links.slice(limit);
      if (!rest.length) return actionChips(shown, actionMap);
      const label = `+${rest.length} 个行动`;
      return `${actionChips(shown, actionMap)}
        <button type="button" class="more-actions" data-more-actions="${esc(key)}" data-more-label="${esc(label)}">${esc(label)}</button>
        <div class="action-chips" data-more-panel="${esc(key)}" hidden>${actionChipItems(rest, actionMap)}</div>`;
    }

    function milestoneList(milestones, checkpoints, links, actionMap) {
      if (!milestones.length) {
        return `<div class="empty-inline">暂无里程碑</div>`;
      }
      const done = milestones.filter((milestone) => milestone.status === "completed").length;
      return `<div class="milestone-list">
        <div class="milestone-list-head"><span>里程碑</span><span>${done}/${milestones.length} 完成</span></div>
        ${milestones.map((milestone) => milestoneBlock(milestone, checkpoints, links, actionMap)).join("")}
      </div>`;
    }

    function checkpointActionList(links, actionMap, queueIndex) {
      if (!links.length) return "";
      const ordered = links.map((link, index) => ({
        link,
        action: actionMap.get(link.action_id),
        index
      })).sort((left, right) => {
        const leftQueue = queueIndex.get(left.link.action_id);
        const rightQueue = queueIndex.get(right.link.action_id);
        if (leftQueue !== undefined && rightQueue !== undefined) return leftQueue - rightQueue;
        if (leftQueue !== undefined) return -1;
        if (rightQueue !== undefined) return 1;
        const leftCreated = left.action && left.action.created_at ? left.action.created_at : "";
        const rightCreated = right.action && right.action.created_at ? right.action.created_at : "";
        if (leftCreated !== rightCreated) return leftCreated.localeCompare(rightCreated);
        return left.index - right.index;
      });
      const doneCount = ordered.filter(({ action }) => action && action.status === "done").length;
      const sequence = ordered.length > 1
        ? `执行顺序 ${ordered.slice(0, 6).map((_, index) => index + 1).join(" → ")}${ordered.length > 6 ? " → …" : ""}`
        : "";
      return `<div class="checkpoint-actions">
        <div class="checkpoint-actions-head">
          <span class="detail-label">行动 · ${ordered.length}</span>
          <span class="checkpoint-order-note">${doneCount}/${ordered.length} 完成${sequence ? ` · ${sequence}` : ""}</span>
        </div>
        <div class="checkpoint-action-list">${ordered.map(({ link, action }, index) => {
          const status = action ? (action.archived_at ? "archived" : action.status) : "missing";
          const checked = action && action.status === "done";
          const disabled = Boolean(state.pendingActionIds.has(link.action_id)) ||
            !action || action.archived_at || !["open", "done"].includes(action.status);
          const due = action && (action.due_at || action.due_date) ? dueLabel(action) : "";
          const meta = [
            status === "done" ? "已完成" : "",
            due,
            status === "cancelled" ? "已取消" : "",
            status === "archived" ? "已归档" : "",
            status === "missing" ? "行动缺失" : ""
          ].filter(Boolean).join(" · ");
          const details = [statusLabel(status), link.role, link.contribution, due].filter(Boolean).join(" · ");
          const budget = action ? actionBudgetChip(action) : "";
          const recurring = Boolean(action && action.recurrence && typeof action.recurrence === "object");
          const recurrence = action ? actionRecurrenceChip(action) : "";
          const canCompleteOccurrence = recurring && status === "open" &&
            !state.pendingActionIds.has(link.action_id);
          const control = recurring
            ? `<button type="button" class="mini-action recurring-complete" data-goal-action-complete="${esc(link.action_id)}" ${canCompleteOccurrence ? "" : "disabled"}>完成本次</button>`
            : `<label class="action-check">
                <input type="checkbox" data-goal-action-toggle="${esc(link.action_id)}" ${checked ? "checked" : ""} ${disabled ? "disabled" : ""} aria-label="${esc(checked ? "重开" : "完成")} ${esc(action ? action.title : link.action_id)}">
                <span class="action-check-mark" aria-hidden="true"></span>
              </label>`;
          return `<div class="checkpoint-action-row ${esc(status)} ${recurring ? "recurring" : ""}">
            <span class="action-order" aria-hidden="true">${index + 1}</span>
            ${control}
            <span class="checkpoint-action-title" title="${esc(details || link.action_id)}">${esc(action ? action.title : link.action_id)}</span>
            <span class="checkpoint-action-meta">${recurrence}${budget ? ` ${budget}` : ""}${meta ? ` · ${esc(meta)}` : ""}</span>
            <button type="button" class="mini-action checkpoint-action-edit" data-goal-action-id="${esc(link.action_id)}">编辑</button>
          </div>`;
        }).join("")}</div>
      </div>`;
    }

    function milestoneBlock(milestone, checkpoints, links, actionMap) {
      const steps = checkpoints.filter((step) => step.milestone_id === milestone.id);
      const milestoneLinks = links.filter((link) =>
        link.milestone_id === milestone.id && !link.checkpoint_id
      );
      const actionCount = milestoneLinks.length + steps.reduce((sum, step) =>
        sum + links.filter((link) => link.checkpoint_id === step.id).length, 0
      );
      const open = milestoneExpanded(milestone);
      const queueIndex = actionQueueIndex();
      const criteria = (milestone.exit_criteria || []).join(" · ");
      const checkpointMarkup = steps.length
        ? `<div class="checkpoint-list">${steps.map((step) => {
            const actions = links.filter((link) => link.checkpoint_id === step.id);
            return `<div class="checkpoint-row" data-checkpoint-id="${esc(step.id)}" tabindex="0">
              <span class="checkpoint-dot ${esc(step.status)}"></span>
              <div class="checkpoint-main">
                <div class="checkpoint-head">
                  <span class="checkpoint-title">${esc(step.title)}</span>
                  <span class="checkpoint-minutes">${esc(step.estimate_minutes ?? "—")} 分钟</span>
                  ${statusBadge(step.status)}
                  <button type="button" class="mini-action checkpoint-add-action" data-new-checkpoint-action="${esc(step.id)}" data-action-checkpoint="${esc(step.id)}">+ 行动</button>
                  <button type="button" class="mini-action" data-edit-checkpoint="${esc(step.id)}">编辑</button>
                </div>
                ${step.description ? `<div class="checkpoint-desc">${esc(step.description)}</div>` : ""}
                ${actions.length ? checkpointActionList(actions, actionMap, queueIndex) : `<div class="empty-inline">暂无关联行动</div>`}
              </div>
            </div>`;
          }).join("")}</div>`
        : `<div class="empty-inline">暂无检查点；使用“+ 检查点”补充小时级步骤。</div>`;
      return `<div class="milestone-item ${open ? "open" : ""}" data-milestone-id="${esc(milestone.id)}">
        <div class="milestone-row">
          <button type="button" class="milestone-toggle" data-milestone-toggle="${esc(milestone.id)}" aria-expanded="${open ? "true" : "false"}">
            <span class="milestone-chevron">›</span>
            <span class="milestone-summary">
              <span class="milestone-title">${esc(milestone.title)}</span>
              <span class="milestone-meta">${esc(milestone.due_at || "无截止")} · ${pct(milestone.progress)}% · ${steps.length} 检查点 · ${actionCount} 行动</span>
            </span>
            ${statusBadge(milestone.status)}
          </button>
          <div class="milestone-actions">
            <button type="button" class="mini-action" data-new-checkpoint-milestone="${esc(milestone.id)}">+ 检查点</button>
            <button type="button" class="mini-action" data-edit-milestone="${esc(milestone.id)}">编辑</button>
          </div>
        </div>
        <div class="milestone-detail" ${open ? "" : "hidden"}>
          ${criteria ? `<div class="milestone-exit">退出标准：${esc(criteria)}</div>` : ""}
          ${milestoneLinks.length ? `<div class="milestone-links"><span class="detail-label">关联行动 · ${milestoneLinks.length}</span>${limitedActionChips(milestoneLinks, actionMap, `milestone-${milestone.id}`, 6)}</div>` : ""}
          ${checkpointMarkup}
        </div>
      </div>`;
    }

    function goalHero(goal, milestones, checkpoints, links, actionMap) {
      const done = milestones.filter((milestone) => milestone.status === "completed").length;
      const stepCount = milestones.reduce((sum, milestone) =>
        sum + checkpoints.filter((step) => step.milestone_id === milestone.id).length, 0
      );
      return `<div class="goal-hero" data-goal-id="${esc(goal.id)}" tabindex="0">
        <div class="goal-hero-main">
          <div class="goal-kicker">
            <span class="badge purple">目标分组</span>
            <span>${esc(horizonLabel(goal.horizon))}</span>
            ${statusBadge(goal.status)}
            ${healthBadge(goal.health)}
            <span class="badge">${done}/${milestones.length} 里程碑</span>
            <span class="badge">${stepCount} 检查点</span>
          </div>
          <h2>${esc(goal.title)}</h2>
          ${goal.purpose || goal.description ? `<p class="goal-purpose">${esc(goal.purpose || goal.description)}</p>` : ""}
          <div class="goal-hero-meta">
            ${goal.start_date || goal.deadline ? `<span>${esc(goal.start_date || "—")} → ${esc(goal.deadline || "—")}</span>` : ""}
            ${goal.next_action ? `<span>下一步：${esc(goal.next_action)}</span>` : ""}
          </div>
        </div>
        <div class="goal-hero-actions">
          <button type="button" class="mini-action" data-new-goal-parent="${esc(goal.id)}">+ 目标</button>
          <button type="button" class="mini-action" data-edit-goal="${esc(goal.id)}">编辑分组</button>
        </div>
      </div>`;
    }

    function goalBranch(node, depth, checkpoints, links, actionMap) {
      const goal = node.goal;
      const milestones = milestonesForGoal(state.goals.data.milestones, goal.id);
      return `<div class="subgoal-block" style="margin-left:${Math.max(0, depth - 1) * 14}px">
        <div class="subgoal-head" data-goal-id="${esc(goal.id)}" tabindex="0">
          <div class="subgoal-title"><span class="badge accent">目标</span> ${esc(goal.title)}</div>
          <div class="subgoal-meta">
            <span class="badge blue">${esc(horizonLabel(goal.horizon))}</span>
            ${statusBadge(goal.status)}
            ${healthBadge(goal.health)}
            ${goal.deadline ? `<span class="badge">${esc(goal.deadline)}</span>` : ""}
            <span class="badge">${pct(goal.progress)}%</span>
          </div>
          <div class="subgoal-actions">
            <button type="button" class="mini-action" data-new-milestone-goal="${esc(goal.id)}">+ 里程碑</button>
            <button type="button" class="mini-action" data-edit-goal="${esc(goal.id)}">编辑目标</button>
          </div>
        </div>
        ${goal.purpose || goal.description ? `<div class="subgoal-purpose">${esc(goal.purpose || goal.description)}</div>` : ""}
        ${milestoneList(milestones, checkpoints, links, actionMap)}
        ${node.children.map((child) => goalBranch(child, depth + 1, checkpoints, links, actionMap)).join("")}
      </div>`;
    }

    function renderGoals() {
      const root = $("#view-goals");
      const document = state.goals && state.goals.data ? state.goals.data : {};
      const goals = document.goals || [];
      const milestones = document.milestones || [];
      const checkpoints = document.checkpoints || [];
      const links = document.action_links || [];
      const review = document.review || {};
      const actionItems = state.actions && state.actions.data && state.actions.data.items ? state.actions.data.items : [];
      const actionMap = new Map(actionItems.map((action) => [action.id, action]));

      const visible = goals.filter((goal) => !["achieved", "abandoned"].includes(goal.status));
      const goalMap = new Map(visible.map((goal) => [goal.id, goal]));
      const primaryParent = new Map();
      const children = new Map();
      for (const goal of visible) {
        const parentId = (goal.parent_goal_ids || []).find((id) => id !== goal.id && goalMap.has(id));
        if (parentId) {
          primaryParent.set(goal.id, parentId);
          if (!children.has(parentId)) children.set(parentId, []);
          children.get(parentId).push(goal);
        }
      }

      const seen = new Set();
      const buildNode = (goal) => {
        seen.add(goal.id);
        const nested = (children.get(goal.id) || [])
          .filter((child) => !seen.has(child.id))
          .map(buildNode);
        return { goal, children: nested };
      };
      const roots = visible
        .filter((goal) => !primaryParent.has(goal.id))
        .map(buildNode);
      for (const goal of visible) {
        if (!seen.has(goal.id)) roots.push(buildNode(goal));
      }
      const activeGroups = roots.filter((node) => node.goal.status === "active").length;
      const goalCount = visible.length - roots.length;
      const reviewText = review.next_review_at
        ? `下次回顾：${esc(fmtDate(review.next_review_at))} ${esc(fmtTime(review.next_review_at))}`
        : "";

      root.innerHTML = `
        <div class="hero">
          <div>
            <h2>${roots.length} 个目标分组 · ${goalCount} 个目标</h2>
            ${reviewText ? `<p>${reviewText}</p>` : ""}
          </div>
          <div class="hero-actions">
            <button id="new-root-goal" class="hero-action" type="button">+ 目标分组</button>
            <div class="badge purple">${activeGroups} 个活跃分组</div>
          </div>
        </div>
        ${ganttSection(document)}
        ${roots.length ? roots.map((node) => {
          const rootMilestones = milestonesForGoal(milestones, node.goal.id);
          const childMarkup = node.children.map((child) => goalBranch(child, 1, checkpoints, links, actionMap)).join("");
          return `<section class="goal-section">
            ${goalHero(node.goal, rootMilestones, checkpoints, links, actionMap)}
            <div class="goal-body panel">
              ${rootMilestones.length ? milestoneList(rootMilestones, checkpoints, links, actionMap) : ""}
              ${childMarkup}
              ${!rootMilestones.length && !node.children.length ? `<div class="empty-inline">还没有目标或里程碑；从上方按钮开始拆分。</div>` : ""}
            </div>
          </section>`;
        }).join("") : emptyBlock("还没有目标分组", "先创建目标分组，再补充目标、里程碑和检查点。")}
      `;

      $$("#view-goals [data-gantt-toggle]").forEach((button) => {
        if (button.disabled) return;
        button.addEventListener("click", () => {
          const goalId = button.dataset.ganttToggle;
          const gantt = $("#view-goals .gantt");
          const scrollLeft = gantt ? gantt.scrollLeft : 0;
          if (state.ganttCollapsed.has(goalId)) state.ganttCollapsed.delete(goalId);
          else state.ganttCollapsed.add(goalId);
          renderGoals();
          const nextGantt = $("#view-goals .gantt");
          if (nextGantt) nextGantt.scrollLeft = scrollLeft;
          const nextButton = $$("#view-goals [data-gantt-toggle]").find(
            (item) => item.dataset.ganttToggle === goalId
          );
          if (nextButton) nextButton.focus();
        });
      });

      $("#new-root-goal").addEventListener("click", () => openGoalCreate().catch((error) => toast(error.message)));
      $$("#view-goals [data-new-goal-parent]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openGoalCreate(button.dataset.newGoalParent).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-new-milestone-goal]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openMilestoneCreate(button.dataset.newMilestoneGoal).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-new-checkpoint-milestone]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openCheckpointCreate(button.dataset.newCheckpointMilestone).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-edit-goal]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openGoalEditor(button.dataset.editGoal).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-goal-id]").forEach((card) => {
        const open = (event) => {
          if (event.target.closest("button")) return;
          openGoalEditor(card.dataset.goalId).catch((error) => toast(error.message));
        };
        card.addEventListener("click", open);
        card.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            open(event);
          }
        });
      });
      $$("#view-goals [data-milestone-toggle]").forEach((button) => {
        button.addEventListener("click", () => {
          const item = button.closest(".milestone-item");
          const detail = item.querySelector(".milestone-detail");
          const open = button.getAttribute("aria-expanded") !== "true";
          button.setAttribute("aria-expanded", open ? "true" : "false");
          item.classList.toggle("open", open);
          detail.hidden = !open;
          state.milestoneSeen.add(button.dataset.milestoneToggle);
          if (open) state.milestoneOpen.add(button.dataset.milestoneToggle);
          else state.milestoneOpen.delete(button.dataset.milestoneToggle);
        });
      });
      $$("#view-goals [data-edit-checkpoint]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openCheckpointEditor(button.dataset.editCheckpoint).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-new-checkpoint-action]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openActionCreate({ checkpointId: button.dataset.actionCheckpoint }).catch(
            (error) => toast(error.message)
          );
        });
      });
      $$("#view-goals [data-edit-milestone]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          openMilestoneEditor(button.dataset.editMilestone).catch((error) => toast(error.message));
        });
      });
      $$("#view-goals [data-checkpoint-id]").forEach((row) => {
        const open = (event) => {
          event.stopPropagation();
          if (event.target.closest("[data-goal-action-id], input, button, label")) return;
          openCheckpointEditor(row.dataset.checkpointId).catch((error) => toast(error.message));
        };
        row.addEventListener("click", open);
        row.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            open(event);
          }
        });
      });
      $$("#view-goals [data-goal-action-toggle]").forEach((input) => {
        input.addEventListener("click", (event) => event.stopPropagation());
        input.addEventListener("change", (event) => {
          event.stopPropagation();
          const actionId = input.dataset.goalActionToggle;
          if (state.pendingActionIds.has(actionId)) return;
          state.pendingActionIds.add(actionId);
          input.disabled = true;
          toggleActionStatus(
            actionId,
            input.checked,
            () => renderGoals()
          ).catch((error) => toast(error.message)).finally(() => {
            state.pendingActionIds.delete(actionId);
            if (state.view === "goals") renderGoals();
          });
        });
      });
      $$("#view-goals [data-goal-action-complete]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          const actionId = button.dataset.goalActionComplete;
          if (state.pendingActionIds.has(actionId)) return;
          state.pendingActionIds.add(actionId);
          button.disabled = true;
          completeRecurringAction(actionId)
            .then(() => toast("已完成本次"))
            .catch((error) => toast(error.message))
            .finally(() => {
              state.pendingActionIds.delete(actionId);
              if (state.view === "goals") renderGoals();
            });
        });
      });
      $$("#view-goals [data-more-actions]").forEach((button) => {
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          const panel = button.nextElementSibling;
          if (!panel || !panel.matches("[data-more-panel]")) return;
          panel.hidden = !panel.hidden;
          button.textContent = panel.hidden ? button.dataset.moreLabel : "收起行动";
        });
      });
      $$("#view-goals [data-goal-action-id]").forEach((chip) => {
        chip.addEventListener("click", (event) => {
          event.stopPropagation();
          openActionEditor(chip.dataset.goalActionId).catch((error) => toast(error.message));
        });
      });
    }

    /* ---- gantt ---- */
    function dayOnly(value) {
      const date = parseDate(value);
      if (!date) return null;
      date.setHours(0, 0, 0, 0);
      return date;
    }

    function startOfWeek(date) {
      const day = new Date(date);
      const offset = (day.getDay() + 6) % 7;
      day.setDate(day.getDate() - offset);
      day.setHours(0, 0, 0, 0);
      return day;
    }

    function addDays(date, days) {
      const next = new Date(date);
      next.setDate(next.getDate() + days);
      return next;
    }

    function overlapsRange(startRaw, endRaw, rangeStart, rangeEnd) {
      const start = dayOnly(startRaw) || dayOnly(endRaw);
      const end = dayOnly(endRaw) || start;
      if (!start || !end) return false;
      return start <= rangeEnd && end >= rangeStart;
    }

    const GANTT_WEEK_PX = 48;

    function ganttSection(document) {
      const goals = (document.goals || []).filter((goal) => !["achieved", "abandoned"].includes(goal.status));
      const milestones = document.milestones || [];
      const dates = [];
      const rangeOf = (startRaw, endRaw) => {
        const start = dayOnly(startRaw) || dayOnly(endRaw);
        const end = dayOnly(endRaw) || start;
        if (!start || !end) return null;
        dates.push(start, end);
        return [start, end];
      };

      const goalRanges = new Map();
      for (const goal of goals) {
        const range = rangeOf(goal.start_date, goal.deadline);
        if (range) goalRanges.set(goal.id, range);
      }
      const renderableIds = new Set(goalRanges.keys());
      const primaryParent = new Map();
      const childrenByGoal = new Map();
      for (const goal of goals) {
        if (!renderableIds.has(goal.id)) continue;
        const parentId = (goal.parent_goal_ids || []).find(
          (id) => id !== goal.id && renderableIds.has(id)
        );
        if (!parentId) continue;
        primaryParent.set(goal.id, parentId);
        if (!childrenByGoal.has(parentId)) childrenByGoal.set(parentId, []);
        childrenByGoal.get(parentId).push(goal);
      }
      const milestoneRowsByGoal = new Map();
      for (const milestone of milestones) {
        if (!renderableIds.has(milestone.goal_id)) continue;
        const range = rangeOf(milestone.start_date, milestone.due_at);
        if (!range) continue;
        if (!milestoneRowsByGoal.has(milestone.goal_id)) {
          milestoneRowsByGoal.set(milestone.goal_id, []);
        }
        milestoneRowsByGoal.get(milestone.goal_id).push({
          kind: "milestone",
          title: milestone.title,
          status: milestone.status,
          sequence: milestone.sequence,
          start: range[0],
          end: range[1]
        });
      }
      for (const rows of milestoneRowsByGoal.values()) {
        rows.sort((left, right) =>
          left.start - right.start ||
          Number(left.sequence ?? 0) - Number(right.sequence ?? 0) ||
          left.title.localeCompare(right.title)
        );
      }
      const buildNode = (goal, depth) => {
        const range = goalRanges.get(goal.id);
        if (!range) return null;
        const children = (childrenByGoal.get(goal.id) || [])
          .map((child) => buildNode(child, depth + 1))
          .filter(Boolean);
        return {
          goal: {
            kind: depth === 0 ? "group" : "goal",
            id: goal.id,
            title: goal.title,
            status: goal.status,
            start: range[0],
            end: range[1],
            depth
          },
          milestones: milestoneRowsByGoal.get(goal.id) || [],
          children
        };
      };
      const roots = goals
        .filter((goal) => renderableIds.has(goal.id) && !primaryParent.has(goal.id))
        .map((goal) => buildNode(goal, 0))
        .filter(Boolean);

      if (!roots.length) {
        return `<div class="panel" style="margin-bottom:16px">
          <div class="panel-head"><h3>时间线</h3><span class="meta">暂无日期</span></div>
          ${emptyBlock("还没有可排期的目标", "为目标填写 start_date/deadline，为里程碑填写 start_date/due_at。")}
        </div>`;
      }

      const today = dayOnly(new Date());
      dates.push(today);
      const min = new Date(Math.min(...dates.map((date) => date.getTime())));
      const max = new Date(Math.max(...dates.map((date) => date.getTime())));
      const start = new Date(min.getFullYear(), min.getMonth(), 1);
      const end = new Date(max.getFullYear(), max.getMonth() + 1, 1);
      const span = Math.max(end.getTime() - start.getTime(), 1);
      const dayMs = 86_400_000;
      const days = Math.max(span / dayMs, 1);
      const chartWidth = Math.max(420, Math.ceil((days / 7) * GANTT_WEEK_PX));
      const weekPx = (7 / days) * chartWidth;
      const percent = (date) => Math.min(100, Math.max(0, ((date.getTime() - start.getTime()) / span) * 100));

      const firstTick = new Date(start);
      const weekday = (firstTick.getDay() + 6) % 7;
      firstTick.setDate(firstTick.getDate() + ((7 - weekday) % 7));
      const offsetPx = ((firstTick.getTime() - start.getTime()) / span) * chartWidth;
      const ticks = [];
      for (let cursor = firstTick; cursor < end; cursor = addDays(cursor, 7)) {
        const full = `${cursor.getFullYear()}-${pad(cursor.getMonth() + 1)}-${pad(cursor.getDate())}`;
        ticks.push(
          `<span style="left:${percent(cursor)}%" title="${full}">${pad(cursor.getMonth() + 1)}-${pad(cursor.getDate())}</span>`
        );
      }
      if (!ticks.length) {
        ticks.push(`<span style="left:0%" title="${esc(fmtDate(start))}">${pad(start.getMonth() + 1)}-${pad(start.getDate())}</span>`);
      }
      const todayMark = `<div class="gantt-today" style="left:${percent(today)}%"></div>`;

      const renderRow = (row, toggle = "", depth = 0) => {
        const milestone = row.kind === "milestone";
        const rowClass = milestone
          ? " gantt-milestone-row"
          : row.kind === "group"
            ? " gantt-group-row"
            : " gantt-goal-row";
        const paddingLeft = (milestone ? 40 : 8) + depth * 14;
        const left = percent(row.start);
        const width = Math.max(percent(row.end) - left, 0);
        const classes = ["gantt-bar"];
        if (milestone) classes.push("milestone");
        if (row.status === "completed" || row.status === "achieved") classes.push("done");
        else if (row.status === "blocked") classes.push("blocked");
        else if (row.status === "planned" || row.status === "proposed") classes.push("planned");
        // Keep this class distinct from the goals list `.milestone-row`, which adds a flex gap.
        return `<div class="gantt-row${rowClass}">
          <div class="gantt-label ${milestone ? "milestone" : ""}" style="padding-left:${paddingLeft}px" title="${esc(row.title)}">
            ${toggle}
            <span class="gantt-label-text">${esc(row.title)}</span>
          </div>
          <div class="gantt-track">
            <div class="${classes.join(" ")}" style="left:${left}%;width:${width}%" title="${esc(fmtDate(row.start))} → ${esc(fmtDate(row.end))}"></div>
            ${todayMark}
          </div>
        </div>`;
      };
      const renderNode = (node) => {
        const collapsed = state.ganttCollapsed.has(node.goal.id);
        const hasChildren = node.milestones.length > 0 || node.children.length > 0;
        const nodeLabel = node.goal.kind === "group" ? "分组" : "目标";
        const toggle = `<button type="button" class="gantt-toggle${collapsed ? "" : " open"}" data-gantt-toggle="${esc(node.goal.id)}" aria-expanded="${collapsed ? "false" : "true"}" aria-label="${collapsed ? "展开" : "折叠"}${nodeLabel}"${hasChildren ? "" : " disabled"}>›</button>`;
        let markup = renderRow(node.goal, toggle, node.goal.depth);
        if (!collapsed) {
          markup += node.milestones
            .map((row) => renderRow(row, "", node.goal.depth))
            .join("");
          markup += node.children.map(renderNode).join("");
        }
        return markup;
      };
      const body = roots.map(renderNode).join("");

      return `<div class="panel gantt-panel" style="margin-bottom:16px">
        <div class="panel-head"><h3>时间线</h3><span class="meta">${esc(fmtDate(start))} → ${esc(fmtDate(new Date(end.getTime() - 86400000)))}</span></div>
        <div class="gantt" style="--gantt-width:${chartWidth}px;--gantt-week:${weekPx.toFixed(2)}px;--gantt-offset:${offsetPx.toFixed(2)}px">
          <div class="gantt-head">
            <div class="gantt-corner"></div>
            <div class="gantt-axis">${ticks.join("")}</div>
          </div>
          ${body}
        </div>
      </div>`;
    }

    /* ---- goal calendar ---- */
    function goalsDocument() {
      return state.goals && state.goals.data ? state.goals.data : {};
    }

    function calendarLevelLabel(level) {
      return CALENDAR_LEVELS.find((item) => item.key === level)?.label || level;
    }

    function calendarItemKey(item) {
      return `${item.level}:${item.id}`;
    }

    function calendarLevelSummary(items) {
      if (!items.length) return "0 个层级项";
      return CALENDAR_LEVELS.map(({ key, label }) => {
        const count = items.filter((item) => item.level === key).length;
        return count ? `${count} 个${label}` : "";
      }).filter(Boolean).join(" · ");
    }

    function calendarToolbar() {
      const modes = [["list", "层级列表"], ["week", "周日历"], ["month", "月日历"], ["year", "年日历"]];
      const chips = modes
        .map(([mode, label]) => `<button class="chip ${state.calendarMode === mode ? "active" : ""}" data-calendar-mode="${mode}">${label}</button>`)
        .join("");
      const levels = CALENDAR_LEVELS.map(({ key, label }) => {
        const active = state.calendarLevels[state.calendarMode].has(key);
        return `<button class="chip ${active ? "active" : ""}" data-calendar-level="${key}" aria-pressed="${active}">${label}</button>`;
      }).join("");
      const nav = state.calendarMode === "list"
        ? ""
        : `<span class="faint" style="margin-left:auto">${esc(calendarPeriodLabel())}</span>
           <button class="chip" data-calendar-nav="-1">←</button>
           <button class="chip" data-calendar-nav="0">今天</button>
           <button class="chip" data-calendar-nav="1">→</button>`;
      return `<div class="filter-bar calendar-toolbar">${chips}${nav}</div>
        <div class="filter-bar calendar-levels">${levels}</div>`;
    }

    function calendarHierarchyItems() {
      const document = goalsDocument();
      const goals = (document.goals || []).filter(
        (goal) => !["achieved", "abandoned"].includes(goal.status)
      );
      const milestones = document.milestones || [];
      const checkpoints = document.checkpoints || [];
      const links = document.action_links || [];
      const actions =
        state.actions && state.actions.data && Array.isArray(state.actions.data.items)
          ? state.actions.data.items
          : [];
      const goalById = new Map(goals.map((goal) => [goal.id, goal]));
      const milestoneById = new Map(milestones.map((milestone) => [milestone.id, milestone]));
      const checkpointById = new Map(checkpoints.map((checkpoint) => [checkpoint.id, checkpoint]));
      const linkByAction = new Map();
      for (const link of links) {
        if (link.action_id && !linkByAction.has(link.action_id)) {
          linkByAction.set(link.action_id, link);
        }
      }

      const items = [];
      const push = (
        level,
        record,
        startRaw,
        endRaw,
        {
          depth = 0,
          goalId = null,
          milestoneId = null,
          checkpointId = null,
          dueAt = null,
          durationMinutes = null,
          bufferMinutes = null
        } = {}
      ) => {
        const start = dayOnly(startRaw) || dayOnly(endRaw);
        const end = dayOnly(endRaw) || start;
        if (!start || !end) return;
        items.push({
          level,
          id: record.id,
          title: record.title || "",
          status: record.status,
          progress: record.progress,
          start,
          end,
          start_date: fmtDateKey(start),
          deadline: fmtDateKey(end),
          depth,
          goal_id: goalId || record.goal_id || null,
          milestone_id: milestoneId || record.milestone_id || null,
          checkpoint_id: checkpointId,
          due_at: dueAt,
          duration_minutes: durationMinutes,
          buffer_minutes: bufferMinutes
        });
      };

      for (const goal of goals) {
        const isGroup = isGoalGroup(goal);
        push(
          isGroup ? "groups" : "goals",
          goal,
          goal.start_date || goal.created_at || goal.deadline,
          goal.deadline || goal.next_review_at || goal.start_date,
          { depth: isGroup ? 0 : 1 }
        );
      }
      for (const milestone of milestones) {
        push(
          "milestones",
          milestone,
          milestone.start_date || milestone.due_at,
          milestone.due_at || milestone.start_date,
          { depth: 2, goalId: milestone.goal_id }
        );
      }
      for (const checkpoint of checkpoints) {
        const milestone = milestoneById.get(checkpoint.milestone_id);
        if (!milestone) continue;
        push(
          "checkpoints",
          checkpoint,
          milestone.start_date || milestone.due_at,
          milestone.due_at || milestone.start_date,
          { depth: 3, goalId: milestone.goal_id, milestoneId: milestone.id }
        );
      }
      for (const action of actions) {
        if (action.archived_at) continue;
        const due = action.due_date || action.due_at;
        if (!due) continue;
        const link = linkByAction.get(action.id);
        let depth = 4;
        let goalId = null;
        let milestoneId = null;
        let checkpointId = null;
        if (link) {
          checkpointId = link.checkpoint_id || null;
          milestoneId = link.milestone_id || null;
          if (checkpointId && !milestoneId) {
            milestoneId = checkpointById.get(checkpointId)?.milestone_id || null;
          }
          if (milestoneId) {
            goalId = milestoneById.get(milestoneId)?.goal_id || null;
            depth = 3;
          } else {
            const goalIds = Array.isArray(link.goal_ids)
              ? link.goal_ids
              : (link.goal_id ? [link.goal_id] : []);
            goalId = goalIds[0] || null;
            const goal = goalId ? goalById.get(goalId) : null;
            depth = goal && isGoalGroup(goal) ? 1 : 2;
            if (checkpointId) depth = 3;
          }
        }
        push("actions", action, due, due, {
          depth,
          goalId,
          milestoneId,
          checkpointId,
          dueAt: action.due_at || null,
          durationMinutes: action.duration_minutes ?? null,
          bufferMinutes: action.buffer_minutes ?? null
        });
      }
      return items;
    }

    function enabledCalendarItems() {
      const enabled = state.calendarLevels[state.calendarMode] || new Set();
      return calendarHierarchyItems().filter((item) => enabled.has(item.level));
    }

    function openCalendarItem(kind, id) {
      if (kind === "actions") return openActionEditor(id);
      if (kind === "checkpoints") return openCheckpointEditor(id);
      if (kind === "milestones") return openMilestoneEditor(id);
      return openGoalEditor(id);
    }

    function calendarPeriodLabel() {
      const anchor = parseDate(state.calendarAnchor) || new Date();
      if (state.calendarMode === "week") {
        const start = startOfWeek(anchor);
        const end = addDays(start, 6);
        return `${fmtDate(start)} → ${fmtDate(end)}`;
      }
      if (state.calendarMode === "month") {
        return `${anchor.getFullYear()}年${anchor.getMonth() + 1}月`;
      }
      return `${anchor.getFullYear()}年`;
    }

    function shiftCalendar(delta) {
      const anchor = parseDate(state.calendarAnchor) || new Date();
      if (delta === 0) {
        state.calendarAnchor = fmtDateKey(new Date());
        return;
      }
      if (state.calendarMode === "week") {
        anchor.setDate(anchor.getDate() + 7 * delta);
      } else if (state.calendarMode === "month") {
        anchor.setDate(1);
        anchor.setMonth(anchor.getMonth() + delta);
      } else {
        anchor.setDate(1);
        anchor.setMonth(0);
        anchor.setFullYear(anchor.getFullYear() + delta);
      }
      state.calendarAnchor = fmtDateKey(anchor);
    }

    function bindCalendarControls() {
      $$("#view-calendar [data-calendar-mode]").forEach((button) => {
        button.addEventListener("click", () => {
          state.calendarMode = button.dataset.calendarMode;
          renderCalendar();
        });
      });
      $$("#view-calendar [data-calendar-level]").forEach((button) => {
        button.addEventListener("click", () => {
          const levels = state.calendarLevels[state.calendarMode];
          const level = button.dataset.calendarLevel;
          if (levels.has(level)) levels.delete(level);
          else levels.add(level);
          renderCalendar();
        });
      });
      $$("#view-calendar [data-calendar-nav]").forEach((button) => {
        button.addEventListener("click", () => {
          shiftCalendar(Number(button.dataset.calendarNav));
          renderCalendar();
        });
      });
      $$("#view-calendar [data-calendar-item-kind]").forEach((element) => {
        const open = (event) => {
          event.stopPropagation();
          openCalendarItem(
            element.dataset.calendarItemKind,
            element.dataset.calendarItemId
          ).catch((error) => toast(error.message));
        };
        element.addEventListener("click", open);
        element.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") open(event);
        });
      });

    }

    function coversDay(startRaw, endRaw, day) {
      const start = dayOnly(startRaw) || dayOnly(endRaw);
      const end = dayOnly(endRaw) || start;
      return Boolean(start && end && day >= start && day <= end);
    }

    function periodGoalList(items, emptyText, toneOf = () => 0) {
      if (!items.length) {
        return `<div class="faint" style="padding:10px 0">${esc(emptyText)}</div>`;
      }
      return `<div class="list" style="margin-top:8px">${items.map((item) => {
        const end = item.deadline || item.due_at;
        const range = item.start_date && end
          ? `${item.start_date} → ${end}`
          : (item.start_date || end || "");
        const review = !range && item.review_at ? `回顾 ${item.review_at}` : "";
        const dueTime = item.level === "actions" && item.due_at ? fmtTime(item.due_at) : "";
        const meta = [
          calendarLevelLabel(item.level),
          range || review,
          dueTime,
          item.progress != null ? `${item.progress}%` : ""
        ].filter(Boolean).join(" · ");
        const tone = (toneOf(item) % 6) + 1;
        const indent = Math.min(4, Math.max(0, item.depth || 0)) * 8;
        const budget = item.level === "actions" ? actionBudgetChip(item) : "";
        return `<div class="list-item" style="padding-left:${indent}px">
          <span class="cal-legend-swatch" style="background:var(--c${tone})"></span>
          <div class="list-body">
            <div class="list-title"><span class="legend-index">${toneOf(item) + 1}</span>${esc(item.title)}</div>
            <div class="list-sub">${esc(meta || "—")}${budget ? ` · ${budget}` : ""}</div>
          </div>
          ${statusBadge(item.status)}
        </div>`;
      }).join("")}</div>`;
    }

    function renderGoalCalendar() {
      if (state.calendarMode === "week") return renderWeekGoals();
      if (state.calendarMode === "month") return renderMonthGoals();
      return renderYearGoals();
    }

    function renderWeekGoals() {
      const anchor = parseDate(state.calendarAnchor) || new Date();
      const start = startOfWeek(anchor);
      const days = Array.from({ length: 7 }, (_, index) => addDays(start, index));
      const end = days[6];
      const today = dayOnly(new Date());
      const active = enabledCalendarItems()
        .filter((item) => overlapsRange(item.start_date, item.deadline, start, end))
        .sort((left, right) => left.start - right.start || left.depth - right.depth);
      const toneIndex = new Map(active.map((item, index) => [calendarItemKey(item), index]));
      const hourHeight = 30;
      const height = 24 * hourHeight;

      // All-day lane: selected hierarchy items plus all-day calendar events.
      const allDay = [];
      for (const item of active) {
        // Timed actions are rendered as duration + elastic blocks on the grid.
        if (item.level === "actions" && item.due_at) continue;
        allDay.push({
          title: item.title,
          start: item.start < start ? start : item.start,
          end: item.end > end ? end : item.end,
          kind: "hierarchy",
          level: item.level,
          id: item.id,
          tone: toneIndex.get(calendarItemKey(item)) ?? 0,
          status: item.status,
          progress: item.progress,
          meta: `${item.start_date} → ${item.deadline}${item.due_at ? ` · ${fmtTime(item.due_at)}` : ""}${
            item.level === "actions" && item.duration_minutes != null
              ? ` · ${formatMinutes(item.duration_minutes)} + 弹性 ${formatMinutes(item.buffer_minutes ?? 0)}`
              : ""
          }`
        });
      }
      allDay.sort((left, right) => left.start - right.start || left.end - right.end);
      const lanes = [];
      for (const item of allDay) {
        let lane = lanes.findIndex((last) => last < item.start);
        if (lane === -1) {
          lane = lanes.length;
          lanes.push(item.end);
        } else {
          lanes[lane] = item.end;
        }
        item.lane = lane;
      }
      const alldayHeight = Math.max(lanes.length, 1) * 22 + 4;
      const alldayBars = allDay.map((item) => {
        const leftIndex = Math.round((item.start - start) / 86400000);
        const rightIndex = Math.round((item.end - start) / 86400000);
        const left = (leftIndex / 7) * 100;
        const width = ((rightIndex - leftIndex + 1) / 7) * 100;
        const tone = (item.tone % 6) + 1;
        const color = item.kind === "event"
          ? ""
          : `--tone:var(--c${tone});--tone-soft:var(--c${tone}-soft);`;
        const detail = item.kind === "event"
          ? `全天事件 · ${statusLabel(item.status || "confirmed")}`
          : `${calendarLevelLabel(item.level)} · ${statusLabel(item.status)}${item.progress != null ? ` · ${item.progress}%` : ""}`;
        const tip = tipAttrs(item.title, item.meta, detail);
        const attrs = item.kind === "event"
          ? ` data-event-id="${esc(item.id)}" role="button" tabindex="0"`
          : ` data-calendar-item-kind="${esc(item.level)}" data-calendar-item-id="${esc(item.id)}" role="button" tabindex="0"`;
        return `<div class="week-allday-bar ${item.kind === "event" ? "event" : "hierarchy"}" style="left:${left}%;width:${width}%;top:${2 + item.lane * 22}px;${color}"${tip}${attrs}>${esc(item.title)}</div>`;
      }).join("");

      // Timed actions are placed on the 24-hour grid; untimed actions stay in the all-day lane.
      const dayEvents = days.map(() => []);
      for (const item of active) {
        if (item.level !== "actions" || !item.due_at) continue;
        const due = parseDate(item.due_at);
        if (!due) continue;
        const dayIndex = Math.round((dayOnly(due) - start) / 86400000);
        if (dayIndex < 0 || dayIndex > 6) continue;
        const startMin = due.getHours() * 60 + due.getMinutes();
        const duration = Math.max(1, Number(item.duration_minutes ?? 30));
        const buffer = Math.max(0, Number(item.buffer_minutes ?? duration));
        const total = duration + buffer;
        const endMin = Math.min(1440, startMin + total);
        if (endMin <= startMin) continue;
        dayEvents[dayIndex].push({
          kind: "action-budget",
          id: item.id,
          title: item.title || "行动",
          status: item.status || "open",
          startMin,
          endMin,
          durationMin: duration,
          bufferMin: buffer,
          totalMin: total,
          due_at: item.due_at
        });
      }

      const hourLabels = Array.from({ length: 24 }, (_, hour) => {
        return `<div class="week-hour-label" style="top:${hour * hourHeight}px">${pad(hour)}:00</div>`;
      }).join("");
      const headers = days.map((day) => {
        const label = `周${["日", "一", "二", "三", "四", "五", "六"][day.getDay()]} ${day.getMonth() + 1}/${day.getDate()}`;
        const isToday = today.getTime() === day.getTime();
        return `<div class="week-dayhead ${isToday ? "today" : ""}">${esc(label)}</div>`;
      }).join("");
      const now = new Date();
      const nowMinutes = now.getHours() * 60 + now.getMinutes();
      const columns = days.map((day, dayIndex) => {
        const events = dayEvents[dayIndex].sort((left, right) => left.startMin - right.startMin || left.endMin - right.endMin);
        // Blocks have a minimum rendered height; extend their layout span so a
        // very short event cannot visually overlap the next one.
        const minMinutes = Math.ceil((16 / height) * 1440);
        for (const event of events) {
          event.layoutEnd = Math.max(
            event.endMin + (event.bufferMin || 0),
            event.startMin + minMinutes
          );
        }
        const clusters = [];
        let cluster = null;
        for (const event of events) {
          if (!cluster || event.startMin >= cluster.end) {
            cluster = { end: event.layoutEnd, items: [] };
            clusters.push(cluster);
          }
          cluster.items.push(event);
          cluster.end = Math.max(cluster.end, event.layoutEnd);
        }
        for (const group of clusters) {
          const laneEnds = [];
          for (const event of group.items) {
            let lane = laneEnds.findIndex((last) => last <= event.startMin);
            if (lane === -1) {
              lane = laneEnds.length;
              laneEnds.push(event.layoutEnd);
            } else {
              laneEnds[lane] = event.layoutEnd;
            }
            event.lane = lane;
          }
          const laneCount = Math.max(laneEnds.length, 1);
          for (const event of group.items) {
            event.laneCount = laneCount;
          }
        }
        const blocks = events.map((event) => {
          const widthPercent = 100 / (event.laneCount || 1);
          const leftPercent = event.lane * widthPercent;
          if (event.kind === "action-budget") {
            const total = Math.max(Number(event.totalMin) || 1, 1);
            const durationShare = Math.min(
              100,
              Math.max(0, ((Number(event.durationMin) || 0) / total) * 100)
            );
            const flexShare = 100 - durationShare;
            const top = (event.startMin / 1440) * height + 1;
            const blockHeight = Math.max(((event.endMin - event.startMin) / 1440) * height - 2, 16);
            const meta = `${fmtTime(event.due_at)} · ${formatMinutes(event.durationMin)} + 弹性 ${formatMinutes(event.bufferMin)}`;
            const tip = tipAttrs(event.title, meta, `行动 · ${statusLabel(event.status)}`);
            const flexLabel = (blockHeight * flexShare) / 100 >= 14 ? "弹性" : "";
            const flex = flexShare > 0
              ? `<span class="week-budget-flex" style="height:${flexShare}%">${flexLabel}</span>`
              : "";
            return `<div class="week-event week-budget ${esc(event.status)}" data-calendar-item-kind="actions" data-calendar-item-id="${esc(event.id)}" role="button" tabindex="0" style="top:${top}px;height:${blockHeight}px;left:calc(${leftPercent}% + 1px);width:calc(${widthPercent}% - 3px)"${tip}>
              <span class="week-budget-duration" style="height:${durationShare}%">${esc(event.title)}</span>
              ${flex}
            </div>`;
          }
          return "";
        }).join("");
        const isToday = today.getTime() === day.getTime();
        const nowLine = isToday ? `<div class="week-now" style="top:${(nowMinutes / 1440) * height}px"></div>` : "";
        return `<div class="week-col ${isToday ? "today" : ""}" data-week-day="${fmtDateKey(day)}" style="height:${height}px">${nowLine}${blocks}</div>`;
      }).join("");

      return `<div class="panel">
        <div class="panel-head"><h3>本周</h3></div>
        <div class="calendar-wrap"><div class="week-grid">
          <div class="week-corner"></div>
          ${headers}
          <div class="week-allday-label">全天</div>
          <div class="week-allday" style="height:${alldayHeight}px">${alldayBars}</div>
          <div class="week-hours" style="height:${height}px">${hourLabels}</div>
          ${columns}
        </div></div>
        ${periodGoalList(active, "本周没有匹配的层级项", (item) => toneIndex.get(calendarItemKey(item)) ?? 0)}
      </div>`;
    }

    function renderMonthGoals() {
      const anchor = parseDate(state.calendarAnchor) || new Date();
      const first = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
      const daysInMonth = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 0).getDate();
      const last = new Date(anchor.getFullYear(), anchor.getMonth(), daysInMonth);
      const today = dayOnly(new Date());
      const active = enabledCalendarItems()
        .filter((item) => overlapsRange(item.start_date, item.deadline, first, last))
        .sort((left, right) => left.start - right.start || left.depth - right.depth);
      const offset = (first.getDay() + 6) % 7;
      const toneIndex = new Map(active.map((item, index) => [calendarItemKey(item), index]));
      const headers = ["一", "二", "三", "四", "五", "六", "日"]
        .map((label) => `<div class="month-head">周${label}</div>`)
        .join("");
      const chipsPerDay = 3;
      const cells = [];
      for (let index = 0; index < offset; index += 1) {
        cells.push('<div class="month-cell"></div>');
      }
      for (let day = 1; day <= daysInMonth; day += 1) {
        const date = new Date(anchor.getFullYear(), anchor.getMonth(), day);
        const dayItems = active
          .filter((item) => coversDay(item.start_date, item.deadline, date))
          .sort((left, right) => (toneIndex.get(calendarItemKey(left)) ?? 0) - (toneIndex.get(calendarItemKey(right)) ?? 0));
        const entries = dayItems.slice(0, chipsPerDay).map((item) => {
          const index = toneIndex.get(calendarItemKey(item)) ?? 0;
          const tone = (index % 6) + 1;
          const meta = `${item.start_date} → ${item.deadline}${item.due_at ? ` · ${fmtTime(item.due_at)}` : ""}`;
          const detail = `${calendarLevelLabel(item.level)} · ${statusLabel(item.status)}${item.progress != null ? ` · ${item.progress}%` : ""}`;
          const tip = tipAttrs(item.title, meta, detail);
          return `<span class="month-event" data-calendar-item-kind="${esc(item.level)}" data-calendar-item-id="${esc(item.id)}" role="button" tabindex="0" style="--tone:var(--c${tone});--tone-soft:var(--c${tone}-soft);"${tip}>${esc(item.title)}</span>`;
        });
        const totalEntries = dayItems.length;
        const more = totalEntries > entries.length
          ? `<div class="month-more">+${totalEntries - entries.length} 更多</div>`
          : "";
        const isToday = today.getFullYear() === anchor.getFullYear() && today.getMonth() === anchor.getMonth() && today.getDate() === day;
        const cellTip = totalEntries
          ? tipAttrs(
              `${anchor.getMonth() + 1}月${day}日`,
              calendarLevelSummary(dayItems),
              dayItems.map((item) => item.title).join("、")
            )
          : "";
        cells.push(`<div class="month-cell ${isToday ? "today" : ""}" data-month-day="${fmtDateKey(date)}"${cellTip}>
          <div class="month-day">${day}</div>
          ${entries.join("")}${more}
        </div>`);
      }
      return `<div class="panel">
        <div class="panel-head"><h3>本月</h3></div>
        <div class="calendar-wrap"><div class="month-grid">${headers}${cells.join("")}</div></div>
        ${periodGoalList(active, "本月没有匹配的层级项", (item) => toneIndex.get(calendarItemKey(item)) ?? 0)}
      </div>`;
    }

    function goalYearRange(goal) {
      const start = dayOnly(goal.start_date) || dayOnly(goal.created_at) || dayOnly(goal.deadline);
      const end = dayOnly(goal.deadline) || dayOnly(goal.next_review_at) || start;
      if (!start && !end) return null;
      const first = start || end;
      const last = end || start;
      return [first, last >= first ? last : first];
    }

    function goalOverlapsYear(goal, year) {
      const range = goalYearRange(goal);
      if (!range) return false;
      const yearStart = new Date(year, 0, 1);
      const yearEnd = new Date(year, 11, 31);
      return range[0] <= yearEnd && range[1] >= yearStart;
    }

    function goalCoversDay(goal, date) {
      const range = goalYearRange(goal);
      return Boolean(range && date >= range[0] && date <= range[1]);
    }

    function renderYearGoals() {
      const anchor = parseDate(state.calendarAnchor) || new Date();
      const year = anchor.getFullYear();
      const today = dayOnly(new Date());
      const active = enabledCalendarItems()
        .filter((item) => goalOverlapsYear(item, year))
        .sort((left, right) => left.start - right.start || left.depth - right.depth);
      const toneIndex = new Map(active.map((item, index) => [calendarItemKey(item), index]));
      const weekdays = ["一", "二", "三", "四", "五", "六", "日"];
      const months = Array.from({ length: 12 }, (_, month) => {
        const first = new Date(year, month, 1);
        const daysInMonth = new Date(year, month + 1, 0).getDate();
        const offset = (first.getDay() + 6) % 7;
        const cells = [];
        for (let index = 0; index < offset; index += 1) {
          cells.push('<div class="year-day blank"></div>');
        }
        let coveredDays = 0;
        for (let day = 1; day <= daysInMonth; day += 1) {
          const date = new Date(year, month, day);
          const dayItems = active.filter((item) => goalCoversDay(item, date));
          if (dayItems.length) coveredDays += 1;
          const dots = dayItems.slice(0, 3).map((item) => {
            const tone = ((toneIndex.get(calendarItemKey(item)) ?? 0) % 6) + 1;
            const range = goalYearRange(item);
            const meta = range ? `${fmtDate(range[0])} → ${fmtDate(range[1])}` : "";
            const detail = `${calendarLevelLabel(item.level)} · ${statusLabel(item.status)}${item.progress != null ? ` · ${item.progress}%` : ""}`;
            const tip = tipAttrs(item.title, meta, detail);
            return `<span class="year-dot" data-calendar-item-kind="${esc(item.level)}" data-calendar-item-id="${esc(item.id)}" style="background:var(--c${tone})"${tip}></span>`;
          }).join("");
          const isToday = today.getFullYear() === year && today.getMonth() === month && today.getDate() === day;
          const cellTip = dayItems.length
            ? tipAttrs(
                `${month + 1}月${day}日`,
                calendarLevelSummary(dayItems),
                dayItems.map((item) => item.title).join("、")
              )
            : "";
          cells.push(`<div class="year-day ${isToday ? "today" : ""}"${cellTip}>${day}${dots ? `<span class="year-dots">${dots}</span>` : ""}</div>`);
        }
        while (cells.length % 7 !== 0) {
          cells.push('<div class="year-day blank"></div>');
        }
        const countLabel = coveredDays > 0 ? `${coveredDays} 天` : "";
        return `<div class="year-month">
          <div class="year-month-head"><span>${month + 1} 月</span><span class="faint">${countLabel}</span></div>
          <div class="year-weekdays">${weekdays.map((label) => `<span>${label}</span>`).join("")}</div>
          <div class="year-days">${cells.join("")}</div>
        </div>`;
      }).join("");
      return `<div class="panel">
        <div class="panel-head"><h3>${year} 年</h3></div>
        <div class="year-grid">${months}</div>
        ${periodGoalList(active, "本年没有匹配的层级项", (item) => toneIndex.get(calendarItemKey(item)) ?? 0)}
      </div>`;
    }

    /* ---- calendar ---- */






























    function renderCalendar() {
      const root = $("#view-calendar");
      if (state.calendarMode !== "list") {
        root.innerHTML = `${calendarToolbar()}${renderGoalCalendar()}`;
        bindCalendarControls();
        return;
      }
      const anchor = dayOnly(parseDate(state.calendarAnchor) || new Date());
      const rangeEnd = addDays(anchor, 14);
      const hierarchy = enabledCalendarItems()
        .filter((item) =>
          overlapsRange(item.start_date, item.deadline, anchor, rangeEnd)
        )
        .sort((left, right) => left.start - right.start || left.depth - right.depth);
      if (!hierarchy.length) {
        root.innerHTML = `
          ${calendarToolbar()}
          <div class="hero">
            <div>
              <h2>未来 14 天</h2>
            </div>
            <div class="badge accent">0 个安排</div>
          </div>
          ${emptyBlock("没有安排", "创建行动或调整层级筛选后再查看。")}
        `;
        bindCalendarControls();
        return;
      }
      const groups = new Map();
      for (const item of hierarchy) {
        const start = item.start < anchor ? anchor : item.start;
        const key = fmtDateKey(start);
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(item);
      }
      root.innerHTML = `
        ${calendarToolbar()}
        <div class="hero">
          <div>
            <h2>未来 14 天 · ${hierarchy.length} 个安排</h2>
            <p>${fmtDateKey(anchor)} → ${fmtDateKey(rangeEnd)}</p>
          </div>
          <div class="badge accent">${groups.size} 天有安排</div>
        </div>
        <div class="panel">
          ${Array.from(groups.entries())
            .sort(([left], [right]) => left.localeCompare(right))
            .map(([key, dayItems]) => `
            <div class="day-group">
              <div class="day-head"><strong>${esc(fmtDateFull(key))}</strong><span>${esc(dayLabel(key))}</span></div>
              ${dayItems.map((item) => hierarchyRow(item)).join("")}
            </div>
          `).join("")}
        </div>
      `;
      bindCalendarControls();
    }

    function hierarchyRow(item) {
      const level = calendarLevelLabel(item.level);
      const range = item.start_date === item.deadline
        ? item.start_date
        : `${item.start_date} → ${item.deadline}`;
      const meta = [
        range,
        item.due_at ? fmtTime(item.due_at) : "",
        item.progress != null ? `${item.progress}%` : ""
      ].filter(Boolean).join(" · ");
      const levelIndex = CALENDAR_LEVELS.findIndex((entry) => entry.key === item.level);
      const tone = ((levelIndex < 0 ? 0 : levelIndex) % 6) + 1;
      const budget = item.level === "actions" ? actionBudgetChip(item) : "";
      return `<div class="event-row editable-row hierarchy-row" data-calendar-item-kind="${esc(item.level)}" data-calendar-item-id="${esc(item.id)}" tabindex="0">
        <div class="event-time">${esc(level)}</div>
        <div class="dot" style="background:var(--c${tone})"></div>
        <div class="event-body">
          <div class="event-title">${esc(item.title)}</div>
          <div class="event-sub">${esc(meta || "—")}${budget ? ` · ${budget}` : ""}</div>
        </div>
        ${statusBadge(item.status)}
      </div>`;
    }



    /* ---- health ---- */
    function renderHealth() {
      const root = $("#view-health");
      const summary = state.summary || {};
      const health = summary.health || {};
      const water = health.water || {};
      const sleep = health.sleep || {};
      const body = health.body;
      const document = state.health && state.health.data ? state.health.data : {};
      const records = document.body && document.body.records ? document.body.records.slice().reverse().slice(0, 10) : [];
      const waterSeries = water.series || [];
      const sleepSeries = sleep.series || [];

      root.innerHTML = `
        <div class="hero">
          <div>
            <h2>身体状态与恢复</h2>
            <p>最近 14 天</p>
          </div>
          <div class="badge blue">${body && body.weight_kg ? `${esc(body.weight_kg)} kg` : "暂无体重"}</div>
        </div>
        <div class="grid-2 even">
          <div class="panel">
            <div class="panel-head"><h3>今日饮水</h3><span class="meta">${esc(water.today_ml ?? 0)} / ${esc(water.goal_ml ?? 2000)} ml</span></div>
            <div class="metric">
              <div class="ring">
                <div><strong>${pct(Math.round((water.ratio || 0) * 100))}%</strong><span>目标</span></div>
              </div>
              <div class="grow">
                <h4>${esc(water.today_ml ?? 0)} ml</h4>
                <p>每日目标 ${esc(water.goal_ml ?? 2000)} ml</p>
              </div>
            </div>
          </div>
          <div class="panel">
            <div class="panel-head"><h3>昨夜睡眠</h3><span class="meta">${sleep.last_night_date ? esc(sleep.last_night_date) : "暂无记录"}</span></div>
            <div class="metric">
              <div class="ring">
                <div><strong>${fmtMinutes(sleep.last_night_minutes)}</strong><span>睡眠</span></div>
              </div>
              <div class="grow">
                <h4>${sleep.quality ? `质量 ${esc(sleep.quality)}/5` : "尚无质量评分"}</h4>
                <p>${sleep.goal_minutes ? `目标 ${fmtMinutes(sleep.goal_minutes)}` : "尚未设置睡眠目标"}</p>
              </div>
            </div>
          </div>
        </div>
        <div class="grid-2 even">
          <div class="panel">
            <div class="panel-head"><h3>饮水 · 14 天</h3><span class="meta">ml</span></div>
            ${healthBars(waterSeries, "ml", "water")}
          </div>
          <div class="panel">
            <div class="panel-head"><h3>睡眠 · 14 天</h3><span class="meta">分钟</span></div>
            ${healthBars(sleepSeries, "minutes", "sleep")}
          </div>
        </div>
        <div class="panel" style="margin-top:16px">
          <div class="panel-head"><h3>身体记录</h3><span class="meta">最近 ${records.length} 条</span></div>
          ${records.length ? bodyTable(records) : emptyBlock("还没有身体记录", "使用 health.body.log 记录体重、心率或精力。")}
        </div>
        ${body ? `<div class="panel" style="margin-top:16px">
          <div class="panel-head"><h3>基础指标</h3><span class="meta">${body.date ? esc(body.date) : ""}</span></div>
          <div class="filter-bar">
            <span class="badge">身高 ${esc(body.height_cm ?? "—")} cm</span>
            <span class="badge">目标体重 ${esc(body.target_weight_kg ?? "—")} kg</span>
            ${body.body_fat_pct ? `<span class="badge">体脂 ${esc(body.body_fat_pct)}%</span>` : ""}
            ${body.heart_rate_bpm ? `<span class="badge">静息心率 ${esc(body.heart_rate_bpm)}</span>` : ""}
            ${body.systolic_bp ? `<span class="badge">血压 ${esc(body.systolic_bp)}/${esc(body.diastolic_bp ?? "—")}</span>` : ""}
          </div>
        </div>` : ""}
      `;
    }

    function healthBars(series, field, kind) {
      if (!series || !series.length) return emptyBlock("暂无数据", "还没有足够的记录。");
      const values = series.map((item) => Number(item[field]) || 0);
      if (values.every((value) => value === 0)) {
        return emptyBlock("暂无数据", "最近 14 天还没有记录。");
      }
      const max = Math.max(...values, kind === "water" ? 2000 : 480);
      return `<div class="chart">${series.map((item) => {
        const value = Number(item[field]) || 0;
        const height = value ? Math.max(4, Math.round((value / max) * 88)) : 2;
        const date = parseDate(item.date);
        const label = date ? String(date.getDate()) : "";
        return `<div class="bar ${kind} ${value ? "" : "no-data"}" title="${esc(item.date)} · ${esc(value)} ${kind === "water" ? "ml" : "min"}">
          <div class="fill" style="height:${height}px"></div>
          <div class="tick">${esc(label)}</div>
        </div>`;
      }).join("")}</div>`;
    }

    function bodyTable(records) {
      return `<div class="table-wrap"><table>
        <thead><tr><th>日期</th><th>体重</th><th>体脂</th><th>心率</th><th>血压</th><th>精力</th></tr></thead>
        <tbody>${records.map((record) => `
          <tr>
            <td class="nowrap">${esc(record.date || "—")}</td>
            <td>${esc(record.weight_kg ?? "—")}</td>
            <td>${esc(record.body_fat_pct ?? "—")}</td>
            <td>${esc(record.heart_rate_bpm ?? "—")}</td>
            <td>${record.systolic_bp ? `${esc(record.systolic_bp)}/${esc(record.diastolic_bp ?? "—")}` : "—"}</td>
            <td>${record.energy_level ? `${esc(record.energy_level)}/5` : "—"}</td>
          </tr>
        `).join("")}</tbody>
      </table></div>`;
    }

    /* ---- life ---- */
    function renderLife() {
      const root = $("#view-life");
      const items = state.life && state.life.data && state.life.data.clothes && state.life.data.clothes.items
        ? state.life.data.clothes.items.filter((item) => !item.archived_at)
        : [];
      const byStatus = (status) => items.filter((item) => item.status === status);
      const dirty = items.filter((item) => ["dirty", "washing", "drying"].includes(item.status));
      const wearing = byStatus("wearing");
      const clean = byStatus("clean");

      root.innerHTML = `
        <div class="hero">
          <div>
            <h2>${items.length} 件衣物，${dirty.length} 件待处理</h2>
          </div>
          <div class="badge ${dirty.length ? "amber" : "green"}">${dirty.length ? "需要清洗" : "洗衣队列为空"}</div>
        </div>
        <div class="grid-2 even">
          <div class="panel">
            <div class="panel-head"><h3>洗衣队列</h3><span class="meta">${dirty.length} 件</span></div>
            ${dirty.length ? `<div class="list">${dirty.map((item) => `
              <div class="list-item">
                <div class="dot ${item.status === "dirty" ? "high" : "normal"}"></div>
                <div class="list-body">
                  <div class="list-title">${esc(item.name)}</div>
                  <div class="list-sub">${esc(item.category || "")} · ${esc(item.location || "—")} · 穿着 ${item.wear_count_since_wash ?? 0}/${item.max_wears_before_wash ?? "—"} 次</div>
                </div>
                ${statusBadge(item.status)}
              </div>
            `).join("")}</div>` : emptyBlock("没有待处理衣物", "所有衣物都在可穿或穿着状态。")}
          </div>
          <div class="panel">
            <div class="panel-head"><h3>正在穿</h3><span class="meta">${wearing.length} 件</span></div>
            ${wearing.length ? `<div class="list">${wearing.map((item) => `
              <div class="list-item">
                <div class="dot open"></div>
                <div class="list-body">
                  <div class="list-title">${esc(item.name)}</div>
                  <div class="list-sub">${esc(item.category || "")} · ${esc(item.location || "—")}</div>
                </div>
              </div>
            `).join("")}</div>` : emptyBlock("没有正在穿的衣物", "记录穿着后会显示在这里。")}
          </div>
        </div>
        <div class="panel" style="margin-top:16px">
          <div class="panel-head"><h3>全部衣物</h3><span class="meta">${items.length} 件 · ${clean.length} 件可穿</span></div>
          ${items.length ? clothingTable(items) : emptyBlock("还没有衣物记录", "使用 life.clothes.add 添加第一件衣物。")}
        </div>
      `;
    }

    function clothingTable(items) {
      return `<div class="table-wrap"><table>
        <thead><tr><th>名称</th><th>分类</th><th>状态</th><th>穿着次数</th><th>位置</th><th>最近穿着</th></tr></thead>
        <tbody>${items.map((item) => `
          <tr>
            <td>${esc(item.name)}</td>
            <td class="muted">${esc(item.category || "—")}</td>
            <td>${statusBadge(item.status)}</td>
            <td class="muted">${item.wear_count_since_wash ?? 0}/${item.max_wears_before_wash ?? "—"}</td>
            <td class="muted">${esc(item.location || "—")}</td>
            <td class="muted nowrap">${esc(item.last_worn_date || "—")}</td>
          </tr>
        `).join("")}</tbody>
      </table></div>`;
    }

    function emptyBlock(title, message) {
      return `<div class="empty"><strong>${esc(title)}</strong>${esc(message)}</div>`;
    }

    async function checkAssetVersion() {
      try {
        const response = await fetch("/", { cache: "no-store" });
        if (!response.ok) return;
        const html = await response.text();
        const match = html.match(/name="me-asset-version" content="([^"]+)"/);
        if (match && match[1] !== assetVersion) location.reload();
      } catch (_) {
        // Offline; keep the current page and retry on the next interval.
      }
    }

    function maybeCheckAssetVersion() {
      const now = Date.now();
      if (now - lastAssetCheck < 300000) return;
      lastAssetCheck = now;
      checkAssetVersion();
    }

    async function refresh(silent = false) {
      if (refreshInFlight) return;
      refreshInFlight = true;
      const button = $("#refresh");
      button.classList.add("spinning");
      const snapshot = snapshotDataState();
      invalidateAllData();
      try {
        const work = [loadSummary()];
        if (state.view !== "overview") work.push(loadView(state.view));
        await Promise.all(work);
        if (!silent) toast("已刷新");
      } catch (error) {
        restoreDataState(snapshot);
        if (!silent) toast(error.message);
      } finally {
        refreshInFlight = false;
        button.classList.remove("spinning");
      }
    }

    function bindEvents() {
      installCalendarTooltip();

      $("#edit-profile").addEventListener("click", openProfileEditor);
      $("#editor-close").addEventListener("click", () => closeEditor());
      $("#editor-cancel").addEventListener("click", () => closeEditor());
      $("#editor-backdrop").addEventListener("click", () => closeEditor());
      $("#editor-form").addEventListener("submit", submitEditor);
      document.addEventListener("keydown", (event) => {
        if (!editorContext) return;
        if (event.key === "Escape") {
          event.preventDefault();
          closeEditor();
          return;
        }
        trapEditorFocus(event);
      });

      $("#login-form").addEventListener("submit", async (event) => {
        event.preventDefault();
        const password = $("#password-input").value.trim();
        const error = $("#login-error");
        error.textContent = "";
        if (!password) {
          error.textContent = "请输入密码";
          return;
        }
        try {
          await login(password);
        } catch (loginError) {
          error.textContent = loginError.message;
        }
      });

      $("#logout").addEventListener("click", logout);
      $("#refresh").addEventListener("click", () => {
        lastAssetCheck = Date.now();
        checkAssetVersion();
        refresh(false);
      });

      $$("#nav button").forEach((button) => {
        button.addEventListener("click", () => setView(button.dataset.view));
      });

      document.addEventListener("click", (event) => {
        const target = event.target.closest("[data-goto]");
        if (target) setView(target.dataset.goto);
      });

      setInterval(() => {
        if (
          document.visibilityState === "visible" &&
          !$("#app").classList.contains("hidden") &&
          !editorContext &&
          pendingCommands === 0
        ) {
          maybeCheckAssetVersion();
          refresh(true);
        }
      }, 60000);
    }

    async function init() {
      bindEvents();
      try {
        // /dashboard/summary is authenticated, so it doubles as the session probe
        // and saves one network round trip on every initial page load.
        await loadSummary();
        await showApp();
      } catch (error) {
        if (error.status === 401) {
          showLogin();
          return;
        }
        await showApp();
        toast(error.message);
      }
    }

    init();
  })();
