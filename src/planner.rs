//! Deterministic, budget-feasible crop simulations. Search is deliberately
//! labelled heuristic: travel, crop quality, and future weather are estimates.
use crate::protocol::*;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
type SprinklerChoice = (
    f64,
    usize,
    Tile,
    Vec<usize>,
    Vec<(ShopDay, Offer, u32)>,
    bool,
);
type CropChoice = (f64, usize, usize, Option<(ShopDay, Offer)>, f64, f64);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Constraints {
    pub reserve_gold: i64,
    pub watering_capacity: Option<u32>,
    pub daily_energy: Option<f64>,
    pub daily_minutes: Option<f64>,
    pub travel_buffer_minutes: f64,
    pub sleep_at: u32,
    pub allow_clearing: bool,
    pub allow_replacement: bool,
    pub allow_sprinkler_investment: bool,
    pub max_new_plots: u32,
    pub time_limit_ms: u64,
}
impl Default for Constraints {
    fn default() -> Self {
        Self {
            reserve_gold: 0,
            watering_capacity: None,
            daily_energy: None,
            daily_minutes: None,
            travel_buffer_minutes: 60.0,
            sleep_at: 2400,
            allow_clearing: true,
            allow_replacement: true,
            allow_sprinkler_investment: true,
            max_new_plots: 200,
            time_limit_ms: 3000,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub kind: String,
    pub item_id: String,
    pub name: String,
    pub quantity: u32,
    pub tiles: Vec<Tile>,
    pub gold: i64,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyPlan {
    pub day: u32,
    pub opening_cash_conservative: i64,
    pub closing_cash_conservative: i64,
    pub closing_cash_expected: f64,
    pub watering_tiles: u32,
    pub energy_used: f64,
    pub minutes_used: f64,
    pub actions: Vec<Action>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub save_id: String,
    pub player_id: String,
    pub year: u32,
    pub season: String,
    pub snapshot_at_unix_ms: i64,
    #[serde(default)]
    pub source_snapshot_request_id: String,
    pub horizon_days: u32,
    pub deadline_day: u32,
    pub constraints: Constraints,
    pub optimality: String,
    pub strategy: String,
    pub evaluated_candidates: u32,
    pub calculation_ms: u128,
    pub starting_cash: i64,
    pub expected_final_cash: f64,
    pub conservative_final_cash: i64,
    pub unsold_crop_value_expected: f64,
    pub assumptions: Vec<String>,
    pub warnings: Vec<String>,
    pub days: Vec<DailyPlan>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Investment {
    pub seed_id: String,
    pub name: String,
    pub seed_price: Option<i64>,
    pub owned_seeds: u32,
    pub first_purchase_day: Option<u32>,
    pub first_harvest_day: Option<u32>,
    pub harvests: u32,
    pub expected_revenue_per_plot: f64,
    pub conservative_revenue_per_plot: i64,
    pub expected_margin_per_plot: Option<f64>,
    pub owned_seed_expected_margin_per_plot: Option<f64>,
    pub note: String,
}

pub fn validate_snapshot(s: &FarmSnapshot) -> Result<()> {
    ensure!(
        s.schema_version == 1,
        "Unsupported snapshot schema {}; expected 1",
        s.schema_version
    );
    ensure!(
        s.world_ready,
        "No loaded game: load a single-player save first"
    );
    ensure!(s.single_player, "This version supports single-player only");
    ensure!(
        !s.save_id.is_empty() && !s.player_id.is_empty(),
        "Missing save/player identity"
    );
    ensure!((1..=28).contains(&s.day) && s.year > 0, "Invalid game date");
    ensure!(
        ["spring", "summer", "fall", "winter"].contains(&s.season.as_str()),
        "Invalid season"
    );
    ensure!(s.money >= 0 && s.money <= 2_000_000_000, "Invalid money");
    ensure!(
        s.plots.len() <= 10000 && s.crops.len() <= 1000 && s.items.len() <= 20000,
        "Snapshot exceeds supported limits"
    );
    ensure!(
        s.max_energy.is_finite() && s.max_energy > 0.0 && s.max_energy < 10000.0,
        "Invalid energy"
    );
    ensure!(
        s.energy.is_finite() && s.energy >= 0.0 && s.energy <= s.max_energy,
        "Invalid current energy"
    );
    ensure!(
        s.watering_energy_per_tile.is_finite() && s.watering_energy_per_tile >= 0.0,
        "Invalid watering energy estimate"
    );
    ensure!(
        s.watering_minutes_per_tile.is_finite() && s.watering_minutes_per_tile > 0.0,
        "Invalid watering time estimate"
    );
    let mut tiles = BTreeSet::new();
    for p in &s.plots {
        let t = p
            .tile
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Plot without coordinates"))?;
        ensure!(tiles.insert((t.x, t.y)), "Duplicate plot coordinates");
        ensure!(
            p.speed_bonus.is_finite() && (0.0..=0.75).contains(&p.speed_bonus),
            "Invalid speed bonus"
        );
        ensure!(
            p.quality_fertilizer <= 3
                && p.clearing_energy.is_finite()
                && p.clearing_energy >= 0.0
                && p.clearing_minutes.is_finite()
                && p.clearing_minutes >= 0.0,
            "Invalid plot costs"
        );
    }
    let mut seeds = BTreeSet::new();
    for c in &s.crops {
        ensure!(seeds.insert(c.seed_id.clone()), "Duplicate crop seed IDs");
        ensure!(
            !c.seed_id.is_empty() && c.base_sale_price >= 0 && c.base_sale_price <= 100000,
            "Invalid crop definition"
        );
        ensure!(
            c.growth_phases.len() <= 20 && c.growth_phases.iter().all(|x| *x <= 120),
            "Invalid growth phases"
        );
        ensure!(
            c.minimum_yield > 0
                && c.expected_yield.is_finite()
                && c.expected_yield >= c.minimum_yield as f64
                && c.expected_yield < 10000.0,
            "Invalid yield"
        );
        ensure!(
            c.sale_prices_by_quality.is_empty()
                || (c.sale_prices_by_quality.len() == 4
                    && c.sale_prices_by_quality
                        .iter()
                        .all(|p| *p >= 0 && *p <= 200000)),
            "Invalid quality prices"
        );
    }
    for shop in &s.shops {
        ensure!(
            (1..=28).contains(&shop.day)
                && !shop.shop.is_empty()
                && shop.opens_at < shop.closes_at
                && shop.travel_minutes.is_finite()
                && shop.travel_minutes >= 0.0,
            "Invalid shop schedule"
        );
        for o in &shop.offers {
            ensure!(
                o.price > 0 && o.price <= 10000000 && o.stock >= -1,
                "Invalid shop offer"
            );
        }
    }
    for item in &s.items {
        ensure!(
            item.quantity <= 1000000
                && item.sale_price >= 0
                && item.sale_price <= 10000000
                && item.retrieval_minutes.is_finite()
                && item.retrieval_minutes >= 0.0,
            "Invalid inventory stack"
        );
    }
    for option in &s.sprinklers {
        ensure!(
            option.coverage_offsets.len() <= 100 && option.materials.len() <= 20,
            "Invalid sprinkler definition"
        );
        ensure!(
            option
                .materials
                .iter()
                .all(|m| m.quantity > 0 && m.quantity <= 1000),
            "Invalid sprinkler recipe"
        );
    }
    Ok(())
}

fn validate_constraints(s: &FarmSnapshot, h: u32, c: &Constraints) -> Result<u32> {
    validate_snapshot(s)?;
    ensure!(
        h > 0 && h <= 28,
        "horizon_days must be between 1 and 28, including today"
    );
    ensure!(
        c.reserve_gold >= 0 && c.reserve_gold <= s.money,
        "reserve_gold must be between 0 and current money"
    );
    ensure!(
        (50..=30000).contains(&c.time_limit_ms),
        "time_limit_ms must be between 50 and 30000"
    );
    ensure!(
        (600..=2600).contains(&c.sleep_at) && c.sleep_at % 100 < 60,
        "Invalid sleep_at; use game HHMM"
    );
    ensure!(
        c.travel_buffer_minutes.is_finite()
            && c.travel_buffer_minutes >= 0.0
            && c.travel_buffer_minutes <= 1000.0,
        "Invalid travel buffer"
    );
    if let Some(e) = c.daily_energy {
        ensure!(
            e.is_finite() && e > 0.0 && e <= s.max_energy,
            "daily_energy must be within maximum energy"
        );
    }
    if let Some(m) = c.daily_minutes {
        ensure!(
            m.is_finite() && m > 0.0 && m <= 1200.0,
            "daily_minutes must be in (0,1200]"
        );
    }
    ensure!(
        c.max_new_plots <= 5000 && c.watering_capacity.unwrap_or(0) <= 10000,
        "Plot/watering limit is too large"
    );
    Ok((s.day + h - 1).min(28))
}

pub fn growth_days(c: &CropDefinition, speed_bonus: f64, agriculturist: bool) -> u32 {
    let mut phases = c.growth_phases.clone();
    let total: u32 = phases.iter().sum();
    let mut remove = (total as f32
        * (speed_bonus as f32 + if agriculturist { 0.1f32 } else { 0.0 }))
    .ceil() as u32;
    // The game distributes speed reductions over phase lengths in up to 3 passes.
    for _ in 0..3 {
        for (i, phase) in phases.iter_mut().enumerate() {
            if remove > 0 && *phase > 0 && (i > 0 || *phase > 1) {
                *phase -= 1;
                remove -= 1;
            }
        }
    }
    phases.iter().sum::<u32>().max(1)
}

fn price(base: i64, quality: u32, tiller: bool) -> i64 {
    let multiplier = match quality {
        1 => 1.25,
        2 => 1.5,
        4 => 2.0,
        _ => 1.0,
    };
    let quality_price = (base as f64 * multiplier).floor();
    (quality_price * if tiller { 1.1 } else { 1.0 }).floor() as i64
}

pub fn harvest_revenue(
    c: &CropDefinition,
    level: u32,
    fertilizer: u32,
    tiller: bool,
) -> (i64, f64) {
    let gold =
        (0.2 * level as f64 / 10.0 + 0.2 * fertilizer as f64 * (level as f64 + 2.0) / 12.0 + 0.01)
            .max(0.0);
    let silver_roll = (gold * 2.0).min(0.75);
    let iridium = if fertilizer == 3 {
        (gold / 2.0).min(1.0)
    } else {
        0.0
    };
    let gold_prob = (1.0 - iridium) * gold.min(1.0);
    let silver_prob = (1.0 - iridium - gold_prob) * if fertilizer == 3 { 1.0 } else { silver_roll };
    let normal_prob = (1.0 - iridium - gold_prob - silver_prob).max(0.0);
    let prices: [i64; 4] = c
        .sale_prices_by_quality
        .as_slice()
        .try_into()
        .unwrap_or_else(|_| [0, 1, 2, 4].map(|q| price(c.base_sale_price, q, tiller)));
    let normal = prices[0];
    let expected_first = normal_prob * normal as f64
        + silver_prob * prices[1] as f64
        + gold_prob * prices[2] as f64
        + iridium * prices[3] as f64;
    // Only the first harvested item receives the crop-quality roll; extra items
    // (e.g. blueberries) are normal quality. Rare luck doubles are excluded.
    let minimum_first = prices[if fertilizer == 3 { 1 } else { 0 }];
    (
        minimum_first + (c.minimum_yield.saturating_sub(1) as i64) * normal,
        expected_first + (c.expected_yield - 1.0).max(0.0) * normal as f64,
    )
}

fn available_crop(c: &CropDefinition, s: &FarmSnapshot) -> bool {
    c.supported && !c.forage_crop && !c.trellis && c.seasons.contains(&s.season)
}
fn owned(s: &FarmSnapshot, id: &str) -> u32 {
    s.items
        .iter()
        .filter(|x| x.item_id == id)
        .map(|x| x.quantity)
        .sum()
}
pub fn compare(s: &FarmSnapshot, h: u32, constraints: &Constraints) -> Result<serde_json::Value> {
    let end = validate_constraints(s, h, constraints)?;
    let mut options = Vec::new();
    for crop in s.crops.iter().filter(|c| available_crop(c, s)) {
        let seed_count = owned(s, &crop.seed_id);
        let offer = s
            .shops
            .iter()
            .filter(|x| x.day >= s.day && x.day <= end)
            .flat_map(|shop| {
                shop.offers
                    .iter()
                    .filter(|o| o.item_id == crop.seed_id && o.stock != 0)
                    .map(move |o| (shop.day, o.price))
            })
            .min();
        let start = if seed_count > 0 {
            Some(s.day)
        } else {
            offer.map(|o| o.0)
        };
        let first = start.map(|d| d + growth_days(crop, 0.0, s.professions.contains(&5)));
        let harvests = first
            .filter(|d| *d <= end)
            .map(|d| 1 + (end - d).checked_div(crop.regrow_days).unwrap_or(0))
            .unwrap_or(0);
        let (low, expected) = harvest_revenue(crop, s.farming_level, 0, s.professions.contains(&0));
        let cost = offer
            .map(|o| o.1)
            .or(if seed_count > 0 { Some(0) } else { None });
        options.push(Investment {
            seed_id: crop.seed_id.clone(), name: crop.name.clone(), seed_price: offer.map(|o| o.1),
            owned_seeds: seed_count, first_purchase_day: offer.map(|o| o.0), first_harvest_day: first,
            harvests, expected_revenue_per_plot: expected * harvests as f64,
            conservative_revenue_per_plot: low * harvests as i64,
            expected_margin_per_plot: cost.map(|cost| expected * harvests as f64 - cost as f64),
            owned_seed_expected_margin_per_plot: if seed_count>0 {Some(expected*harvests as f64)}else{None},
            note: "Per-plot comparison, one planting, no fertilizer. Harvest value is not deadline cash until sold; use plan_earnings for shared cash, watering, reinvestment, shops, and sprinklers.".into(),
        });
    }
    options.sort_by(|a, b| {
        b.expected_margin_per_plot
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.expected_margin_per_plot.unwrap_or(f64::NEG_INFINITY))
    });
    let sprinkler_options:Vec<_>=s.sprinklers.iter().map(|sprinkler|{
        let purchase=s.shops.iter().filter(|shop|shop.day>=s.day&&shop.day<=end).flat_map(|shop|shop.offers.iter().filter(|o|o.item_id==sprinkler.item_id&&o.stock!=0).map(move|o|(shop.day,o.price))).min();
        let materials:Vec<_>=sprinkler.materials.iter().map(|m|serde_json::json!({"item_id":m.item_id,"required":m.quantity,"owned":owned(s,&m.item_id),"missing":m.quantity.saturating_sub(owned(s,&m.item_id))})).collect();
        serde_json::json!({"item_id":sprinkler.item_id,"name":sprinkler.name,"owned":owned(s,&sprinkler.item_id),"coverage_tiles":sprinkler.coverage_offsets.len(),"recipe_unlocked":sprinkler.recipe_unlocked,"first_purchase_day":purchase.map(|o|o.0),"purchase_price":purchase.map(|o|o.1),"materials":materials,"note":"Benefit depends on placement, watering bottlenecks, seeds and remaining growth time. plan_earnings compares owned placement, buying and crafting with complete farm simulations. Ore smelting and new furnace investments are outside this model."})
    }).collect();
    Ok(
        serde_json::json!({"deadline_day": end, "options": options, "sprinkler_options":sprinkler_options,"optimality": "comparison_only", "excluded_crops": s.crops.iter().filter(|c| !available_crop(c,s)).map(|c| serde_json::json!({"seed_id":c.seed_id,"reason":if c.trellis {"Trellis routing is not modeled"} else if c.forage_crop {"Forage crop mechanics are not modeled"} else {c.unsupported_reason.as_str()}})).collect::<Vec<_>>() }),
    )
}

#[derive(Clone)]
struct SimPlot {
    plot: Plot,
    crop: Option<usize>,
    due: u32,
    active: bool,
    planted_today: bool,
}
#[derive(Clone, Copy, Default)]
struct Value {
    low: i64,
    expected: f64,
}
impl Value {
    fn add(&mut self, other: Value) {
        self.low += other.low;
        self.expected += other.expected;
    }
}
fn value(c: &CropDefinition, p: &Plot, s: &FarmSnapshot) -> Value {
    let (low, expected) = harvest_revenue(
        c,
        s.farming_level,
        p.quality_fertilizer,
        s.professions.contains(&0),
    );
    Value { low, expected }
}
fn remaining_value(c: &CropDefinition, p: &Plot, s: &FarmSnapshot, first: u32, end: u32) -> Value {
    if first > end {
        return Value::default();
    }
    let n = 1 + (end - first).checked_div(c.regrow_days).unwrap_or(0);
    let v = value(c, p, s);
    Value {
        low: v.low * n as i64,
        expected: v.expected * n as f64,
    }
}
fn minutes(time: u32) -> f64 {
    ((time / 100) * 60 + time % 100) as f64
}
fn action(
    kind: &str,
    id: &str,
    name: &str,
    tile: Option<&Tile>,
    gold: i64,
    reason: &str,
) -> Action {
    Action {
        kind: kind.into(),
        item_id: id.into(),
        name: name.into(),
        quantity: 1,
        tiles: tile.cloned().into_iter().collect(),
        gold,
        reason: reason.into(),
    }
}
fn compact(actions: Vec<Action>) -> Vec<Action> {
    let mut result: Vec<Action> = Vec::new();
    for a in actions {
        if let Some(prev) = result.iter_mut().find(|x| {
            x.kind == a.kind && x.item_id == a.item_id && x.reason == a.reason && x.name == a.name
        }) {
            prev.quantity += a.quantity;
            prev.gold += a.gold;
            prev.tiles.extend(a.tiles);
        } else {
            result.push(a);
        }
    }
    result
}

struct Simulation<'a> {
    s: &'a FarmSnapshot,
    c: &'a Constraints,
    end: u32,
    plots: Vec<SimPlot>,
    stock: BTreeMap<String, u32>,
    money: i64,
    expected_money: f64,
    held: Value,
    shipped: Value,
    bought: BTreeMap<(u32, String, String), u32>,
    warnings: BTreeSet<String>,
    days: Vec<DailyPlan>,
    added_plots: u32,
    retrievals: BTreeMap<String, (f64, Option<Tile>)>,
}
impl<'a> Simulation<'a> {
    fn new(s: &'a FarmSnapshot, c: &'a Constraints, end: u32) -> Self {
        let mut stock = BTreeMap::new();
        let mut retrievals = BTreeMap::new();
        let mut warnings = BTreeSet::new();
        let mut held = Value::default();
        for item in &s.items {
            if item.source.starts_with("farm_chest:") {
                let already: f64 = retrievals.values().map(|(m, _)| m).sum();
                let available =
                    (minutes(c.sleep_at) - minutes(s.time_of_day) - c.travel_buffer_minutes)
                        .max(0.0)
                        .min(c.daily_minutes.unwrap_or(f64::INFINITY));
                if !retrievals.contains_key(&item.source)
                    && already + item.retrieval_minutes > available * 0.5
                {
                    warnings.insert("Some chest contents are excluded because retrieving them would consume too much of today's estimated work budget.".into());
                    continue;
                }
                retrievals
                    .entry(item.source.clone())
                    .or_insert((item.retrieval_minutes, item.source_tile));
            }
            if item.is_crop {
                held.add(Value {
                    low: item.sale_price * item.quantity as i64,
                    expected: (item.sale_price * item.quantity as i64) as f64,
                });
            } else {
                *stock.entry(item.item_id.clone()).or_default() += item.quantity;
            }
        }
        let plots = s
            .plots
            .iter()
            .filter(|p| p.reachable)
            .map(|p| {
                let crop = p
                    .crop
                    .as_ref()
                    .filter(|c| !c.dead)
                    .and_then(|g| s.crops.iter().position(|c| c.seed_id == g.seed_id));
                SimPlot {
                    plot: p.clone(),
                    crop,
                    due: s.day + p.crop.as_ref().map(|c| c.days_to_harvest).unwrap_or(0),
                    active: p.crop.as_ref().is_some_and(|c| !c.dead),
                    planted_today: false,
                }
            })
            .collect();
        Self {
            s,
            c,
            end,
            plots,
            stock,
            money: s.money,
            expected_money: s.money as f64,
            held,
            shipped: Value::default(),
            bought: BTreeMap::new(),
            warnings,
            days: Vec::new(),
            added_plots: 0,
            retrievals,
        }
    }
    fn budget(&self, day: u32) -> (f64, f64) {
        let energy = self
            .c
            .daily_energy
            .unwrap_or(self.s.max_energy)
            .min(if day == self.s.day {
                self.s.energy
            } else {
                self.s.max_energy
            });
        let available = (minutes(self.c.sleep_at)
            - minutes(if day == self.s.day {
                self.s.time_of_day
            } else {
                600
            })
            - self.c.travel_buffer_minutes)
            .max(0.0);
        (
            energy,
            available.min(self.c.daily_minutes.unwrap_or(available)),
        )
    }
    fn rain(&self, day: u32) -> bool {
        self.s.known_weather.iter().any(|w| w.day == day && w.rain)
    }
    fn manual_count(&self, day: u32) -> u32 {
        if self.rain(day) {
            return 0;
        }
        self.plots
            .iter()
            .filter(|p| {
                p.active
                    && p.crop.is_some_and(|i| self.s.crops[i].needs_watering)
                    && !p.plot.sprinkler_covered
                    && !(day == self.s.day && p.plot.watered_today)
            })
            .count() as u32
    }
    fn shop_usable(&self, shop: &ShopDay, day: u32, labor: f64) -> bool {
        shop.day == day
            && minutes(shop.closes_at)
                > minutes(if day == self.s.day {
                    self.s.time_of_day
                } else {
                    600
                }) + labor
                    + shop.travel_minutes / 2.0
            && minutes(shop.opens_at) < minutes(self.c.sleep_at)
    }
    fn visit_cost(&self, shop: &ShopDay, day: u32, labor: f64, visits: &BTreeSet<String>) -> f64 {
        if visits.contains(&shop.shop) {
            return 0.0;
        }
        let arrival = minutes(if day == self.s.day {
            self.s.time_of_day
        } else {
            600
        }) + labor
            + shop.travel_minutes / 2.0;
        let normal = shop.travel_minutes + (minutes(shop.opens_at) - arrival).max(0.0);
        let start = minutes(if day == self.s.day {
            self.s.time_of_day
        } else {
            600
        }) + labor;
        normal.max(if shop.return_not_before > 0 {
            (minutes(shop.return_not_before) - start).max(0.0)
        } else {
            0.0
        })
    }
    fn offer(
        &self,
        item: &str,
        day: u32,
        labor: f64,
        visits: &BTreeSet<String>,
    ) -> Option<(ShopDay, Offer)> {
        // Purchases from an already visited shop are grouped into that visit.
        // No later same-day harvest receipts are used during this phase.
        self.s
            .shops
            .iter()
            .filter(|s| {
                s.day == day && (visits.contains(&s.shop) || self.shop_usable(s, day, labor))
            })
            .flat_map(|shop| {
                shop.offers
                    .iter()
                    .filter(|o| {
                        o.item_id == item
                            && o.stock != 0
                            && (o.stock < 0
                                || self
                                    .bought
                                    .get(&(day, shop.shop.clone(), item.into()))
                                    .copied()
                                    .unwrap_or(0)
                                    < o.stock as u32)
                    })
                    .map(move |o| (shop, o))
            })
            .min_by_key(|(_, o)| o.price)
            .map(|(shop, offer)| (shop.clone(), offer.clone()))
    }
    fn buy_one(
        &mut self,
        shop: &ShopDay,
        offer: &Offer,
        day: u32,
        actions: &mut Vec<Action>,
        visits: &mut BTreeSet<String>,
        labor: &mut f64,
    ) {
        self.money -= offer.price;
        self.expected_money -= offer.price as f64;
        *self.stock.entry(offer.item_id.clone()).or_default() += 1;
        *self
            .bought
            .entry((day, shop.shop.clone(), offer.item_id.clone()))
            .or_default() += 1;
        let visit = self.visit_cost(shop, day, *labor, visits);
        if visits.insert(shop.shop.clone()) {
            *labor += visit;
        }
        *labor += 0.25;
        actions.push(action(
            "buy",
            &offer.item_id,
            &offer.name,
            None,
            offer.price,
            &format!(
                "Buy from {} during {:04}–{:04}; budget uses conservative cash",
                shop.shop, shop.opens_at, shop.closes_at
            ),
        ));
    }
    fn sell(
        &mut self,
        day: u32,
        actions: &mut Vec<Action>,
        visits: &mut BTreeSet<String>,
        labor: &mut f64,
    ) {
        if self.held.low == 0 && self.held.expected == 0.0 {
            return;
        }
        let (_, max_minutes) = self.budget(day);
        let shop = self
            .s
            .shops
            .iter()
            .filter(|shop| shop.buys_crops && self.shop_usable(shop, day, *labor))
            .filter(|shop| *labor + self.visit_cost(shop, day, *labor, visits) + 2.0 <= max_minutes)
            .min_by(|a, b| a.travel_minutes.total_cmp(&b.travel_minutes))
            .cloned();
        if let Some(shop) = shop {
            let visit = self.visit_cost(&shop, day, *labor, visits);
            if visits.insert(shop.shop.clone()) {
                *labor += visit;
            }
            *labor += 2.0;
            actions.push(action(
                "sell",
                "crop_harvests",
                "Available crop harvests",
                None,
                self.held.low,
                &format!(
                    "Sell directly to {} for immediate cash; expected receipt {:.0}g",
                    shop.shop, self.held.expected
                ),
            ));
            self.money += self.held.low;
            self.expected_money += self.held.expected;
            self.held = Value::default();
        } else if day < self.end && *labor + 5.0 <= max_minutes {
            actions.push(action(
                "ship",
                "crop_harvests",
                "Available crop harvests",
                None,
                self.held.low,
                "Put in shipping bin; payment is available tomorrow, never today",
            ));
            *labor += 5.0;
            self.shipped.add(self.held);
            self.held = Value::default();
        }
    }
    fn invest_sprinkler(
        &mut self,
        day: u32,
        energy: &mut f64,
        labor: &mut f64,
        actions: &mut Vec<Action>,
        visits: &mut BTreeSet<String>,
        deadline: Instant,
    ) {
        if !self.c.allow_sprinkler_investment || day >= self.end {
            return;
        }
        let (_, max_minutes) = self.budget(day);
        let mut best: Option<SprinklerChoice> = None;
        for (oi, o) in self.s.sprinklers.iter().enumerate() {
            if Instant::now() >= deadline {
                break;
            }
            let owned = self.stock.get(&o.item_id).copied().unwrap_or(0) > 0;
            let purchase = self.offer(&o.item_id, day, *labor, visits);
            let mut buys = Vec::new();
            let mut craft = false;
            let mut cost = 0i64;
            if !owned {
                cost = i64::MAX;
                if let Some((shop, offer)) = purchase {
                    cost = offer.price;
                    buys.push((shop, offer, 1));
                }
                if o.recipe_unlocked && !o.materials.is_empty() {
                    let mut craft_buys = Vec::new();
                    let mut craft_cost = 0;
                    let mut possible = true;
                    for m in &o.materials {
                        let missing = m
                            .quantity
                            .saturating_sub(self.stock.get(&m.item_id).copied().unwrap_or(0));
                        if missing > 0 {
                            if let Some((shop, offer)) = self.offer(&m.item_id, day, *labor, visits)
                            {
                                let remaining = if offer.stock < 0 {
                                    u32::MAX
                                } else {
                                    (offer.stock as u32).saturating_sub(
                                        self.bought
                                            .get(&(day, shop.shop.clone(), m.item_id.clone()))
                                            .copied()
                                            .unwrap_or(0),
                                    )
                                };
                                if remaining < missing {
                                    possible = false;
                                    break;
                                }
                                craft_cost += offer.price * missing as i64;
                                craft_buys.push((shop, offer, missing));
                            } else {
                                possible = false;
                                break;
                            }
                        }
                    }
                    if possible && craft_cost < cost {
                        cost = craft_cost;
                        buys = craft_buys;
                        craft = true;
                    }
                }
                if cost == i64::MAX {
                    continue;
                }
            }
            if cost > self.money - self.c.reserve_gold {
                continue;
            }
            let mut extra_visits = BTreeSet::new();
            let travel: f64 = buys
                .iter()
                .filter(|(shop, _, _)| {
                    !visits.contains(&shop.shop) && extra_visits.insert(shop.shop.clone())
                })
                .map(|(shop, _, _)| self.visit_cost(shop, day, *labor, visits))
                .sum();
            if *labor + travel + 5.0 > max_minutes {
                continue;
            }
            // Sprinklers occupy an empty, already clear farm tile. They do not
            // water on the placement day. Never assume a crop can share it.
            for center in self
                .plots
                .iter()
                .filter(|p| !p.active && p.plot.tilled && p.plot.obstruction.is_empty())
            {
                if Instant::now() >= deadline {
                    break;
                }
                let t = center.plot.tile.as_ref().unwrap();
                let covered: Vec<usize> =
                    self.plots
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| {
                            !p.plot.sprinkler_covered
                                && p.plot.reachable
                                && (p.plot.tilled || p.plot.clearable)
                                && o.coverage_offsets.iter().any(|off| {
                                    p.plot.tile.as_ref().is_some_and(|pt| {
                                        pt.x == t.x + off.x && pt.y == t.y + off.y
                                    })
                                })
                        })
                        .map(|(i, _)| i)
                        .collect();
                let crop_best = self
                    .s
                    .crops
                    .iter()
                    .filter(|c| available_crop(c, self.s))
                    .map(|c| {
                        remaining_value(
                            c,
                            &center.plot,
                            self.s,
                            day + 1
                                + growth_days(
                                    c,
                                    center.plot.speed_bonus,
                                    self.s.professions.contains(&5),
                                ),
                            self.end,
                        )
                        .expected
                    })
                    .fold(0.0, f64::max);
                let daily_capacity = self
                    .c
                    .watering_capacity
                    .unwrap_or(self.s.estimated_watering_capacity);
                let potential = (self.manual_count(day)
                    + self.plots.iter().filter(|p| !p.active).count() as u32)
                    .saturating_sub(daily_capacity) as usize;
                let gain = (covered.len().min(potential) as f64) * crop_best - cost as f64;
                if !covered.is_empty()
                    && (owned || gain > 0.0)
                    && best.as_ref().is_none_or(|b| gain > b.0)
                {
                    best = Some((gain, oi, *t, covered, buys.clone(), craft));
                }
            }
        }
        if let Some((_, oi, tile, covered, buys, craft)) = best {
            let o = self.s.sprinklers[oi].clone();
            for (shop, offer, n) in buys {
                for _ in 0..n {
                    self.buy_one(&shop, &offer, day, actions, visits, labor);
                }
            }
            if craft {
                for m in &o.materials {
                    *self.stock.entry(m.item_id.clone()).or_default() -= m.quantity;
                }
                *self.stock.entry(o.item_id.clone()).or_default() += 1;
                actions.push(action("craft",&o.item_id,&o.name,None,0,"Use the unlocked recipe and listed farm resources; crafting inputs are consumed"));
            }
            *self.stock.entry(o.item_id.clone()).or_default() -= 1;
            for i in covered {
                self.plots[i].plot.sprinkler_covered = true;
            }
            if let Some(p) = self
                .plots
                .iter_mut()
                .find(|p| p.plot.tile.as_ref() == Some(&tile))
            {
                p.plot.reachable = false;
                p.plot.tilled = false;
                p.plot.clearable = false;
            }
            *energy += 0.0;
            *labor += 5.0;
            actions.push(action("place_sprinkler",&o.item_id,&o.name,Some(&tile),0,"Place on this empty tile. Water newly covered crops manually today; automatic watering starts tomorrow"));
            self.warnings.insert("Sprinkler placement is a heuristic; its estimated benefit is verified by complete daily simulation, not an optimal placement proof.".into());
        }
    }
    fn run(mut self, policy: usize, sprinkler: bool, deadline: Instant) -> Plan {
        for day in self.s.day..=self.end {
            self.money += self.shipped.low;
            self.expected_money += self.shipped.expected;
            self.shipped = Value::default();
            let opening = self.money;
            let mut energy = 0.0;
            let mut labor = 0.0;
            let mut actions = Vec::new();
            let mut visits = BTreeSet::new();
            let (max_energy, max_minutes) = self.budget(day);
            if day == self.s.day {
                for (chest, (time, tile)) in &self.retrievals {
                    labor += time;
                    actions.push(action("retrieve_resources",chest,"Farm chest resources",tile.as_ref(),0,"Retrieve usable resources from this accessible chest; travel and handling time are included"));
                }
            }
            for p in &mut self.plots {
                p.planted_today = false;
                if !p.active || p.due > day {
                    continue;
                }
                let Some(i) = p.crop else {
                    continue;
                };
                let crop = &self.s.crops[i];
                if !crop.supported || crop.forage_crop {
                    self.warnings.insert(format!(
                        "Existing {} harvest is excluded because its mechanics are unsupported",
                        crop.name
                    ));
                    continue;
                }
                if labor + 1.0 > max_minutes {
                    self.warnings.insert("Harvesting exceeds the estimated work budget; some crops are left for a later day.".into());
                    continue;
                }
                labor += 1.0;
                self.held.add(value(crop, &p.plot, self.s));
                actions.push(action(
                    "harvest",
                    &crop.harvest_id,
                    &crop.name,
                    p.plot.tile.as_ref(),
                    0,
                    "Harvest is ready; quality and extra yield are reported as estimates",
                ));
                if crop.regrow_days > 0 {
                    p.due = day + crop.regrow_days;
                } else {
                    p.active = false;
                    p.crop = None;
                }
            }
            self.sell(day, &mut actions, &mut visits, &mut labor);
            let coverage_before: Vec<bool> = self
                .plots
                .iter()
                .map(|p| {
                    p.plot.sprinkler_covered
                        && p.plot.tilled
                        && (day != self.s.day || p.plot.watered_today)
                })
                .collect();
            if sprinkler && Instant::now() < deadline {
                self.invest_sprinkler(
                    day,
                    &mut energy,
                    &mut labor,
                    &mut actions,
                    &mut visits,
                    deadline,
                );
            }
            let capacity = self
                .c
                .watering_capacity
                .unwrap_or(self.s.estimated_watering_capacity);
            // Plant one seed at a time, selecting marginal season revenue with
            // existing crop opportunity cost, current inventory, and daily cash.
            let mut rejected = BTreeSet::new();
            loop {
                if policy == 4 {
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                let current_manual = self
                    .plots
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| {
                        p.active
                            && p.crop.is_some_and(|j| self.s.crops[j].needs_watering)
                            && !self.rain(day)
                            && !(coverage_before[*i] || (day == self.s.day && p.plot.watered_today))
                    })
                    .count() as u32;
                let mut best: Option<CropChoice> = None;
                let future_count = self
                    .plots
                    .iter()
                    .filter(|p| {
                        p.active
                            && p.crop.is_some_and(|i| self.s.crops[i].needs_watering)
                            && !p.plot.sprinkler_covered
                    })
                    .count() as u32;
                let active_count = self.plots.iter().filter(|p| p.active).count();
                let future_energy = self.c.daily_energy.unwrap_or(self.s.max_energy);
                let future_minutes = self
                    .c
                    .daily_minutes
                    .unwrap_or(minutes(self.c.sleep_at) - 360.0 - self.c.travel_buffer_minutes);
                let offers: Vec<_> = self
                    .s
                    .crops
                    .iter()
                    .map(|crop| {
                        if self.stock.get(&crop.seed_id).copied().unwrap_or(0) > 0 {
                            None
                        } else {
                            self.offer(&crop.seed_id, day, labor, &visits)
                        }
                    })
                    .collect();
                for (pi, p) in self.plots.iter().enumerate() {
                    if pi % 64 == 0 && Instant::now() >= deadline {
                        break;
                    }
                    if !p.plot.reachable || p.planted_today || rejected.contains(&pi) {
                        continue;
                    }
                    if !p.plot.tilled
                        && (!self.c.allow_clearing
                            || !p.plot.clearable
                            || self.added_plots >= self.c.max_new_plots)
                    {
                        continue;
                    }
                    if p.active && !self.c.allow_replacement {
                        continue;
                    }
                    if p.active && p.crop.is_none() {
                        continue;
                    }
                    if p.active && p.crop.is_some_and(|i| !self.s.crops[i].supported) {
                        continue;
                    }
                    let old = p
                        .crop
                        .map(|i| {
                            remaining_value(&self.s.crops[i], &p.plot, self.s, p.due, self.end)
                        })
                        .unwrap_or_default();
                    for (ci, crop) in self
                        .s
                        .crops
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| available_crop(c, self.s))
                    {
                        let due = day
                            + growth_days(
                                crop,
                                p.plot.speed_bonus,
                                self.s.professions.contains(&5),
                            );
                        if due > self.end {
                            continue;
                        }
                        let mut new = remaining_value(crop, &p.plot, self.s, due, self.end);
                        if !p.plot.tilled {
                            new = remaining_value(
                                crop,
                                &Plot {
                                    quality_fertilizer: 0,
                                    speed_bonus: 0.0,
                                    ..p.plot.clone()
                                },
                                self.s,
                                day + growth_days(crop, 0.0, self.s.professions.contains(&5)),
                                self.end,
                            );
                        }
                        let seed_owned = self.stock.get(&crop.seed_id).copied().unwrap_or(0) > 0;
                        let offer = &offers[ci];
                        if !seed_owned && offer.is_none() {
                            continue;
                        }
                        let cost = offer.as_ref().map(|(_, o)| o.price).unwrap_or(0);
                        if cost > self.money - self.c.reserve_gold {
                            continue;
                        }
                        let gain = new.expected - cost as f64 - old.expected;
                        if gain <= 0.0 || new.low - cost - old.low < 0 {
                            continue;
                        }
                        let already_manual = p.active
                            && p.crop.is_some_and(|i| self.s.crops[i].needs_watering)
                            && !coverage_before[pi]
                            && !(day == self.s.day && p.plot.watered_today)
                            && !self.rain(day);
                        let manual_today = crop.needs_watering
                            && !coverage_before[pi]
                            && !self.rain(day)
                            && !(day == self.s.day && p.plot.watered_today);
                        let future_manual = crop.needs_watering && !p.plot.sprinkler_covered;
                        let old_future = p.active
                            && p.crop.is_some_and(|i| self.s.crops[i].needs_watering)
                            && !p.plot.sprinkler_covered;
                        if (manual_today && !already_manual && current_manual >= capacity)
                            || (future_manual && !old_future && future_count >= capacity)
                        {
                            continue;
                        }
                        let extra_energy = if !p.plot.tilled {
                            p.plot.clearing_energy + 2.0
                        } else {
                            0.0
                        };
                        let dead = p.plot.crop.as_ref().is_some_and(|g| g.dead);
                        let extra_minutes =
                            0.5 + if !p.plot.tilled {
                                p.plot.clearing_minutes + 1.0
                            } else {
                                0.0
                            } + if p.active || dead { 1.0 } else { 0.0 };
                        let travel = offer
                            .as_ref()
                            .map(|(shop, _)| self.visit_cost(shop, day, labor, &visits) + 0.25)
                            .unwrap_or(0.0);
                        let water_count = current_manual.saturating_sub(already_manual as u32)
                            + manual_today as u32;
                        if energy
                            + extra_energy
                            + water_count as f64 * self.s.watering_energy_per_tile
                            > max_energy
                            || labor
                                + extra_minutes
                                + travel
                                + water_count as f64 * self.s.watering_minutes_per_tile
                                > max_minutes
                        {
                            continue;
                        }
                        // Reserve future watering energy and labor (including one
                        // minute per crop for a possible shared harvest day).
                        let future_water =
                            future_count.saturating_sub(old_future as u32) + future_manual as u32;
                        let future_active = active_count + (!p.active) as usize;
                        if future_water as f64 * self.s.watering_energy_per_tile > future_energy
                            || future_water as f64 * self.s.watering_minutes_per_tile
                                + future_active as f64
                                + 60.0
                                > future_minutes
                        {
                            continue;
                        }
                        let score = match policy {
                            0 => gain,
                            1 => gain / (cost.max(1) as f64),
                            2 => {
                                gain / (growth_days(
                                    crop,
                                    p.plot.speed_bonus,
                                    self.s.professions.contains(&5),
                                )
                                .max(1) as f64)
                            }
                            _ => {
                                gain / (if future_manual {
                                    self.s.watering_energy_per_tile.max(0.1)
                                } else {
                                    0.1
                                })
                            }
                        };
                        if best.as_ref().is_none_or(|b| score > b.0) {
                            best =
                                Some((score, pi, ci, offer.clone(), extra_energy, extra_minutes));
                        }
                    }
                }
                let Some((_, pi, ci, offer, extra_energy, extra_minutes)) = best else {
                    break;
                };
                if let Some((shop, o)) = offer {
                    self.buy_one(&shop, &o, day, &mut actions, &mut visits, &mut labor);
                }
                let crop = &self.s.crops[ci];
                let p = &mut self.plots[pi];
                if p.active {
                    actions.push(action("remove_crop","","Existing crop",p.plot.tile.as_ref(),0,"Replacement increases modeled deadline earnings after the value of remaining harvests and seed cost"));
                } else if p.plot.crop.as_ref().is_some_and(|g| g.dead) {
                    actions.push(action(
                        "remove_crop",
                        "",
                        "Dead crop",
                        p.plot.tile.as_ref(),
                        0,
                        "Remove the dead crop before planting; time is included",
                    ));
                }
                if !p.plot.tilled {
                    if !p.plot.obstruction.is_empty() {
                        actions.push(action("clear","",&p.plot.obstruction,p.plot.tile.as_ref(),0,"Clear only this reachable tile; collected materials are not counted as income"));
                    }
                    actions.push(action("till","","Soil",p.plot.tile.as_ref(),0,"Prepare this explicitly modeled plot within the estimated time and energy budget"));
                    self.added_plots += 1;
                    p.plot.tilled = true;
                    p.plot.quality_fertilizer = 0;
                    p.plot.speed_bonus = 0.0;
                }
                *self.stock.entry(crop.seed_id.clone()).or_default() -= 1;
                p.crop = Some(ci);
                p.active = true;
                p.planted_today = true;
                p.due =
                    day + growth_days(crop, p.plot.speed_bonus, self.s.professions.contains(&5));
                p.plot.crop = None;
                energy += extra_energy;
                labor += extra_minutes;
                actions.push(action("plant",&crop.seed_id,&crop.name,p.plot.tile.as_ref(),0,"Use an owned or purchased seed; harvest dates include existing fertilizer and Agriculturist"));
                rejected.insert(pi);
            }
            let mut watering = 0;
            for (pi, p) in self.plots.iter_mut().enumerate() {
                if !p.active
                    || !p.crop.is_some_and(|i| self.s.crops[i].needs_watering)
                    || self.s.known_weather.iter().any(|w| w.day == day && w.rain)
                    || coverage_before[pi]
                    || (day == self.s.day && p.plot.watered_today)
                {
                    continue;
                }
                if watering < capacity
                    && energy + self.s.watering_energy_per_tile <= max_energy
                    && labor + self.s.watering_minutes_per_tile <= max_minutes
                {
                    watering += 1;
                    energy += self.s.watering_energy_per_tile;
                    labor += self.s.watering_minutes_per_tile;
                    actions.push(action("water","","Crop plots",p.plot.tile.as_ref(),0,"Water manually; includes newly placed sprinklers that will activate tomorrow"));
                } else {
                    p.due += 1;
                    self.warnings.insert("Existing crop demand exceeds the work/watering budget. Listed unwatered crops have their harvest delayed; override capacity if the estimate is too low.".into());
                    actions.push(action(
                        "leave_unwatered",
                        "",
                        "Crop plot",
                        p.plot.tile.as_ref(),
                        0,
                        "Daily budget exhausted; modeled harvest is delayed by one day",
                    ));
                }
            }
            self.days.push(DailyPlan {
                day,
                opening_cash_conservative: opening,
                closing_cash_conservative: self.money,
                closing_cash_expected: self.expected_money,
                watering_tiles: watering,
                energy_used: energy,
                minutes_used: labor,
                actions: compact(actions),
            });
        }
        Plan {
            id: uuid::Uuid::new_v4().to_string(),
            save_id: self.s.save_id.clone(),
            player_id: self.s.player_id.clone(),
            year: self.s.year,
            season: self.s.season.clone(),
            snapshot_at_unix_ms: self.s.captured_at_unix_ms,
            source_snapshot_request_id: self.s.refresh_request_id.clone(),
            horizon_days: self.end - self.s.day + 1,
            deadline_day: self.end,
            constraints: self.c.clone(),
            optimality: "feasible_estimate".into(),
            strategy: format!("policy_{policy};sprinklers={sprinkler}"),
            evaluated_candidates: 0,
            calculation_ms: 0,
            starting_cash: self.s.money,
            expected_final_cash: self.expected_money,
            conservative_final_cash: self.money,
            unsold_crop_value_expected: self.held.expected,
            assumptions: Vec::new(),
            warnings: self.warnings.into_iter().collect(),
            days: self.days,
        }
    }
}

pub fn plan(s: &FarmSnapshot, h: u32, c: &Constraints) -> Result<Plan> {
    let end = validate_constraints(s, h, c)?;
    let start = Instant::now();
    let deadline = start + Duration::from_millis(c.time_limit_ms);
    let baseline = Simulation::new(s, c, end).run(4, false, deadline);
    let mut best = if baseline.days.len() == (end - s.day + 1) as usize {
        Some(baseline)
    } else {
        None
    };
    let mut evaluated = best.is_some() as u32;
    for sprinklers in [false, true] {
        if sprinklers && !c.allow_sprinkler_investment {
            continue;
        }
        for policy in 0..4 {
            let candidate = Simulation::new(s, c, end).run(policy, sprinklers, deadline);
            if candidate.days.len() != (end - s.day + 1) as usize {
                continue;
            }
            evaluated += 1;
            if best
                .as_ref()
                .is_none_or(|p| candidate.expected_final_cash > p.expected_final_cash)
            {
                best = Some(candidate);
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    let Some(mut p) = best else {
        bail!(
            "Planning time limit expired before a full plan was evaluated; increase time_limit_ms or reduce max_new_plots"
        );
    };
    p.evaluated_candidates = evaluated;
    p.calculation_ms = start.elapsed().as_millis();
    p.assumptions = s.assumptions.clone();
    p.assumptions.extend([
        "Today is included in horizon_days; deadlines never extend beyond day 28.".into(),
        "Objective is expected cash actually received by the deadline. Purchases use conservative receipts, not speculative quality bonuses.".into(),
        "Conservative earnings assume minimum harvest quantity and minimum modeled quality, with no crop loss. They are not a guarantee against lightning, crows, missed actions, or incorrect time estimates.".into(),
        "Manual watering is required unless sprinkler coverage or observed weather is known. New sprinklers activate the following morning.".into(),
        "Travel and task duration are conservative estimates. Shop calendars and current unlocks come from the snapshot; future unlocks and skill gains are not predicted.".into(),
        "Existing inventory and accessible farm chests may fund farming; only crops are sold. Food purchases and processing machines are excluded.".into(),
        "Trellis routing, forage crops, giant crops, rare luck doubles, retaining-soil randomness, mixed seeds, crop mutation, and nonstandard growth mechanics are excluded from new planting.".into(),
        "Search compares multiple greedy reinvestment policies and sprinkler alternatives; no global optimality proof is provided.".into(),
    ]);
    p.warnings.extend(s.warnings.clone());
    if start.elapsed() >= Duration::from_millis(c.time_limit_ms) {
        p.warnings.push("Search time limit reached. The returned plan still simulates every day through the deadline, but some investment alternatives were not explored.".into());
    }
    if s.plots.iter().any(|p| {
        p.crop
            .as_ref()
            .is_some_and(|g| !g.dead && !s.crops.iter().any(|c| c.seed_id == g.seed_id))
    }) {
        p.warnings.push("Some existing crops are absent from the crop catalog and cannot be valued or scheduled.".into());
    }
    Ok(p)
}

pub fn render(p: &Plan, details: bool) -> String {
    let mut text = format!(
        "{} {}–{} | expected final cash {:.0}g | conservative {}g\n{}; {} candidates; {}ms\n",
        p.season,
        p.days.first().map(|d| d.day).unwrap_or(0),
        p.deadline_day,
        p.expected_final_cash,
        p.conservative_final_cash,
        p.optimality,
        p.evaluated_candidates,
        p.calculation_ms
    );
    for d in &p.days {
        text.push_str(&format!(
            "\nDay {}: cash {}g → {}g (expected {:.0}g); water {} tiles\n",
            d.day,
            d.opening_cash_conservative,
            d.closing_cash_conservative,
            d.closing_cash_expected,
            d.watering_tiles
        ));
        for a in &d.actions {
            text.push_str(&format!(
                "  - {} {} {}{}\n",
                a.kind,
                a.quantity,
                if a.kind == "harvest" {
                    format!("plots of {}", a.name)
                } else {
                    format!("× {}", a.name)
                },
                if a.gold > 0 {
                    format!(" ({}g)", a.gold)
                } else {
                    String::new()
                }
            ));
            if details {
                text.push_str(&format!(
                    "    {}\n    Tiles: {}\n",
                    a.reason,
                    a.tiles
                        .iter()
                        .map(|t| format!("({}, {})", t.x, t.y))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if details {
            text.push_str(&format!(
                "  Estimated work: {:.1} energy; {:.1} game minutes\n",
                d.energy_used, d.minutes_used
            ));
        }
    }
    if details {
        for a in &p.assumptions {
            text.push_str(&format!("Assumption: {a}\n"));
        }
    }
    for w in &p.warnings {
        text.push_str(&format!("Warning: {w}\n"));
    }
    text
}
