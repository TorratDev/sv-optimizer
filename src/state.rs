use crate::{
    planner,
    protocol::{FarmSnapshot, RefreshRequest},
    storage::Store,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, RwLock, mpsc};
type ObserverSession = (String, mpsc::Sender<Result<RefreshRequest, tonic::Status>>);

#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<RwLock<Option<FarmSnapshot>>>,
    pub store: Arc<Mutex<Store>>,
    pub refresh: Arc<RwLock<Option<ObserverSession>>>,
    pub updated: Arc<Notify>,
    pub fixture: bool,
    pub token: String,
}
impl AppState {
    pub fn open(
        directory: &Path,
        mut fixture: Option<FarmSnapshot>,
        token: String,
    ) -> Result<Self> {
        let mut store = Store::open(directory)?;
        if let Some(s) = fixture.as_mut() {
            s.refresh_request_id = uuid::Uuid::new_v4().to_string();
            planner::validate_snapshot(s)?;
            store.save_snapshot(s)?;
        }
        let snapshot = fixture.clone().or(store.latest_snapshot()?);
        Ok(Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            store: Arc::new(Mutex::new(store)),
            refresh: Arc::new(RwLock::new(None)),
            updated: Arc::new(Notify::new()),
            fixture: fixture.is_some(),
            token,
        })
    }
    pub async fn observe(&self, s: FarmSnapshot) -> Result<()> {
        if s.world_ready {
            planner::validate_snapshot(&s)?;
        }
        ensure!(s.schema_version == 1, "Unsupported snapshot version");
        let now = chrono::Utc::now().timestamp_millis();
        ensure!(
            s.captured_at_unix_ms <= now + 60000 && s.captured_at_unix_ms >= now - 300000,
            "Snapshot timestamp is outside the permitted clock window"
        );
        self.store
            .lock()
            .map_err(|_| anyhow::anyhow!("Snapshot storage lock failed"))?
            .save_snapshot(&s)?;
        *self.snapshot.write().await = Some(s);
        self.updated.notify_waiters();
        Ok(())
    }
    pub async fn current(&self) -> Result<FarmSnapshot> {
        let s = self.snapshot.read().await.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "No farm snapshot yet. Start SMAPI, load your save, and connect the Observer mod."
            )
        })?;
        planner::validate_snapshot(&s)?;
        Ok(s)
    }
    pub async fn fresh(&self) -> Result<FarmSnapshot> {
        if self.fixture {
            return self.current().await;
        }
        let sender=self.refresh.read().await.as_ref().map(|(_,s)|s.clone()).ok_or_else(||anyhow::anyhow!("Observer is disconnected. Stored snapshots are historical; planning requires a live refresh."))?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let wait = self.updated.notified();
        tokio::pin!(wait);
        wait.as_mut().enable();
        sender
            .send(Ok(RefreshRequest {
                request_id: request_id.clone(),
            }))
            .await
            .map_err(|_| anyhow::anyhow!("Observer disconnected during refresh"))?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let time_left = deadline.saturating_duration_since(Instant::now());
            tokio::time::timeout(time_left,wait.as_mut()).await.map_err(|_|anyhow::anyhow!("Game did not refresh within 8 seconds. Unpause/unminimize the game, then retry."))?;
            let s = self.snapshot.read().await.clone();
            if let Some(s) = s
                && s.refresh_request_id == request_id
            {
                planner::validate_snapshot(&s)?;
                return Ok(s);
            }
            wait.set(self.updated.notified());
            wait.as_mut().enable();
        }
    }
    pub async fn tool(&self, name: &str, args: Value) -> Result<Value> {
        let args = args
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Tool arguments must be an object"))?;
        let allowed: &[&str] = match name {
            "get_farm_snapshot" => &["refresh"],
            "compare_investments" | "plan_earnings" => &["horizon_days", "constraints"],
            "check_plan_progress" => &[],
            _ => return Err(anyhow::anyhow!("Unknown tool: {name}")),
        };
        ensure!(
            args.keys().all(|k| allowed.contains(&k.as_str())),
            "Unknown tool argument"
        );
        if name == "get_farm_snapshot" {
            let refresh = args
                .get("refresh")
                .map(|v| {
                    v.as_bool()
                        .ok_or_else(|| anyhow::anyhow!("refresh must be boolean"))
                })
                .transpose()?
                .unwrap_or(true);
            let s = if refresh {
                self.fresh().await?
            } else {
                self.snapshot
                    .read()
                    .await
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No snapshot available"))?
            };
            let connected = self.refresh.read().await.is_some();
            return Ok(
                json!({"snapshot":s,"live_connected":connected,"simulation":self.fixture,"age_ms":(chrono::Utc::now().timestamp_millis()-s.captured_at_unix_ms).max(0),"historical":!refresh&&!self.fixture}),
            );
        }
        let s = self.fresh().await?;
        let h = args
            .get("horizon_days")
            .map(|v| {
                v.as_u64()
                    .filter(|x| *x <= u32::MAX as u64)
                    .map(|x| x as u32)
                    .ok_or_else(|| anyhow::anyhow!("horizon_days must be a positive integer"))
            })
            .transpose()?
            .unwrap_or(29 - s.day);
        let constraints: planner::Constraints = args
            .get("constraints")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();
        match name {
            "compare_investments" => planner::compare(&s, h, &constraints),
            "plan_earnings" => {
                let p = tokio::task::spawn_blocking(move || planner::plan(&s, h, &constraints))
                    .await??;
                self.store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Plan storage lock failed"))?
                    .save_plan(&p)?;
                Ok(
                    json!({"plan":p,"checklist":planner::render(&p,false),"simulation":self.fixture}),
                )
            }
            "check_plan_progress" => {
                let plan = self
                    .store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Plan storage lock failed"))?
                    .latest_plan(&s.save_id, &s.player_id)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "No plan exists for this save/player; call plan_earnings first"
                        )
                    })?;
                let date_matches = s.year == plan.year && s.season == plan.season;
                let morning = plan.days.iter().find(|d| d.day == s.day);
                let expected = if date_matches {
                    morning.map(|d| d.opening_cash_conservative)
                } else {
                    None
                };
                let needs_replan = !date_matches
                    || s.day > plan.deadline_day
                    || expected.is_some_and(|e| s.money != e);
                let original = self
                    .store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Snapshot storage lock failed"))?
                    .snapshot_at(
                        &s.save_id,
                        &s.player_id,
                        plan.snapshot_at_unix_ms,
                        &plan.source_snapshot_request_id,
                    )?;
                let equipment_changed = original.as_ref().is_some_and(|old| old.tools != s.tools);
                let farming_skill_changed = original.as_ref().is_some_and(|old| {
                    old.farming_level != s.farming_level || old.professions != s.professions
                });
                let crop_progress = if date_matches {
                    original
                        .as_ref()
                        .map(|original| crate::progress::crops(original, &s, &plan))
                } else {
                    None
                };
                let crop_changed = crop_progress.as_ref().is_some_and(|p| {
                    p["crop_differences"]
                        .as_array()
                        .is_some_and(|x| !x.is_empty())
                        || p["harvest_date_differences"]
                            .as_array()
                            .is_some_and(|x| !x.is_empty())
                });
                let planned_actions = morning.map(|d| &d.actions);
                Ok(
                    json!({"plan_id":plan.id,"snapshot_at_unix_ms":s.captured_at_unix_ms,"day":s.day,"time_of_day":s.time_of_day,"current_money":s.money,"planned_morning_cash_conservative":expected,"cash_difference_from_planned_morning":expected.map(|e|s.money-e),"date_matches":date_matches,"source_snapshot_found":original.is_some(),"equipment_changed":equipment_changed,"farming_skill_changed":farming_skill_changed,"recalculation_suggested":needs_replan||crop_changed||equipment_changed||farming_skill_changed||original.is_none(),"today_actions":planned_actions,"crop_progress":crop_progress,"current_crop_state":s.plots.iter().filter(|p|p.crop.is_some()).map(|p|json!({"tile":p.tile,"crop":p.crop,"watered_today":p.watered_today})).collect::<Vec<_>>(),"note":"Morning cash comparison is a checkpoint, not an audit of completed actions. At midday, purchases and sales may explain cash differences. Extra cash or improved equipment may permit better investments. No replacement plan is created automatically."}),
                )
            }
            _ => unreachable!(),
        }
    }
}
