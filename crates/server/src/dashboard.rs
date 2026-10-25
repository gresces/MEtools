//! Aggregated, read-only view models for the dashboard.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, SecondsFormat, TimeZone, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::data::{self, LoadedResource, Resource};

const OPEN_ACTION_STATUSES: &[&str] = &["open", "in_progress", "doing"];
const ACTIVE_GOAL_STATUSES: &[&str] = &["active", "in_progress", "current"];
const CLOSED_MILESTONE_STATUSES: &[&str] = &["completed", "skipped"];

/// Build the dashboard summary. Missing files become empty sections instead of
/// failing the whole page.
pub fn summary(store: &dyn shared::DocumentStore, tz: FixedOffset) -> Value {
    let now = Utc::now().with_timezone(&tz);
    let today = now.date_naive();

    let profile = load_opt(store, Resource::Profile);
    let actions = load_opt(store, Resource::Actions);
    let health = load_opt(store, Resource::Health);
    let life = load_opt(store, Resource::Life);
    let goals = load_opt(store, Resource::Goals);

    let versions = json!({
        "profile": profile.as_ref().and_then(|doc| doc.version),
        "actions": actions.as_ref().and_then(|doc| doc.version),
        "health": health.as_ref().and_then(|doc| doc.version),
        "life": life.as_ref().and_then(|doc| doc.version),
        "goals": goals.as_ref().and_then(|doc| doc.version),
    });

    let profile_view = profile_view(profile.as_ref());
    let action_view = action_view(actions.as_ref(), now);
    let goal_view = goal_view(goals.as_ref(), today);
    let health_view = health_view(health.as_ref(), today, tz);
    let life_view = life_view(life.as_ref());
    let projects = data::project_summaries(store);
    let project_view = project_view(&projects);

    json!({
        "status": "succeeded",
        "generated_at": now.to_rfc3339_opts(SecondsFormat::Secs, false),
        "today": today.to_string(),
        "timezone_offset_hours": tz.local_minus_utc() / 3600,
        "profile": profile_view,
        "versions": versions,
        "actions": action_view,
        "goals": goal_view,
        "health": health_view,
        "life": life_view,
        "projects": project_view,
    })
}

fn load_opt(store: &dyn shared::DocumentStore, resource: Resource) -> Option<LoadedResource> {
    data::load_resource_for_read(store, resource, false).ok()
}

fn profile_view(profile: Option<&LoadedResource>) -> Value {
    const RESERVED_FIELDS: &[&str] = &["version", "updated_at", "updated_by", "recent_requests"];

    let Some(profile) = profile else {
        return json!({});
    };
    let mut view = profile.value.clone();
    if let Some(fields) = view.as_object_mut() {
        for field in RESERVED_FIELDS {
            fields.remove(*field);
        }
    }
    view
}

fn action_view(actions: Option<&LoadedResource>, now: DateTime<FixedOffset>) -> Value {
    let empty: Vec<Value> = Vec::new();
    let items = actions
        .and_then(|doc| doc.value.get("items"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let mut total = 0usize;
    let mut open = 0usize;
    let mut overdue = 0usize;
    let mut due_today = 0usize;
    let mut done_7d = 0usize;
    let mut focus: Vec<&Value> = Vec::new();

    for item in items {
        if item
            .get("archived_at")
            .is_some_and(|archived| !archived.is_null())
        {
            continue;
        }
        total += 1;
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        let is_open = OPEN_ACTION_STATUSES.contains(&status);
        let due = action_due(item, now);
        let is_overdue = is_open && due.is_some_and(|date| date < now);
        let is_due_today = due.is_some_and(|date| date.date_naive() == now.date_naive());

        if is_open {
            open += 1;
            if is_overdue {
                overdue += 1;
            }
            if is_due_today {
                due_today += 1;
            }
            focus.push(item);
        } else if status == "done" {
            let completed = item
                .get("completed_at")
                .and_then(Value::as_str)
                .or_else(|| item.get("updated_at").and_then(Value::as_str))
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok());
            if completed
                .is_some_and(|date| date.with_timezone(now.offset()) >= now - Duration::days(7))
            {
                done_7d += 1;
            }
        }
    }

    // Execution queue order: index 0 is the current action, index 1 the next one.
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for item in items {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            by_id.entry(id).or_insert(item);
        }
    }
    let mut ordered: Vec<Value> = Vec::new();
    if let Some(queue) = actions
        .and_then(|doc| doc.value.get("queue"))
        .and_then(Value::as_array)
    {
        for value in queue {
            let Some(id) = value.as_str() else {
                continue;
            };
            if let Some(item) = by_id.get(id) {
                ordered.push((*item).clone());
            }
        }
    }
    let queue: Vec<Value> = if ordered.is_empty() {
        focus.sort_by(|left, right| {
            let left_due = action_due(left, now);
            let right_due = action_due(right, now);
            let left_key = (
                !left_due.is_some_and(|date| date < now),
                left_due.map(|date| date.timestamp()).unwrap_or(i64::MAX),
                priority_rank(left.get("priority").and_then(Value::as_str)),
            );
            let right_key = (
                !right_due.is_some_and(|date| date < now),
                right_due.map(|date| date.timestamp()).unwrap_or(i64::MAX),
                priority_rank(right.get("priority").and_then(Value::as_str)),
            );
            left_key.cmp(&right_key)
        });
        focus.into_iter().cloned().collect()
    } else {
        ordered
    };
    let queue_length = queue.len();
    let focus_view: Vec<Value> = queue.iter().take(8).cloned().collect();
    let queue_view: Vec<Value> = queue.iter().take(50).cloned().collect();

    json!({
        "total": total,
        "open": open,
        "overdue": overdue,
        "due_today": due_today,
        "done_7d": done_7d,
        "focus": focus_view,
        "queue": queue_view,
        "queue_length": queue_length,
    })
}

fn action_due(item: &Value, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    if let Some(raw) = item.get("due_at").and_then(Value::as_str) {
        if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
            return Some(date.with_timezone(now.offset()));
        }
        if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
            return Some(end_of_day(date, *now.offset()));
        }
    }
    item.get("due_date")
        .and_then(Value::as_str)
        .and_then(|raw| NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok())
        .map(|date| end_of_day(date, *now.offset()))
}

fn end_of_day(date: NaiveDate, tz: FixedOffset) -> DateTime<FixedOffset> {
    tz.from_local_datetime(&date.and_hms_opt(23, 59, 59).expect("valid time"))
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("valid time")))
}

fn priority_rank(priority: Option<&str>) -> u8 {
    match priority.unwrap_or("normal") {
        "urgent" => 0,
        "high" => 1,
        "normal" | "medium" => 2,
        "low" => 3,
        _ => 2,
    }
}

fn is_goal_group(goal: &Value) -> bool {
    match goal.get("parent_goal_ids").and_then(Value::as_array) {
        Some(parents) => parents.is_empty(),
        None => true,
    }
}

fn goal_view(goals: Option<&LoadedResource>, today: NaiveDate) -> Value {
    let empty: Vec<Value> = Vec::new();
    let document = goals.map(|doc| &doc.value);
    let goals = document
        .and_then(|doc| doc.get("goals"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let milestones = document
        .and_then(|doc| doc.get("milestones"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let checkpoints = document
        .and_then(|doc| doc.get("checkpoints"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let action_links = document
        .and_then(|doc| doc.get("action_links"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let goal_titles: BTreeMap<String, String> = goals
        .iter()
        .filter_map(|goal| {
            Some((
                goal.get("id").and_then(Value::as_str)?.to_string(),
                goal.get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect();

    let active_goals: Vec<&Value> = goals
        .iter()
        .filter(|goal| !is_goal_group(goal))
        .filter(|goal| {
            goal.get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| ACTIVE_GOAL_STATUSES.contains(&status))
        })
        .collect();

    let average_progress = if active_goals.is_empty() {
        0
    } else {
        let sum: u64 = active_goals
            .iter()
            .map(|goal| goal.get("progress").and_then(Value::as_u64).unwrap_or(0))
            .sum();
        (sum / active_goals.len() as u64) as u32
    };

    let mut milestones_by_goal: HashMap<&str, Vec<&Value>> = HashMap::new();
    for milestone in milestones {
        if let Some(goal_id) = milestone.get("goal_id").and_then(Value::as_str) {
            milestones_by_goal
                .entry(goal_id)
                .or_default()
                .push(milestone);
        }
    }
    let mut checkpoints_by_milestone: HashMap<&str, usize> = HashMap::new();
    for checkpoint in checkpoints {
        if let Some(milestone_id) = checkpoint.get("milestone_id").and_then(Value::as_str) {
            *checkpoints_by_milestone.entry(milestone_id).or_default() += 1;
        }
    }
    let mut action_counts: HashMap<&str, usize> = HashMap::new();
    for link in action_links {
        let mut goal_ids: BTreeSet<&str> = BTreeSet::new();
        if let Some(goal_id) = link.get("goal_id").and_then(Value::as_str) {
            goal_ids.insert(goal_id);
        }
        if let Some(ids) = link.get("goal_ids").and_then(Value::as_array) {
            goal_ids.extend(ids.iter().filter_map(Value::as_str));
        }
        for goal_id in goal_ids {
            *action_counts.entry(goal_id).or_default() += 1;
        }
    }

    let top: Vec<Value> = active_goals
        .iter()
        .map(|goal| {
            let goal_id = goal.get("id").and_then(Value::as_str).unwrap_or("");
            let goal_milestones = milestones_by_goal
                .get(goal_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let parent_names: Vec<Value> = goal
                .get("parent_goal_ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .filter_map(|id| goal_titles.get(id))
                        .map(|title| Value::String(title.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let checkpoint_count: usize = goal_milestones
                .iter()
                .filter_map(|milestone| milestone.get("id").and_then(Value::as_str))
                .filter_map(|milestone_id| checkpoints_by_milestone.get(milestone_id))
                .sum();
            let action_count = action_counts.get(goal_id).copied().unwrap_or(0);
            json!({
                "id": goal.get("id").cloned().unwrap_or(Value::Null),
                "title": goal.get("title").cloned().unwrap_or(Value::Null),
                "description": goal.get("description").cloned().unwrap_or(Value::Null),
                "purpose": goal.get("purpose").cloned().unwrap_or(Value::Null),
                "status": goal.get("status").cloned().unwrap_or(Value::Null),
                "health": goal.get("health").cloned().unwrap_or(Value::Null),
                "priority": goal.get("priority").cloned().unwrap_or(Value::Null),
                "progress": goal.get("progress").cloned().unwrap_or(Value::Null),
                "deadline": goal.get("deadline").cloned().unwrap_or(Value::Null),
                "next_action": goal.get("next_action").cloned().unwrap_or(Value::Null),
                "parent_goal_ids": goal.get("parent_goal_ids").cloned().unwrap_or_else(|| json!([])),
                "parent_goal_titles": parent_names,
                "milestone_count": goal_milestones.len(),
                "milestones_done": goal_milestones.iter().filter(|milestone| {
                    milestone.get("status").and_then(Value::as_str) == Some("completed")
                }).count(),
                "checkpoint_count": checkpoint_count,
                "action_count": action_count,
            })
        })
        .collect();

    let mut upcoming: Vec<(NaiveDate, Value)> = milestones
        .iter()
        .filter(|milestone| {
            !milestone
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| CLOSED_MILESTONE_STATUSES.contains(&status))
        })
        .filter_map(|milestone| {
            let due = milestone
                .get("due_at")
                .and_then(Value::as_str)
                .and_then(parse_date_prefix)?;
            if due > today + Duration::days(14) {
                return None;
            }
            Some((
                due,
                json!({
                    "id": milestone.get("id").cloned().unwrap_or(Value::Null),
                    "title": milestone.get("title").cloned().unwrap_or(Value::Null),
                    "goal_id": milestone.get("goal_id").cloned().unwrap_or(Value::Null),
                    "due_at": milestone.get("due_at").cloned().unwrap_or(Value::Null),
                    "status": milestone.get("status").cloned().unwrap_or(Value::Null),
                    "progress": milestone.get("progress").cloned().unwrap_or(Value::Null),
                    "overdue": due < today,
                }),
            ))
        })
        .collect();
    upcoming.sort_by_key(|(due, _)| *due);
    let upcoming: Vec<Value> = upcoming.into_iter().map(|(_, item)| item).collect();

    json!({
        "active_goals": active_goals.len(),
        "average_progress": average_progress,
        "milestones_due_soon": upcoming.len(),
        "top": top,
        "upcoming_milestones": upcoming.into_iter().take(8).collect::<Vec<_>>(),
    })
}

fn health_view(health: Option<&LoadedResource>, today: NaiveDate, tz: FixedOffset) -> Value {
    let Some(health) = health else {
        return json!({
            "water": {"today_ml": 0, "goal_ml": 2000, "ratio": 0.0, "series": []},
            "sleep": {"last_night_minutes": null, "goal_minutes": null, "quality": null, "series": []},
            "body": null,
        });
    };

    let water = health.value.get("water");
    let water_goal = water
        .and_then(|water| water.get("daily_goal_ml"))
        .and_then(Value::as_u64)
        .unwrap_or(2000);
    let water_records: &[Value] = water
        .and_then(|water| water.get("records"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut water_by_date: HashMap<&str, u64> = HashMap::new();
    for record in water_records {
        if let Some(date) = record_date(record) {
            *water_by_date.entry(date).or_default() +=
                record.get("amount_ml").and_then(Value::as_u64).unwrap_or(0);
        }
    }

    let sleep = health.value.get("sleep");
    let sleep_goal = sleep
        .and_then(|sleep| sleep.get("goal"))
        .and_then(|goal| goal.get("target_duration_minutes"))
        .and_then(Value::as_u64);
    let sleep_records: &[Value] = sleep
        .and_then(|sleep| sleep.get("records"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut sleep_by_date: HashMap<&str, &Value> = HashMap::new();
    for record in sleep_records {
        let Some(date) = record_date(record) else {
            continue;
        };
        let is_latest = sleep_by_date.get(date).is_none_or(|current| {
            record
                .get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                >= current
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or("")
        });
        if is_latest {
            sleep_by_date.insert(date, record);
        }
    }

    let mut water_series = Vec::new();
    let mut sleep_series = Vec::new();
    let mut today_ml = 0u64;
    let mut last_sleep: Option<(NaiveDate, Value)> = None;

    for offset in (0..14).rev() {
        let date = today - Duration::days(offset);
        let date_text = date.to_string();

        let day_ml = water_by_date.get(date_text.as_str()).copied().unwrap_or(0);
        if date == today {
            today_ml = day_ml;
        }
        water_series.push(json!({"date": date_text, "ml": day_ml}));

        let sleep_record = sleep_by_date.get(date_text.as_str()).copied();
        let minutes = sleep_record
            .and_then(|record| record.get("duration_minutes"))
            .and_then(Value::as_u64);
        let quality = sleep_record
            .and_then(|record| record.get("quality_score"))
            .and_then(Value::as_u64);
        if sleep_record.is_some() {
            last_sleep = Some((
                date,
                json!({
                    "date": date_text,
                    "minutes": minutes,
                    "quality": quality,
                }),
            ));
        }
        sleep_series.push(json!({"date": date_text, "minutes": minutes, "quality": quality}));
    }

    let body_series = health
        .value
        .get("body")
        .and_then(|body| body.get("profile"));
    let body_records: &[Value] = health
        .value
        .get("body")
        .and_then(|body| body.get("records"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let latest_body = body_records
        .iter()
        .max_by_key(|record| record_date(record).unwrap_or(""))
        .map(|record| {
            json!({
                "date": record.get("date").cloned().unwrap_or(Value::Null),
                "weight_kg": record.get("weight_kg").cloned().unwrap_or(Value::Null),
                "body_fat_pct": record.get("body_fat_pct").cloned().unwrap_or(Value::Null),
                "heart_rate_bpm": record.get("heart_rate_bpm").cloned().unwrap_or(Value::Null),
                "systolic_bp": record.get("systolic_bp").cloned().unwrap_or(Value::Null),
                "diastolic_bp": record.get("diastolic_bp").cloned().unwrap_or(Value::Null),
                "energy_level": record.get("energy_level").cloned().unwrap_or(Value::Null),
                "height_cm": body_series.and_then(|profile| profile.get("height_cm")).cloned().unwrap_or(Value::Null),
                "target_weight_kg": body_series.and_then(|profile| profile.get("target_weight_kg")).cloned().unwrap_or(Value::Null),
            })
        });

    let last_sleep_value = last_sleep.map(|(_, value)| value);
    json!({
        "water": {
            "today_ml": today_ml,
            "goal_ml": water_goal,
            "ratio": if water_goal == 0 { 0.0 } else { today_ml as f64 / water_goal as f64 },
            "series": water_series,
        },
        "sleep": {
            "last_night_minutes": last_sleep_value.as_ref().and_then(|value| value.get("minutes")).cloned().unwrap_or(Value::Null),
            "last_night_date": last_sleep_value.as_ref().and_then(|value| value.get("date")).cloned().unwrap_or(Value::Null),
            "goal_minutes": sleep_goal,
            "quality": last_sleep_value.as_ref().and_then(|value| value.get("quality")).cloned().unwrap_or(Value::Null),
            "series": sleep_series,
        },
        "body": latest_body,
        "timezone": tz.to_string(),
    })
}

fn record_date(record: &Value) -> Option<&str> {
    record.get("date").and_then(Value::as_str).or_else(|| {
        let raw = record.get("recorded_at").and_then(Value::as_str)?;
        let prefix = raw.get(..10).unwrap_or(raw);
        parse_date_prefix(prefix).map(|_| prefix)
    })
}

fn life_view(life: Option<&LoadedResource>) -> Value {
    let empty: Vec<Value> = Vec::new();
    let items = life
        .and_then(|doc| doc.value.get("clothes"))
        .and_then(|clothes| clothes.get("items"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let active: Vec<&Value> = items
        .iter()
        .filter(|item| {
            !item
                .get("archived_at")
                .is_some_and(|archived| !archived.is_null())
        })
        .collect();

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in &active {
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(status).or_default() += 1;
    }

    let mut to_wash: Vec<Value> = active
        .iter()
        .filter(|item| {
            matches!(
                item.get("status").and_then(Value::as_str),
                Some("dirty" | "washing" | "drying")
            )
        })
        .map(|item| {
            json!({
                "id": item.get("id").cloned().unwrap_or(Value::Null),
                "name": item.get("name").cloned().unwrap_or(Value::Null),
                "status": item.get("status").cloned().unwrap_or(Value::Null),
                "category": item.get("category").cloned().unwrap_or(Value::Null),
                "wear_count_since_wash": item.get("wear_count_since_wash").cloned().unwrap_or(Value::Null),
                "max_wears_before_wash": item.get("max_wears_before_wash").cloned().unwrap_or(Value::Null),
                "location": item.get("location").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    to_wash.sort_by(|left, right| {
        let left = left
            .get("wear_count_since_wash")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let right = right
            .get("wear_count_since_wash")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        right.cmp(&left)
    });

    let wearing: Vec<Value> = active
        .iter()
        .filter(|item| item.get("status").and_then(Value::as_str) == Some("wearing"))
        .map(|item| {
            json!({
                "id": item.get("id").cloned().unwrap_or(Value::Null),
                "name": item.get("name").cloned().unwrap_or(Value::Null),
                "category": item.get("category").cloned().unwrap_or(Value::Null),
                "location": item.get("location").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    json!({
        "total": active.len(),
        "clean": counts.get("clean").copied().unwrap_or(0),
        "needs_wash": to_wash.len(),
        "wearing": counts.get("wearing").copied().unwrap_or(0),
        "by_status": counts,
        "items_to_wash": to_wash.into_iter().take(8).collect::<Vec<_>>(),
        "items_wearing": wearing,
    })
}

fn project_view(projects: &Value) -> Value {
    let items = projects.as_array().cloned().unwrap_or_default();
    let active = items
        .iter()
        .filter(|project| {
            project
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| matches!(status, "active" | "in_progress" | "current"))
        })
        .count();
    json!({
        "active": active,
        "total": items.len(),
        "items": items.into_iter().take(6).collect::<Vec<_>>(),
    })
}

fn parse_date_prefix(raw: &str) -> Option<NaiveDate> {
    let prefix = raw.get(..10).unwrap_or(raw);
    NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok()
}

/// Convenience used by tests and clients that need a stable empty summary shape.
pub fn empty_summary() -> Value {
    json!({
        "status": "succeeded",
        "actions": {"total": 0, "open": 0, "overdue": 0, "due_today": 0, "done_7d": 0, "focus": [], "queue": [], "queue_length": 0},
        "goals": {"active_goals": 0, "average_progress": 0, "milestones_due_soon": 0, "top": [], "upcoming_milestones": []},
        "health": {"water": {"today_ml": 0, "goal_ml": 2000, "ratio": 0.0, "series": []}, "sleep": {"last_night_minutes": null, "goal_minutes": null, "quality": null, "series": []}, "body": null},
        "life": {"total": 0, "clean": 0, "needs_wash": 0, "wearing": 0, "by_status": {}, "items_to_wash": [], "items_wearing": []},
        "projects": {"active": 0, "total": 0, "items": []},
    })
}

#[cfg(test)]
mod tests {
    use super::{LoadedResource, NaiveDate, Value, goal_view, json};

    fn goals_resource(goals: Value) -> LoadedResource {
        LoadedResource {
            value: json!({
                "goals": goals,
                "milestones": [],
                "checkpoints": [],
                "action_links": [],
            }),
            version: Some(1),
            updated_at: None,
        }
    }

    #[test]
    fn goal_view_excludes_groups_from_active_count_progress_and_top() {
        let resource = goals_resource(json!([
            {
                "id": "goal-group",
                "parent_goal_ids": [],
                "status": "active",
                "progress": 100,
            },
            {
                "id": "goal-child",
                "parent_goal_ids": ["goal-group"],
                "status": "active",
                "progress": 20,
            },
        ]));
        let today = NaiveDate::from_ymd_opt(2026, 10, 23).expect("valid date");

        let view = goal_view(Some(&resource), today);

        assert_eq!(view["active_goals"], 1);
        assert_eq!(view["average_progress"], 20);
        let top = view["top"].as_array().expect("top array");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0]["id"], "goal-child");
    }

    #[test]
    fn goal_view_reports_zero_goals_when_only_groups_are_active() {
        let resource = goals_resource(json!([{
            "id": "goal-group",
            "parent_goal_ids": [],
            "status": "active",
            "progress": 80,
        }]));
        let today = NaiveDate::from_ymd_opt(2026, 10, 23).expect("valid date");

        let view = goal_view(Some(&resource), today);

        assert_eq!(view["active_goals"], 0);
        assert_eq!(view["average_progress"], 0);
        assert_eq!(view["top"].as_array().map(Vec::len), Some(0));
    }
}
