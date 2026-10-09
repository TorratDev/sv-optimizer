use sv_optimizer::{
    planner::{self, Constraints},
    protocol::*,
};
fn farm() -> FarmSnapshot {
    serde_json::from_str(include_str!("../fixtures/spring-demo.json")).unwrap()
}
fn last_day() -> FarmSnapshot {
    let mut s = farm();
    s.day = 28;
    s.time_of_day = 900;
    s.money = 0;
    s.plots.truncate(1);
    s.plots[0].crop = Some(GrowingCrop {
        seed_id: "(O)472".into(),
        days_to_harvest: 0,
        dead: false,
    });
    s
}
#[test]
fn ordinary_farm_has_finance_and_work_feasible_plan() {
    let s = farm();
    let p = planner::plan(&s, 28, &Constraints::default()).unwrap();
    assert!(p.expected_final_cash >= p.conservative_final_cash as f64);
    assert!(p.conservative_final_cash > s.money);
    assert_eq!(p.optimality, "feasible_estimate");
    assert_eq!(p.days.len(), 28);
    for day in &p.days {
        assert!(day.closing_cash_conservative >= 0);
        assert!(day.watering_tiles <= s.estimated_watering_capacity);
        assert!(day.energy_used <= s.max_energy);
        assert!(day.minutes_used <= 1020.0);
    }
}
#[test]
fn immediate_shop_sale_counts_on_last_day() {
    let p = planner::plan(&last_day(), 1, &Constraints::default()).unwrap();
    assert_eq!(p.conservative_final_cash, 35);
    assert!(p.days[0].actions.iter().any(|a| a.kind == "sell"));
}
#[test]
fn last_day_shipping_is_not_deadline_cash() {
    let mut s = last_day();
    s.shops.clear();
    let p = planner::plan(&s, 1, &Constraints::default()).unwrap();
    assert_eq!(p.conservative_final_cash, 0);
    assert!(p.unsold_crop_value_expected >= 35.0);
    assert!(!p.days[0].actions.iter().any(|a| a.kind == "ship"));
}
#[test]
fn shipping_cannot_fund_same_day_purchases() {
    let mut s = farm();
    s.money = 0;
    s.day = 4;
    s.plots.truncate(2);
    s.plots[0].crop = Some(GrowingCrop {
        seed_id: "(O)472".into(),
        days_to_harvest: 0,
        dead: false,
    });
    s.shops.retain(|shop| shop.day != 4);
    let p = planner::plan(&s, 8, &Constraints::default()).unwrap();
    assert_eq!(p.days[0].closing_cash_conservative, 0);
    assert!(p.days[0].actions.iter().any(|a| a.kind == "ship"));
    assert!(!p.days[0].actions.iter().any(|a| a.kind == "buy"));
    assert_eq!(p.days[1].opening_cash_conservative, 35);
}
#[test]
fn no_end_of_season_seed_waste() {
    let mut s = last_day();
    s.money = 500;
    s.plots[0].crop = None;
    let p = planner::plan(&s, 28, &Constraints::default()).unwrap();
    assert_eq!(p.deadline_day, 28);
    assert_eq!(p.days.len(), 1);
    assert_eq!(p.conservative_final_cash, 500);
    assert!(!p.days[0].actions.iter().any(|a| a.kind == "plant"));
}
#[test]
fn harvest_date_includes_planting_day_correctly() {
    let mut s = farm();
    s.plots.truncate(1);
    s.crops.truncate(1);
    s.money = 20;
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    assert!(p.days[0].actions.iter().any(|a| a.kind == "plant"));
    assert!(p.days[4].actions.iter().any(|a| a.kind == "harvest"));
    assert!(
        !p.days[..4]
            .iter()
            .any(|d| d.actions.iter().any(|a| a.kind == "harvest"))
    );
}
#[test]
fn zero_watering_capacity_prevents_new_manual_crops() {
    let s = farm();
    let c = Constraints {
        watering_capacity: Some(0),
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    assert!(
        p.days
            .iter()
            .all(|d| !d.actions.iter().any(|a| a.kind == "plant"))
    );
}
#[test]
fn established_sprinklers_allow_crops_with_zero_manual_capacity() {
    let mut s = farm();
    for p in &mut s.plots {
        p.sprinkler_covered = true;
        p.watered_today = true;
    }
    let c = Constraints {
        watering_capacity: Some(0),
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    assert!(p.days[0].actions.iter().any(|a| a.kind == "plant"));
    assert!(p.days.iter().all(|d| d.watering_tiles == 0));
}
#[test]
fn new_sprinklers_do_not_water_on_placement_day() {
    let mut s = farm();
    s.crops.truncate(1);
    s.money = 500;
    s.sprinklers.push(SprinklerOption {
        item_id: "(O)621".into(),
        name: "Quality Sprinkler".into(),
        coverage_offsets: vec![
            Tile { x: 1, y: 0 },
            Tile { x: 0, y: 1 },
            Tile { x: 1, y: 1 },
        ],
        ..Default::default()
    });
    s.items.push(ItemStack {
        item_id: "(O)621".into(),
        quantity: 1,
        source: "inventory".into(),
        ..Default::default()
    });
    let c = Constraints {
        watering_capacity: Some(1),
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    for d in &p.days {
        assert!(d.watering_tiles <= 1);
    }
    // Any first-day plants under a newly placed sprinkler still require manual
    // watering today; there is no free placement-day crop growth.
    if p.days[0]
        .actions
        .iter()
        .any(|a| a.kind == "place_sprinkler")
    {
        let plants: u32 = p.days[0]
            .actions
            .iter()
            .filter(|a| a.kind == "plant")
            .map(|a| a.quantity)
            .sum();
        assert!(plants <= 1);
    }
}
#[test]
fn future_rain_is_not_assumed() {
    let mut s = farm();
    s.plots.truncate(1);
    s.crops.truncate(1);
    s.money = 20;
    s.known_weather.push(WeatherDay { day: 2, rain: true });
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    assert_eq!(p.days[1].watering_tiles, 0);
    assert_eq!(p.days[2].watering_tiles, 1);
}
#[test]
fn preserve_crop_constraint_is_respected() {
    let mut s = farm();
    s.plots.truncate(1);
    s.plots[0].crop = Some(GrowingCrop {
        seed_id: "(O)472".into(),
        days_to_harvest: 3,
        dead: false,
    });
    let c = Constraints {
        allow_replacement: false,
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    assert!(
        p.days
            .iter()
            .all(|d| !d.actions.iter().any(|a| a.kind == "remove_crop"))
    );
}
#[test]
fn unknown_existing_crops_are_not_overwritten() {
    let mut s = farm();
    s.plots.truncate(1);
    s.plots[0].crop = Some(GrowingCrop {
        seed_id: "unknown".into(),
        days_to_harvest: 1,
        dead: false,
    });
    let p = planner::plan(&s, 28, &Constraints::default()).unwrap();
    assert!(p.days.iter().all(|d| {
        !d.actions
            .iter()
            .any(|a| a.kind == "plant" || a.kind == "remove_crop")
    }));
}
#[test]
fn finite_seed_stock_is_shared_across_plots() {
    let mut s = farm();
    s.crops.truncate(1);
    s.shops.retain(|shop| shop.day == 1 || shop.day == 5);
    for shop in s.shops.iter_mut().filter(|shop| shop.day == 5) {
        shop.offers.clear();
    }
    s.shops[0].offers.retain(|o| o.item_id == "(O)472");
    s.shops[0].offers[0].stock = 2;
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    let bought: u32 = p.days[0]
        .actions
        .iter()
        .filter(|a| a.kind == "buy")
        .map(|a| a.quantity)
        .sum();
    assert_eq!(bought, 2);
}
#[test]
fn cash_reserve_is_preserved() {
    let s = farm();
    let c = Constraints {
        reserve_gold: 450,
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    assert!(p.days.iter().all(|d| d.closing_cash_conservative >= 450));
}
#[test]
fn regrowing_harvests_have_the_correct_interval() {
    let mut s = farm();
    s.plots.truncate(1);
    s.crops.truncate(1);
    s.crops[0].regrow_days = 3;
    s.money = 20;
    let p = planner::plan(&s, 11, &Constraints::default()).unwrap();
    let days: Vec<_> = p
        .days
        .iter()
        .filter(|d| d.actions.iter().any(|a| a.kind == "harvest"))
        .map(|d| d.day)
        .collect();
    assert_eq!(days, vec![5, 8, 11]);
}
#[test]
fn game_quality_prices_override_generic_tiller_prices() {
    let mut c = farm().crops[0].clone();
    c.sale_prices_by_quality = vec![17, 21, 26, 35];
    let (low, expected) = planner::harvest_revenue(&c, 0, 0, true);
    assert_eq!(low, 17);
    assert!((17.0..18.0).contains(&expected));
}
#[test]
fn blueberry_extra_items_are_normal_quality() {
    let c = CropDefinition {
        minimum_yield: 3,
        expected_yield: 3.0,
        base_sale_price: 50,
        ..Default::default()
    };
    let (low, expected) = planner::harvest_revenue(&c, 10, 3, false);
    assert_eq!(low, 62 + 100);
    assert!(expected <= 100.0 + 100.0);
}
#[test]
fn speed_reduction_can_remove_later_one_day_phases() {
    let c = CropDefinition {
        growth_phases: vec![1, 1, 1, 1],
        ..Default::default()
    };
    assert_eq!(planner::growth_days(&c, 0.25, false), 3);
    assert_eq!(planner::growth_days(&c, 0.25, true), 2);
}
#[test]
fn unreachable_or_nonclearable_land_is_preserved() {
    let mut s = farm();
    for p in &mut s.plots {
        p.reachable = false;
    }
    let p = planner::plan(&s, 28, &Constraints::default()).unwrap();
    assert_eq!(p.conservative_final_cash, s.money);
}
#[test]
fn chest_retrieval_is_a_real_action_and_cost() {
    let mut s = farm();
    s.money = 0;
    s.plots.truncate(1);
    s.crops.truncate(1);
    s.items.push(ItemStack {
        item_id: "(O)472".into(),
        quantity: 1,
        source: "farm_chest:5,5".into(),
        source_tile: Some(Tile { x: 5, y: 5 }),
        retrieval_minutes: 40.0,
        ..Default::default()
    });
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    assert!(
        p.days[0]
            .actions
            .iter()
            .any(|a| a.kind == "retrieve_resources")
    );
    assert!(p.days[0].minutes_used >= 40.0);
}
#[test]
fn bad_snapshots_and_constraints_fail_explicitly() {
    let mut s = farm();
    s.schema_version = 2;
    assert!(planner::plan(&s, 28, &Constraints::default()).is_err());
    s = farm();
    assert!(planner::plan(&s, 0, &Constraints::default()).is_err());
    s.single_player = false;
    assert!(planner::plan(&s, 28, &Constraints::default()).is_err());
    s = farm();
    s.plots.push(s.plots[0].clone());
    assert!(planner::validate_snapshot(&s).is_err());
}
#[test]
fn unsupported_trellis_crops_are_explained_in_comparison() {
    let result = planner::compare(&farm(), 28, &Constraints::default()).unwrap();
    assert!(
        result["excluded_crops"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["seed_id"] == "(O)473")
    );
}
#[test]
fn shops_do_not_pay_after_closing() {
    let mut s = last_day();
    s.time_of_day = 1700;
    let p = planner::plan(&s, 1, &Constraints::default()).unwrap();
    assert_eq!(p.conservative_final_cash, 0);
}
#[test]
fn shop_opening_wait_counts_against_daily_budget() {
    let s = farm();
    let c = Constraints {
        daily_minutes: Some(180.0),
        ..Default::default()
    };
    let p = planner::plan(&s, 5, &c).unwrap();
    assert!(p.days[0].actions.iter().all(|a| a.kind != "buy"));
}
#[test]
fn newly_tilled_soil_under_existing_sprinklers_needs_water_today() {
    let mut s = farm();
    s.plots.truncate(1);
    s.plots[0].tilled = false;
    s.plots[0].sprinkler_covered = true;
    s.plots[0].clearable = true;
    s.money = 20;
    s.crops.truncate(1);
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    assert_eq!(p.days[0].watering_tiles, 1);
}
#[test]
fn dead_crops_are_removed_before_planting() {
    let mut s = farm();
    s.plots.truncate(1);
    s.crops.truncate(1);
    s.money = 20;
    s.plots[0].crop = Some(GrowingCrop {
        seed_id: "(O)472".into(),
        dead: true,
        ..Default::default()
    });
    let p = planner::plan(&s, 5, &Constraints::default()).unwrap();
    assert!(p.days[0].actions.iter().any(|a| a.kind == "remove_crop"));
}
#[test]
fn festival_purchase_accounts_for_evening_return_and_bulk_seeds() {
    let mut s = farm();
    s.day = 13;
    s.crops.truncate(1);
    s.shops.clear();
    s.money = 100;
    s.shops.push(ShopDay {
        day: 13,
        shop: "EggFestival".into(),
        opens_at: 900,
        closes_at: 1400,
        return_not_before: 2200,
        travel_minutes: 60.0,
        offers: vec![Offer {
            item_id: "(O)472".into(),
            price: 20,
            stock: -1,
            ..Default::default()
        }],
        ..Default::default()
    });
    s.shops.push(ShopDay {
        day: 18,
        shop: "SeedShop".into(),
        opens_at: 900,
        closes_at: 1700,
        buys_crops: true,
        travel_minutes: 60.0,
        ..Default::default()
    });
    let p = planner::plan(&s, 6, &Constraints::default()).unwrap();
    let bought: u32 = p.days[0]
        .actions
        .iter()
        .filter(|a| a.kind == "buy")
        .map(|a| a.quantity)
        .sum();
    assert_eq!(bought, 5);
    assert!(p.days[0].minutes_used >= 960.0);
    assert!(p.days[0].minutes_used <= 1020.0);
}
#[test]
fn time_limited_search_still_returns_a_complete_daily_plan() {
    let mut s = farm();
    s.money = 5000;
    s.plots = (0..2500)
        .map(|i| Plot {
            tile: Some(Tile {
                x: i % 100,
                y: i / 100,
            }),
            tilled: true,
            reachable: true,
            ..Default::default()
        })
        .collect();
    let c = Constraints {
        time_limit_ms: 50,
        ..Default::default()
    };
    let p = planner::plan(&s, 28, &c).unwrap();
    assert_eq!(p.days.len(), 28);
    assert_eq!(p.deadline_day, 28);
    assert!(p.conservative_final_cash >= s.money);
    assert!(p.calculation_ms < 2000);
}
#[test]
fn sprinkler_material_purchase_and_crafting_beats_expensive_direct_purchase() {
    let mut s = farm();
    s.crops.truncate(1);
    s.items.push(ItemStack {
        item_id: "(O)472".into(),
        quantity: 20,
        source: "inventory".into(),
        ..Default::default()
    });
    s.sprinklers.push(SprinklerOption {
        item_id: "(O)621".into(),
        name: "Quality Sprinkler".into(),
        coverage_offsets: vec![
            Tile { x: 1, y: 0 },
            Tile { x: 0, y: 1 },
            Tile { x: 1, y: 1 },
        ],
        materials: vec![Material {
            item_id: "(O)334".into(),
            quantity: 1,
        }],
        recipe_unlocked: true,
    });
    s.shops[0].offers.extend([
        Offer {
            item_id: "(O)621".into(),
            name: "Quality Sprinkler".into(),
            price: 200,
            stock: 1,
        },
        Offer {
            item_id: "(O)334".into(),
            name: "Copper Bar".into(),
            price: 20,
            stock: 1,
        },
    ]);
    let c = Constraints {
        watering_capacity: Some(1),
        ..Default::default()
    };
    let p = planner::plan(&s, 10, &c).unwrap();
    assert!(
        p.days
            .iter()
            .any(|d| d.actions.iter().any(|a| a.kind == "craft"))
    );
    assert!(p.days.iter().any(|d| {
        d.actions
            .iter()
            .any(|a| a.kind == "buy" && a.item_id == "(O)334")
    }));
    assert!(!p.days.iter().any(|d| {
        d.actions
            .iter()
            .any(|a| a.kind == "buy" && a.item_id == "(O)621")
    }));
}
