//! Default secrets provider that reads from environment variables.

use async_trait::async_trait;

use crate::errors::SecretsError;
use crate::traits::SecretsProvider;

/// Reads secrets from environment variables.
///
/// `get(key)` maps directly to `std::env::var(key)`. Returns `Ok(None)` if
/// the variable is not set. `list_keys()` returns an empty list since
/// environment variables cannot be enumerated portably.
pub struct EnvSecretsProvider;

#[async_trait]
impl SecretsProvider for EnvSecretsProvider {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        match std::env::var(key) {
            Ok(val) => Ok(Some(val)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(SecretsError::Provider {
                message: format!("failed to read env var {key}: {e}"),
            }),
        }
    }

    async fn list_keys(&self) -> Result<Vec<String>, SecretsError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_env_var_found() {
        // Use a unique name to avoid cross-test interference.
        unsafe { std::env::set_var("CLEARGATE_TEST_SECRET_XYZ_001", "my_secret_value") };
        let provider = EnvSecretsProvider;
        let result = provider.get("CLEARGATE_TEST_SECRET_XYZ_001").await.unwrap();
        assert_eq!(result, Some("my_secret_value".to_string()));
        unsafe { std::env::remove_var("CLEARGATE_TEST_SECRET_XYZ_001") };
    }

    #[tokio::test]
    async fn test_env_var_not_found() {
        let provider = EnvSecretsProvider;
        let result = provider
            .get("CLEARGATE_DEFINITELY_NONEXISTENT_VAR")
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_list_keys_empty() {
        let provider = EnvSecretsProvider;
        let keys = provider.list_keys().await.unwrap();
        assert!(keys.is_empty());
    }
}
