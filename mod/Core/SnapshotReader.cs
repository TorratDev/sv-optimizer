using System.Collections;
using SvOptimizer.Protocol;
using static SvOptimizer.Core.GameAccess;

namespace SvOptimizer.Core;

/// <summary>Stardew 1.6 observer. Capture must run on SMAPI's game thread.</summary>
public sealed class SnapshotReader
{
    private readonly Type game = Type("StardewValley.Game1");
    private readonly Type registry = Type("StardewValley.ItemRegistry");
    private readonly Type loader = Type("StardewValley.DataLoader");
    private readonly Type queries = Type("StardewValley.GameStateQuery");
    private readonly int threadId = Environment.CurrentManagedThreadId;
    private readonly int maxCandidatePlots;
    public SnapshotReader(int maxCandidatePlots = 2500) => this.maxCandidatePlots = Math.Clamp(maxCandidatePlots, 1, 10000);

    public FarmSnapshot Capture(bool ready, bool singlePlayer, string gameVersion, string smapiVersion)
    {
        if (Environment.CurrentManagedThreadId != threadId) throw new InvalidOperationException("Game observations must run on the game thread");
        var s = new FarmSnapshot { SchemaVersion = 1, CapturedAtUnixMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(),
            WorldReady = ready, SinglePlayer = singlePlayer, GameVersion = gameVersion, SmapiVersion = smapiVersion };
        if (!ready) return s;
        if (!gameVersion.StartsWith("1.6", StringComparison.Ordinal)) throw new NotSupportedException("Observer supports Stardew Valley 1.6.x; update the game and SMAPI");
        var player = Member(game, "player")!;
        var farm = Call(game, "getFarm")!;
        s.SaveId = Get<ulong>(game, "uniqueIDForThisGame").ToString();
        s.PlayerId = Get<long>(player, "UniqueMultiplayerID").ToString();
        s.Year = Get<uint>(game, "year"); s.Day = Get<uint>(game, "dayOfMonth");
        s.Season = (Member(game, "season", false)?.ToString() ?? Get<string>(game, "currentSeason")).ToLowerInvariant();
        s.TimeOfDay = Get<uint>(game, "timeOfDay"); s.Money = Get<long>(player, "Money");
        s.FarmingLevel = Get<uint>(player, "FarmingLevel"); s.MaxEnergy = Get<double>(player, "MaxStamina");
        s.Energy = Math.Clamp(Get<double>(player, "Stamina"), 0, s.MaxEnergy);
        foreach (var profession in Values(Member(player, "professions"))) s.Professions.Add(Convert.ToUInt32(profession));
        foreach (var name in new[] { "Farming", "Fishing", "Foraging", "Mining", "Combat", "Luck" })
            s.Skills.Add(new SkillState { Name = name.ToLowerInvariant(), Level = Get<uint>(player, name + "Level", 0, false) });
        foreach (var item in Values(Member(player, "Items")))
        {
            ReadItem(item, "inventory", null, 0, s);
            if (item.GetType().IsSubclassOf(Type("StardewValley.Tool")))
                s.Tools.Add(new ToolState { Name = Get<string>(item, "Name", "", false).Contains("Scythe", StringComparison.OrdinalIgnoreCase) ? "Scythe" : item.GetType().Name, UpgradeLevel = Get<uint>(item, "UpgradeLevel", 0, false) });
        }
        bool canWater = s.Tools.Any(t => t.Name == "WateringCan");
        // Single-tile estimates remain feasible without assuming a rectangular
        // charged pattern. Upgrades are exposed for manual capacity overrides.
        s.WateringEnergyPerTile = Math.Max(0.1, 2.0 - 0.1 * s.FarmingLevel);
        s.WateringMinutesPerTile = 1.0;
        s.EstimatedWateringCapacity = canWater ? (uint)Math.Floor(s.Energy / s.WateringEnergyPerTile) : 0;
        s.Assumptions.Add("Watering uses conservative single-tile can estimates; charged watering and enchantments may permit a higher manual override. Water-can refills are covered by the travel buffer.");
        s.Assumptions.Add("Future farming level and equipment stay fixed; future morning energy assumes sleeping on time with no exhaustion penalties.");
        ReadCropCatalog(s);
        var distances = ReadPlots(farm, player, s);
        ReadChests(farm, distances, s);
        ReadSprinklers(player, s);
        ReadShops(player, s);
        ReadWeather(s);
        s.Assumptions.Add("Candidate land is main-farm outdoor soil reachable from the player or a farm entrance. Only bare soil and ordinary weeds, twigs, and small stones are modeled for clearing; buildings, paths, trees, large stumps and boulders are preserved.");
        s.Assumptions.Add("Current shop unlocks are held fixed for the remainder of the season. Date conditions and festival closures are applied without changing the game clock. Dynamic/random item queries, unsupported price modifiers and barter are omitted.");
        return s;
    }

    private void ReadItem(object item, string source, Tile? tile, double retrieval, FarmSnapshot s)
    {
        var id = ItemId(item);
        if (Get<uint>(item, "Stack", 0) == 0) return;
        // Farm-grown outputs are recognized even when their category is unusual.
        int category = Get<int>(item, "Category", 0, false);
        bool crop = category is -75 or -79 or -80;
        long salePrice = 0;
        if (crop) salePrice = Convert.ToInt64(Call(item, "sellToStorePrice", -1L));
        s.Items.Add(new ItemStack { ItemId = id, Name = Get<string>(item, "DisplayName", id),
            Quantity = Get<uint>(item, "Stack"), Quality = Get<uint>(item, "Quality", 0, false),
            SalePrice = Math.Max(0, salePrice), IsCrop = crop, Source = source,
            SourceTile = tile, RetrievalMinutes = retrieval });
    }

    private void ReadCropCatalog(FarmSnapshot s)
    {
        var objects = Pairs(Member(game, "objectData")).ToDictionary(p => p.Key.ToString()!, p => p.Value);
        foreach (var (key, data) in Pairs(Member(game, "cropData")))
        {
            string seed = key.ToString()!;
            string harvest = Get<string>(data, "HarvestItemId");
            var plainHarvest = harvest.StartsWith("(O)") ? harvest[3..] : harvest;
            if (!objects.TryGetValue(plainHarvest, out var output)) { s.Warnings.Add($"Crop {seed} has no object price; excluded"); continue; }
            int min = Get<int>(data, "HarvestMinStack", 1, false);
            int max = Get<int>(data, "HarvestMaxStack", min, false);
            max += (int)((float)s.FarmingLevel * Get<float>(data, "HarvestMaxIncreasePerFarmingLevel", 0, false));
            bool paddy = Get<bool>(data, "IsPaddyCrop", false, false);
            bool forage = Get<bool>(data, "IsWildSeedCrop", false, false) || seed is "495" or "496" or "497" or "498";
            bool normalQuality = Get<int>(data, "HarvestMinQuality", 0, false) == 0 && Get<int>(data, "HarvestMaxQuality", 4, false) >= 4;
            bool sellable = Get<int>(output, "Category", 0, false) is -75 or -79 or -80;
            var crop = new CropDefinition { SeedId = Qualify(seed), HarvestId = Qualify(harvest),
                Name = Get<string>(output, "Name", plainHarvest), RegrowDays = (uint)Math.Max(0, Get<int>(data, "RegrowDays", -1, false)),
                MinimumYield = (uint)Math.Max(1, min), ExpectedYield = SnapshotMath.ExpectedYield(min, max, Get<double>(data, "ExtraHarvestChance", 0, false)),
                BaseSalePrice = Get<int>(output, "Price"),
                Trellis = Get<bool>(data, "IsRaised", false, false), NeedsWatering = Get<bool>(data, "NeedsWatering", true, false),
                ForageCrop = forage, Supported = !paddy && !forage && normalQuality && sellable,
                UnsupportedReason = paddy ? "Paddy water proximity is not modeled" : forage ? "Forage crop mechanics are not modeled" : !normalQuality ? "Custom quality limits are not modeled" : !sellable ? "Harvest is not a standard shop-sellable crop" : "" };
            crop.GrowthPhases.Add(Values(Member(data, "DaysInPhase")).Select(Convert.ToUInt32));
            crop.Seasons.Add(Values(Member(data, "Seasons")).Select(x => x.ToString()!.ToLowerInvariant()));
            foreach (int quality in new[] { 0, 1, 2, 4 })
            {
                // Transient items are never added to inventory. Ask the game's
                // own price calculation to include profession and profit margin.
                var transient = Call(registry, "Create", crop.HarvestId, 1, quality)!;
                crop.SalePricesByQuality.Add(Convert.ToInt64(Call(transient, "sellToStorePrice", -1L)));
            }
            s.Crops.Add(crop);
        }
    }

    private Dictionary<(int X, int Y), int> ReadPlots(object farm, object player, FarmSnapshot s)
    {
        var map = Member(farm, "Map")!;
        var layers = Values(Member(map, "Layers")).ToArray();
        var back = layers.First(x => Get<string>(x, "Id") == "Back");
        int width = Get<int>(back, "LayerWidth"), height = Get<int>(back, "LayerHeight");
        var objects = Pairs(Member(farm, "Objects")).ToDictionary(p => Coordinates(p.Key), p => p.Value);
        var features = Pairs(Member(farm, "terrainFeatures")).ToDictionary(p => Coordinates(p.Key), p => p.Value);
        var blocked = new HashSet<(int, int)>();
        foreach (var key in objects.Keys) blocked.Add(key);
        foreach (var (key, feature) in features)
            if (!feature.GetType().Name.Equals("HoeDirt", StringComparison.Ordinal) && !feature.GetType().Name.Equals("Grass", StringComparison.Ordinal)) blocked.Add(key);
        foreach (var building in Values(Member(farm, "buildings", false)))
        {
            int x = Get<int>(building, "tileX"), y = Get<int>(building, "tileY");
            int w = Get<int>(building, "tilesWide"), h = Get<int>(building, "tilesHigh");
            for (int xx = x; xx < x + w; xx++) for (int yy = y; yy < y + h; yy++) blocked.Add((xx, yy));
        }
        var passable = new HashSet<(int, int)>();
        var diggable = new HashSet<(int, int)>();
        for (int x = 0; x < width; x++) for (int y = 0; y < height; y++)
        {
            if (Call(farm, "doesTileHaveProperty", x, y, "Diggable", "Back") != null) diggable.Add((x, y));
            // Tiles with Buildings-layer collision remain blocked. Null means
            // no tile; Passable is the game's explicit override.
            var buildingLayer = layers.FirstOrDefault(l => Get<string>(l, "Id") == "Buildings");
            bool collision = false;
            if (buildingLayer != null)
            {
                var tiles = Member(buildingLayer, "Tiles")!;
                var indexer = tiles.GetType().GetProperty("Item", new[] { typeof(int), typeof(int) });
                collision = indexer?.GetValue(tiles, new object[] { x, y }) != null
                    && Call(farm, "doesTileHaveProperty", x, y, "Passable", "Buildings") == null;
            }
            bool trellis = features.TryGetValue((x, y), out var feature) && feature.GetType().Name == "HoeDirt"
                && Member(feature, "crop", false) is object growing && Get<bool>(growing, "raisedSeeds", false, false) && !Get<bool>(growing, "dead", false, false);
            var backTiles = Member(back, "Tiles")!;
            var backIndexer = backTiles.GetType().GetProperty("Item", new[] { typeof(int), typeof(int) });
            bool hasGround = backIndexer?.GetValue(backTiles, new object[] { x, y }) != null;
            bool water = Call(farm, "doesTileHaveProperty", x, y, "Water", "Back") != null;
            if (hasGround && !water && !blocked.Contains((x, y)) && !collision && !trellis) passable.Add((x, y));
        }
        var starts = new List<(int, int)>();
        var location = Member(player, "currentLocation")!;
        if (Get<string>(location, "NameOrUniqueName") == Get<string>(farm, "NameOrUniqueName"))
            starts.Add((Convert.ToInt32(Call(player, "getTileX")), Convert.ToInt32(Call(player, "getTileY"))));
        else foreach (var warp in Values(Member(farm, "warps")))
            if (Get<string>(warp, "TargetName").Contains("FarmHouse", StringComparison.Ordinal)) starts.Add((Get<int>(warp, "X"), Get<int>(warp, "Y")));
        if (starts.Count == 0) throw new InvalidDataException("Cannot identify an accessible farm entrance; visit the farm and refresh");
        var distance = new Dictionary<(int, int), int>(); var queue = new Queue<(int, int)>();
        foreach (var start in starts) { distance[start] = 0; queue.Enqueue(start); }
        while (queue.TryDequeue(out var p)) foreach (var next in Neighbors(p))
            if (passable.Contains(next) && !distance.ContainsKey(next)) { distance[next] = distance[p] + 1; queue.Enqueue(next); }
        bool reachable((int X, int Y) p) => distance.ContainsKey(p) || Neighbors(p).Any(distance.ContainsKey);
        var covered = new HashSet<(int, int)>();
        foreach (var (_, item) in objects)
        {
            if (ItemId(item) is not ("(O)599" or "(O)621" or "(O)645")) continue;
            foreach (var tile in Values(Call(item, "GetSprinklerTiles"))) covered.Add(Coordinates(tile));
        }
        var candidates = diggable.Union(features.Where(p => p.Value.GetType().Name == "HoeDirt").Select(p => p.Key))
            .OrderBy(p => features.ContainsKey(p) ? 0 : 1).ThenBy(p => distance.GetValueOrDefault(p, int.MaxValue));
        foreach (var p in candidates)
        {
            if (!reachable(p)) continue;
            features.TryGetValue(p, out var feature); objects.TryGetValue(p, out var obstacle);
            if (feature != null && feature.GetType().Name != "HoeDirt") continue;
            bool tilled = feature?.GetType().Name == "HoeDirt";
            string obstruction = obstacle == null ? "" : Get<string>(obstacle, "Name", "unknown");
            bool debris = obstacle != null && (obstruction.Contains("Weeds", StringComparison.OrdinalIgnoreCase)
                || obstruction.Contains("Twig", StringComparison.OrdinalIgnoreCase) || obstruction == "Stone");
            if (obstacle != null && !debris) continue;
            bool canClear = obstacle == null || (obstruction.Contains("Weeds", StringComparison.OrdinalIgnoreCase) && s.Tools.Any(t => t.Name.Contains("Scythe")))
                || (obstruction.Contains("Twig", StringComparison.OrdinalIgnoreCase) && s.Tools.Any(t => t.Name == "Axe"))
                || (obstruction == "Stone" && s.Tools.Any(t => t.Name == "Pickaxe"));
            var plot = new Plot { Tile = new Tile { X = p.Item1, Y = p.Item2 }, Tilled = tilled,
                Clearable = canClear && s.Tools.Any(t => t.Name == "Hoe"), Reachable = true,
                SprinklerCovered = covered.Contains(p), Obstruction = obstruction,
                ClearingEnergy = obstacle == null ? 0 : obstruction.Contains("Weeds") ? 0 : 6,
                ClearingMinutes = obstacle == null ? 0 : 3 };
            if (tilled && feature != null)
            {
                plot.WateredToday = Get<int>(feature, "state") == 1;
                plot.QualityFertilizer = Convert.ToUInt32(Call(feature, "GetFertilizerQualityBoostLevel"));
                plot.SpeedBonus = Convert.ToDouble(Call(feature, "GetFertilizerSpeedBoost"));
                if (Member(feature, "crop", false) is object crop)
                {
                    plot.Crop = new GrowingCrop { SeedId = Qualify(Get<string>(crop, "netSeedIndex", "", false)), Dead = Get<bool>(crop, "dead"),
                        DaysToHarvest = (uint)SnapshotMath.DaysToHarvest(Values(Member(crop, "phaseDays")).Select(Convert.ToInt32),
                            Get<int>(crop, "currentPhase"), Get<int>(crop, "dayOfCurrentPhase"), Get<bool>(crop, "fullyGrown")) };
                }
            }
            s.Plots.Add(plot);
            if (s.Plots.Count >= maxCandidatePlots) { s.Warnings.Add($"Candidate plots capped at {maxCandidatePlots}; existing plots are prioritized. Change MaxCandidatePlots in the mod config to expand observation."); break; }
        }
        return distance;
    }
    private static (int X, int Y) Coordinates(object tile) => (Get<int>(tile, "X"), Get<int>(tile, "Y"));
    private static IEnumerable<(int X, int Y)> Neighbors((int X, int Y) p)
    { yield return (p.X + 1, p.Y); yield return (p.X - 1, p.Y); yield return (p.X, p.Y + 1); yield return (p.X, p.Y - 1); }

    private void ReadChests(object farm, Dictionary<(int X, int Y), int> distances, FarmSnapshot s)
    {
        foreach (var (key, chest) in Pairs(Member(farm, "Objects")))
        {
            if (chest.GetType().Name != "Chest" || !Get<bool>(chest, "playerChest", false, false)) continue;
            var special = Member(chest, "SpecialChestType", false)?.ToString();
            if (special is "MiniShippingBin" or "JunimoChest" or "AutoLoader") continue;
            var tile = Coordinates(key); var nearby = Neighbors(tile).Where(distances.ContainsKey).Select(t => distances[t]).ToArray();
            if (nearby.Length == 0) continue;
            // .Items is read directly: GetItemsForPlayer can create a per-player
            // inventory in some special chests, violating strict observation.
            var items = Member(chest, "Items", false) ?? Member(chest, "items", false);
            if (items == null) { s.Warnings.Add($"Could not read chest at {tile}; excluded"); continue; }
            foreach (var item in Values(items)) ReadItem(item, $"farm_chest:{tile.X},{tile.Y}", new Tile { X = tile.X, Y = tile.Y }, 5 + nearby.Min() * 0.5, s);
        }
    }

    private void ReadSprinklers(object player, FarmSnapshot s)
    {
        var unlocked = Pairs(Member(player, "craftingRecipes")).Select(p => p.Key.ToString()).ToHashSet();
        var recipes = Pairs(Call(loader, "CraftingRecipes", Member(game, "content"))).ToDictionary(p => p.Key.ToString()!, p => p.Value.ToString()!);
        foreach (var (name, id, radius) in new[] { ("Sprinkler", "(O)599", 0), ("Quality Sprinkler", "(O)621", 1), ("Iridium Sprinkler", "(O)645", 2) })
        {
            var option = new SprinklerOption { ItemId = id, Name = name, RecipeUnlocked = unlocked.Contains(name) };
            if (radius == 0) option.CoverageOffsets.Add(new[] { new Tile { X = -1 }, new Tile { X = 1 }, new Tile { Y = -1 }, new Tile { Y = 1 } });
            else for (int x = -radius; x <= radius; x++) for (int y = -radius; y <= radius; y++)
                if (x != 0 || y != 0) option.CoverageOffsets.Add(new Tile { X = x, Y = y });
            if (recipes.TryGetValue(name, out var recipe))
            {
                var parts = recipe.Split('/')[0].Split(' ', StringSplitOptions.RemoveEmptyEntries);
                for (int i = 0; i + 1 < parts.Length; i += 2) option.Materials.Add(new Material { ItemId = Qualify(parts[i]), Quantity = uint.Parse(parts[i + 1]) });
            }
            else option.RecipeUnlocked = false;
            s.Sprinklers.Add(option);
        }
    }

    private void ReadShops(object player, FarmSnapshot s)
    {
        var shops = Pairs(Call(loader, "Shops", Member(game, "content"))).ToDictionary(p => p.Key.ToString()!, p => p.Value);
        bool mail(string id) => Convert.ToBoolean(Call(player, "hasOrWillReceiveMail", id));
        bool cc = mail("ccIsComplete");
        var reachable = new List<(string Id, uint Open, uint Close, bool Buys, double Travel)>
            { ("SeedShop", 900, 1700, true, 60), ("Blacksmith", 900, 1600, false, 80) };
        if (!cc && !mail("JojaMartClosed")) reachable.Add(("Joja", 900, 2300, false, 80));
        if (mail("ccVault") || mail("jojaVault")) reachable.Add(("Sandy", 1000, 2300, false, 120));
        if (Get<bool>(player, "hasRustyKey", false, false)) reachable.Add(("Krobus", 800, 2400, false, 100));
        string? eggShop = shops.Keys.FirstOrDefault(id => id.Contains("Egg", StringComparison.OrdinalIgnoreCase)
            && Values(Member(shops[id], "Items")).Any(item => Get<string>(item, "ItemId", "", false) == "(O)745"));
        if (eggShop != null && s.Season == "spring" && s.Day <= 13) reachable.Add((eggShop, 900, 1400, false, 60));
        foreach (var schedule in reachable)
        {
            if (!shops.TryGetValue(schedule.Id, out var data)) continue;
            // Never call ShopBuilder.GetShopStock: it updates synchronized stock.
            // Read static offers and evaluate nonrandom conditions instead.
            for (uint day = s.Day; day <= 28; day++)
            {
                bool festival = Convert.ToBoolean(Call(Type("StardewValley.Utility"), "isFestivalDay", (int)day, Member(game, "season")));
                bool egg = schedule.Id == eggShop;
                if (egg && day != 13) continue;
                if (festival && schedule.Id is not "Krobus" && !egg) continue;
                if (schedule.Id == "SeedShop" && day % 7 == 3 && !cc) continue;
                if (schedule.Id == "Blacksmith" && day % 7 == 5 && cc) continue;
                var shop = new ShopDay { Day = day, Shop = schedule.Id, OpensAt = schedule.Open, ClosesAt = schedule.Close, BuysCrops = schedule.Buys, TravelMinutes = schedule.Travel, ReturnNotBefore = egg ? 2200u : 0u };
                foreach (var item in Values(Member(data, "Items")))
                {
                    string? id = Get<string?>(item, "ItemId", null, false);
                    if (string.IsNullOrEmpty(id) || id.Any(char.IsWhiteSpace) || !id.StartsWith("(O)", StringComparison.Ordinal)) continue;
                    string condition = Get<string>(item, "Condition", "", false);
                    if (!SnapshotMath.DateCondition(condition, (int)day, c => Convert.ToBoolean(Call(queries, "CheckConditions", c)))) continue;
                    if (Get<bool>(item, "IsRecipe", false, false) || Member(item, "TradeItemId", false) is string) continue;
                    var transient = Call(registry, "Create", id)!;
                    float price = Get<float>(item, "Price", -1, false);
                    if (price < 0) price = Get<bool>(item, "UseObjectDataPrice", false, false)
                        ? Get<float>(transient, "Price") : Convert.ToSingle(Call(transient, "salePrice", true));
                    bool margins = Member(item, "ApplyProfitMargins", false) as bool? ?? Member(data, "ApplyProfitMargins", false) as bool?
                        ?? Convert.ToBoolean(Call(transient, "appliesProfitMargins"));
                    if (margins) price *= Get<float>(Member(game, "MasterPlayer")!, "difficultyModifier", 1, false);
                    if (!Get<bool>(item, "IgnoreShopPriceModifiers", false, false)) price = Modify(price, data, "PriceModifiers", "PriceModifierMode", (int)day);
                    price = Modify(price, item, "PriceModifiers", "PriceModifierMode", (int)day);
                    if (price <= 0 || !float.IsFinite(price)) continue;
                    int stock = Get<int>(item, "AvailableStock", -1, false);
                    if (stock == int.MaxValue) stock = -1;
                    if (stock >= 0)
                    {
                        float modified = Modify(stock, item, "AvailableStockModifiers", "AvailableStockModifierMode", (int)day);
                        if (!float.IsFinite(modified)) continue;
                        stock = modified >= int.MaxValue ? -1 : (int)Math.Max(-1, modified);
                    }
                    // Finite shop stock can already have been purchased. Without
                    // reading the synchronized counter, do not advertise it today.
                    if (stock >= 0 && day == s.Day) continue;
                    shop.Offers.Add(new Offer { ItemId = id, Name = Get<string>(transient, "DisplayName", id), Price = (long)price, Stock = stock });
                }
                s.Shops.Add(shop);
            }
        }
        s.Warnings.Add("Finite stock is omitted for today unless independently observed; future finite offers assume no purchases outside the plan. Refresh after purchases or unlocks.");
    }

    private float Modify(float initial, object owner, string collectionName, string modeName, int day)
    {
        string mode = Member(owner, modeName, false)?.ToString() ?? "Stack";
        float? result = null;
        foreach (var modifier in Values(Member(owner, collectionName, false)))
        {
            if (HasItems(Member(modifier, "RandomAmount", false))) return float.NaN;
            string condition = Get<string>(modifier, "Condition", "", false);
            if (!SnapshotMath.DateCondition(condition, day, c => Convert.ToBoolean(Call(queries, "CheckConditions", c)))) continue;
            float basis = mode == "Stack" ? result ?? initial : initial;
            float applied = Convert.ToSingle(Call(modifier.GetType(), "Apply", basis, Member(modifier, "Modification"), Get<float>(modifier, "Amount")));
            result = mode switch { "Minimum" => Math.Min(result ?? applied, applied), "Maximum" => Math.Max(result ?? applied, applied), _ => applied };
        }
        return result ?? initial;
    }

    private void ReadWeather(FarmSnapshot s)
    {
        s.KnownWeather.Add(new WeatherDay { Day = s.Day, Rain = Get<bool>(game, "isRaining", false, false) || Get<bool>(game, "isLightning", false, false) });
        // WeatherForTomorrow is an actual forecast. No later RNG is inspected.
        if (s.Day < 28)
        {
            string tomorrow = Get<string>(game, "weatherForTomorrow", "", false);
            if (tomorrow is "Rain" or "Storm" or "1" or "3") s.KnownWeather.Add(new WeatherDay { Day = s.Day + 1, Rain = true });
        }
    }
}
