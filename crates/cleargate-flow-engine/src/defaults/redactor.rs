//! Default redactor implementation for the write pipeline.

use serde_json::Value;

use crate::traits::Redactor;
use crate::types::Sensitivity;

/// Default data redactor applied inside the write pipeline.
///
/// Redaction rules by sensitivity level:
/// - `None` — pass through unchanged
/// - `Secret` — replace with `"[REDACTED]"`
/// - `Masked` — keep last 4 chars: `"****" + last4`; non-strings get `"[MASKED]"`
/// - `Pii` — replace with `"[PII_REDACTED]"`
/// - `Custom(_)` — replace with `"[CUSTOM_REDACTED]"`
pub struct DefaultRedactor;

impl Redactor for DefaultRedactor {
    fn redact(&self, _field: &str, sensitivity: &Sensitivity, value: &Value) -> Value {
        match sensitivity {
            Sensitivity::None => value.clone(),
            Sensitivity::Secret => Value::String("[REDACTED]".into()),
            Sensitivity::Masked => match value {
                Value::String(s) if s.len() >= 4 => {
                    Value::String(format!("****{}", &s[s.len() - 4..]))
                }
                _ => Value::String("[MASKED]".into()),
            },
            Sensitivity::Pii => Value::String("[PII_REDACTED]".into()),
            // Custom and any future variants.
            _ => Value::String("[CUSTOM_REDACTED]".into()),
        }
    }

    fn redact_llm_messages(&self, messages: &Value, sensitivity: &Sensitivity) -> Value {
        if *sensitivity == Sensitivity::None {
            return messages.clone();
        }
        match messages {
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .map(|msg| redact_message(self, msg, sensitivity))
                    .collect(),
            ),
            other => self.redact("llm_messages", sensitivity, other),
        }
    }
}

/// Redact the `content` field of a single message object while preserving
/// structural fields (role, tool_call_id, etc.) needed for replay.
fn redact_message(redactor: &DefaultRedactor, message: &Value, sensitivity: &Sensitivity) -> Value {
    match message {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == "content" {
                    out.insert(k.clone(), redactor.redact("content", sensitivity, v));
                } else {
                    out.insert(k.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_none_passthrough() {
        let r = DefaultRedactor;
        let val = json!("sensitive data");
        let result = r.redact("field", &Sensitivity::None, &val);
        assert_eq!(result, val);
    }

    #[test]
    fn test_secret_redacted() {
        let r = DefaultRedactor;
        let val = json!("my_api_key_12345");
        let result = r.redact("field", &Sensitivity::Secret, &val);
        assert_eq!(result, json!("[REDACTED]"));
    }

    #[test]
    fn test_masked_keeps_last4() {
        let r = DefaultRedactor;
        let val = json!("mysecretkey");
        let result = r.redact("field", &Sensitivity::Masked, &val);
        assert_eq!(result, json!("****tkey"));
    }

    #[test]
    fn test_masked_short_string() {
        let r = DefaultRedactor;
        let val = json!("ab");
        let result = r.redact("field", &Sensitivity::Masked, &val);
        assert_eq!(result, json!("[MASKED]"));
    }

    #[test]
    fn test_masked_non_string() {
        let r = DefaultRedactor;
        let val = json!(42);
        let result = r.redact("field", &Sensitivity::Masked, &val);
        assert_eq!(result, json!("[MASKED]"));
    }

    #[test]
    fn test_pii_redacted() {
        let r = DefaultRedactor;
        let val = json!("john.doe@example.com");
        let result = r.redact("field", &Sensitivity::Pii, &val);
        assert_eq!(result, json!("[PII_REDACTED]"));
    }

    #[test]
    fn test_custom_redacted() {
        let r = DefaultRedactor;
        let val = json!("hipaa data");
        let result = r.redact("field", &Sensitivity::Custom("hipaa".into()), &val);
        assert_eq!(result, json!("[CUSTOM_REDACTED]"));
    }

    #[test]
    fn test_redact_llm_messages_preserves_structure() {
        let r = DefaultRedactor;
        let messages = json!([
            {"role": "system", "content": "You are a helpful assistant"},
            {"role": "user", "content": "Tell me about user@example.com"},
            {"role": "assistant", "content": "Here is the info", "tool_call_id": "call_1"}
        ]);
        let result = r.redact_llm_messages(&messages, &Sensitivity::Pii);

        let arr = result.as_array().expect("should be array");
        assert_eq!(arr.len(), 3);

        // role preserved, content redacted
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "[PII_REDACTED]");

        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[1]["content"], "[PII_REDACTED]");

        // tool_call_id preserved
        assert_eq!(arr[2]["role"], "assistant");
        assert_eq!(arr[2]["content"], "[PII_REDACTED]");
        assert_eq!(arr[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn test_redact_llm_messages_none_passthrough() {
        let r = DefaultRedactor;
        let messages = json!([{"role": "user", "content": "hello"}]);
        let result = r.redact_llm_messages(&messages, &Sensitivity::None);
        assert_eq!(result, messages);
    }
}
