use crate::{planner::Plan, protocol::FarmSnapshot};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Compare against the planned start-of-day crop checkpoint. Today's actions
/// are still pending, so a midday difference is explained rather than treated
/// as proof the player did something wrong.
pub fn crops(original: &FarmSnapshot, current: &FarmSnapshot, plan: &Plan) -> Value {
    let mut expected: BTreeMap<(i32, i32), String> = original
        .plots
        .iter()
        .filter_map(|p| {
            p.tile
                .as_ref()
                .zip(p.crop.as_ref())
                .filter(|(_, c)| !c.dead)
                .map(|(t, c)| ((t.x, t.y), c.seed_id.clone()))
        })
        .collect();
    for day in plan.days.iter().filter(|d| d.day < current.day) {
        for action in &day.actions {
            for tile in &action.tiles {
                let key = (tile.x, tile.y);
                match action.kind.as_str() {
                    "remove_crop" => {
                        expected.remove(&key);
                    }
                    "plant" => {
                        expected.insert(key, action.item_id.clone());
                    }
                    "harvest"
                        if expected
                            .get(&key)
                            .and_then(|seed| original.crops.iter().find(|c| &c.seed_id == seed))
                            .is_some_and(|c| c.regrow_days == 0) =>
                    {
                        expected.remove(&key);
                    }
                    _ => {}
                }
            }
        }
    }
    let actual: BTreeMap<(i32, i32), String> = current
        .plots
        .iter()
        .filter_map(|p| {
            p.tile
                .as_ref()
                .zip(p.crop.as_ref())
                .filter(|(_, c)| !c.dead)
                .map(|(t, c)| ((t.x, t.y), c.seed_id.clone()))
        })
        .collect();
    let mut mismatches = Vec::new();
    for (tile, seed) in &expected {
        if actual.get(tile) != Some(seed) {
            mismatches.push(json!({"tile":{"x":tile.0,"y":tile.1},"expected_seed_id":seed,"actual_seed_id":actual.get(tile),"kind":"missing_or_changed_crop"}));
        }
    }
    for (tile, seed) in &actual {
        if !expected.contains_key(tile) {
            mismatches.push(json!({"tile":{"x":tile.0,"y":tile.1},"actual_seed_id":seed,"kind":"additional_crop"}));
        }
    }
    // Compare next scheduled harvest with observed crop phase. Unknown crop
    // definitions are not used to infer a ready date.
    let mut harvest_differences = Vec::new();
    for plot in &current.plots {
        let Some(tile) = &plot.tile else {
            continue;
        };
        let Some(crop) = &plot.crop else {
            continue;
        };
        if crop.dead || expected.get(&(tile.x, tile.y)) != Some(&crop.seed_id) {
            continue;
        }
        let next = plan
            .days
            .iter()
            .filter(|d| d.day >= current.day)
            .find(|d| {
                d.actions
                    .iter()
                    .any(|a| a.kind == "harvest" && a.tiles.contains(tile))
            })
            .map(|d| d.day);
        if let Some(next) = next {
            let observed = current.day + crop.days_to_harvest;
            if observed != next {
                harvest_differences.push(json!({"tile":tile,"seed_id":crop.seed_id,"planned_harvest_day":next,"observed_ready_day_with_daily_watering":observed}));
            }
        }
    }
    json!({"checkpoint":"start_of_current_day","crop_differences":mismatches,"harvest_date_differences":harvest_differences,"note":"At midday, differences may reflect today's completed actions. Read the day's checklist before treating them as deviations. Observed harvest dates assume daily watering from now."})
}
