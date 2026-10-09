namespace SvOptimizer.Core;

public static class SnapshotMath
{
    public static int DaysToHarvest(IEnumerable<int> phases, int phase, int elapsed, bool regrowing)
    {
        if (regrowing) return Math.Max(0, elapsed);
        var days = phases.ToArray();
        if (phase >= days.Length - 1) return 0;
        return Math.Max(0, days[phase] - elapsed) + days.Skip(phase + 1).Take(days.Length - phase - 2).Sum();
    }
    public static double ExpectedYield(int minimum, int maximum, double extraChance)
    {
        var p = Math.Clamp(extraChance, 0, 0.9);
        return (minimum + Math.Max(minimum, maximum)) / 2.0 + p / (1 - p);
    }
    public static bool DateCondition(string query, int day, Func<string, bool> currentStateQuery)
    {
        // Conditions that depend on tomorrow's time, random choices, or weather
        // are excluded rather than evaluated against today's state.
        var clauses = query.Split(',', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
        foreach (var clause in clauses)
        {
            var tokens = clause.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            if (tokens.Length == 0) continue;
            var name = tokens[0]; bool negate = name.StartsWith('!'); name = name.TrimStart('!');
            if (name == "DAY_OF_WEEK")
            {
                var weekday = new[] { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" }[(day - 1) % 7];
                bool matches = tokens.Skip(1).Any(t => t.StartsWith(weekday, StringComparison.OrdinalIgnoreCase));
                if (matches == negate) return false;
            }
            else if (name == "DAY_OF_MONTH")
            {
                bool matches = tokens.Skip(1).Any(t => (int.TryParse(t, out int n) && n == day)
                    || (t.Equals("even", StringComparison.OrdinalIgnoreCase) && day % 2 == 0)
                    || (t.Equals("odd", StringComparison.OrdinalIgnoreCase) && day % 2 == 1));
                if (matches == negate) return false;
            }
            else if (name.Contains("RANDOM", StringComparison.Ordinal) || name.Contains("TIME", StringComparison.Ordinal)
                || name.Contains("WEATHER", StringComparison.Ordinal) || name.Contains("DAYS_PLAYED", StringComparison.Ordinal)
                || name.Contains("SEASON_DAY", StringComparison.Ordinal) || clause.Contains('"')) return false;
            else if (!currentStateQuery(clause)) return false;
        }
        return true;
    }
}
