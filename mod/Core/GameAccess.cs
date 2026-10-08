using System.Collections;
using System.Reflection;

namespace SvOptimizer.Core;

/// <summary>
/// Isolates 1.6 game API reads from transport and snapshot types. No setters,
/// game actions, stock builders, or global RNG calls are used. A missing
/// required member fails the observation rather than silently inventing state.
/// </summary>
public static class GameAccess
{
    private const BindingFlags Flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static;
    public static Type Type(string name) => AppDomain.CurrentDomain.GetAssemblies().Select(a => a.GetType(name, false)).FirstOrDefault(t => t != null)
        ?? throw new NotSupportedException($"Game type unavailable: {name}; Stardew 1.6 is required");
    public static object? Member(object target, string name, bool required = true)
    {
        var type = target as Type ?? target.GetType(); var instance = target is Type ? null : target;
        for (var t = type; t != null; t = t.BaseType)
        {
            var property = t.GetProperty(name, Flags | BindingFlags.DeclaredOnly);
            if (property != null && property.GetIndexParameters().Length == 0) return property.GetValue(instance);
            var field = t.GetField(name, Flags | BindingFlags.DeclaredOnly);
            if (field != null) return field.GetValue(instance);
        }
        if (required) throw new MissingMemberException(type.FullName, name);
        return null;
    }
    public static object? Unwrap(object? value)
    {
        if (value == null) return null;
        if (value.GetType().Namespace?.StartsWith("Netcode", StringComparison.Ordinal) == true)
        {
            var v = Member(value, "Value", false);
            if (v != null) return v;
        }
        return value;
    }
    public static T Get<T>(object target, string name, T fallback = default!, bool required = true)
    {
        var value = Unwrap(Member(target, name, required));
        if (value == null) return fallback;
        if (value is T typed) return typed;
        return (T)Convert.ChangeType(value, typeof(T), System.Globalization.CultureInfo.InvariantCulture);
    }
    public static IEnumerable<object> Values(object? collection) => collection is IEnumerable values ? values.Cast<object>().Where(v => v != null) : Enumerable.Empty<object>();
    public static IEnumerable<(object Key, object Value)> Pairs(object? dictionary)
    {
        if (dictionary == null) yield break;
        var pairs = Member(dictionary, "Pairs", false) ?? dictionary;
        foreach (var pair in Values(pairs))
        {
            if (pair is DictionaryEntry e) { if (e.Value != null) yield return (e.Key, e.Value); }
            else { var key = Member(pair, "Key"); var value = Member(pair, "Value"); if (key != null && value != null) yield return (key, value); }
        }
    }
    public static object? Call(object target, string name, params object?[] args)
    {
        var type = target as Type ?? target.GetType(); var instance = target is Type ? null : target;
        foreach (var method in type.GetMethods(Flags).Where(m => m.Name == name && !m.ContainsGenericParameters))
        {
            var parameters = method.GetParameters();
            if (args.Length > parameters.Length || parameters.Skip(args.Length).Any(p => !p.IsOptional)) continue;
            if (args.Where((a, i) => a != null && !parameters[i].ParameterType.IsInstanceOfType(a)).Any()) continue;
            var supplied = parameters.Select((p, i) => i < args.Length ? args[i] : p.DefaultValue).ToArray();
            try { return method.Invoke(instance, supplied); }
            catch (TargetInvocationException ex) { throw ex.InnerException ?? ex; }
        }
        throw new MissingMethodException(type.FullName, name);
    }
    public static string ItemId(object item) => Get<string>(item, "QualifiedItemId");
    public static string Qualify(string id) => id.StartsWith('(') ? id : $"(O){id}";
    public static bool HasItems(object? collection) => Values(collection).Any();
}
