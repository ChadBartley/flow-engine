using System.Runtime.CompilerServices;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

namespace Cleargate;

/// <summary>
/// Decorating <see cref="IChatCompletionService"/> that records every LLM call
/// through a Cleargate <see cref="AdapterSession"/> using the
/// <c>on_llm_start</c> / <c>on_llm_end</c> adapter protocol.
/// Captures full request/response data for ReplayEngine and DiffEngine.
/// </summary>
internal sealed class InstrumentedChatCompletionService : IChatCompletionService
{
    private readonly IChatCompletionService _inner;
    private readonly AdapterSession _session;
    private readonly string _model;
    private readonly string _provider;
    private int _callIndex;

    public InstrumentedChatCompletionService(
        IChatCompletionService inner,
        AdapterSession session,
        string? model = null,
        string? provider = null)
    {
        _inner = inner;
        _session = session;
        _model = model
            ?? (inner.Attributes.TryGetValue("ModelId", out var mid) && mid is string m ? m : "unknown");
        _provider = provider ?? "unknown";
    }

    public IReadOnlyDictionary<string, object?> Attributes => _inner.Attributes;

    public async Task<IReadOnlyList<ChatMessageContent>> GetChatMessageContentsAsync(
        ChatHistory chatHistory,
        PromptExecutionSettings? executionSettings = null,
        Kernel? kernel = null,
        CancellationToken cancellationToken = default)
    {
        var serviceId = $"llm-{Interlocked.Increment(ref _callIndex)}";

        _session.OnEvent(new
        {
            callback = "on_llm_start",
            service_id = serviceId,
            request = BuildRequest(chatHistory, executionSettings),
        });

        var startTick = Environment.TickCount64;

        var results = await _inner.GetChatMessageContentsAsync(
            chatHistory, executionSettings, kernel, cancellationToken);

        var durationMs = Environment.TickCount64 - startTick;

        _session.OnEvent(new
        {
            callback = "on_llm_end",
            service_id = serviceId,
            response = BuildResponse(results),
            duration_ms = durationMs,
        });

        return results;
    }

    public async IAsyncEnumerable<StreamingChatMessageContent> GetStreamingChatMessageContentsAsync(
        ChatHistory chatHistory,
        PromptExecutionSettings? executionSettings = null,
        Kernel? kernel = null,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        var serviceId = $"llm-{Interlocked.Increment(ref _callIndex)}";

        _session.OnEvent(new
        {
            callback = "on_llm_start",
            service_id = serviceId,
            request = BuildRequest(chatHistory, executionSettings),
        });

        var startTick = Environment.TickCount64;
        var chunks = new List<string>();
        string? modelId = null;
        IReadOnlyDictionary<string, object?>? lastMetadata = null;

        await foreach (var chunk in _inner.GetStreamingChatMessageContentsAsync(
            chatHistory, executionSettings, kernel, cancellationToken))
        {
            if (chunk.Content is not null)
                chunks.Add(chunk.Content);
            modelId ??= chunk.ModelId;
            lastMetadata = chunk.Metadata;

            yield return chunk;
        }

        var durationMs = Environment.TickCount64 - startTick;

        _session.OnEvent(new
        {
            callback = "on_llm_end",
            service_id = serviceId,
            response = new
            {
                content = string.Concat(chunks),
                model = modelId,
                metadata = lastMetadata is not null ? SanitizeMetadata(lastMetadata) : null,
                streamed = true,
            },
            duration_ms = durationMs,
        });
    }

    private object BuildRequest(ChatHistory chatHistory, PromptExecutionSettings? settings)
    {
        var messages = chatHistory.Select(m => new
        {
            role = m.Role.Label,
            content = m.Content,
        }).ToArray();

        // Allow per-request model override via execution settings
        var model = settings?.ModelId ?? _model;

        return new
        {
            messages,
            model,
            provider = _provider,
        };
    }

    private static object BuildResponse(IReadOnlyList<ChatMessageContent> results)
    {
        var first = results.FirstOrDefault();
        return new
        {
            content = first?.Content,
            model = first?.ModelId,
            metadata = first?.Metadata is not null ? SanitizeMetadata(first.Metadata) : null,
            choice_count = results.Count,
        };
    }

    private static Dictionary<string, object?> SanitizeMetadata(IReadOnlyDictionary<string, object?> metadata)
    {
        var result = new Dictionary<string, object?>();
        foreach (var kvp in metadata)
        {
            if (kvp.Value is null or string or int or long or double or float or bool)
                result[kvp.Key] = kvp.Value;
            else
                result[kvp.Key] = kvp.Value.ToString();
        }
        return result;
    }
}
