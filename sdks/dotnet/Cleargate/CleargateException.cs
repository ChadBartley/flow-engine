namespace Cleargate;

/// <summary>
/// Exception thrown by Cleargate native operations.
/// </summary>
public class CleargateException : Exception
{
    public CleargateException(string message) : base(message) { }
    public CleargateException(string message, Exception inner) : base(message, inner) { }
}
