//! Postgres-backed account store.
//!
//! Persists each account's `Credentials` enum sealed via [`Sealer`].
//! `AccountId` is stored as TEXT (the underlying `Ulid` is not a native
//! Postgres type), `provider` as a short snake_case TEXT tag.
//!
//! Uses the sqlx runtime API (no `query!` macros, so no compile-time DB
//! connection is required).

use chrono::{DateTime, Utc};
use eyre::{eyre, Report, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::core::{AccountId, Credentials, ProviderKind};
use crate::storage::sealer::Sealer;

/// In-memory representation of a row in `submux_accounts` with credentials
/// already opened (or ready to be sealed for insert/update).
#[derive(Debug, Clone)]
pub struct PersistedAccount {
    pub id: AccountId,
    pub provider: ProviderKind,
    pub display_name: String,
    pub credentials: Credentials,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Connection-pooled Postgres account store. Cheap to clone (pool is Arc).
#[derive(Clone)]
pub struct PostgresAccountStore {
    pool: PgPool,
    sealer: Sealer,
}

impl PostgresAccountStore {
    /// Connect to Postgres at `database_url` with a small bounded pool.
    pub async fn connect(database_url: &str, sealer: Sealer) -> Result<Self, Report> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| eyre!("postgres connect failed: {e}"))?;
        Ok(Self { pool, sealer })
    }

    /// Construct from an existing pool (handy for tests / shared pools).
    pub fn from_pool(pool: PgPool, sealer: Sealer) -> Self {
        Self { pool, sealer }
    }

    /// Ensure the `submux_accounts` table exists. Idempotent.
    pub async fn migrate(&self) -> Result<(), Report> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS submux_accounts (
                id              TEXT PRIMARY KEY,
                provider        TEXT NOT NULL,
                display_name    TEXT NOT NULL,
                sealed_creds    BYTEA NOT NULL,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| eyre!("submux_accounts migrate failed: {e}"))?;
        Ok(())
    }

    /// Load every persisted account. Rows whose provider tag or sealed
    /// payload can no longer be decoded are skipped (logged via tracing).
    pub async fn load_all(&self) -> Result<Vec<PersistedAccount>, Report> {
        let rows = sqlx::query(
            r#"SELECT id, provider, display_name, sealed_creds, created_at, updated_at
                 FROM submux_accounts"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| eyre!("submux_accounts load_all query failed: {e}"))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id_str: String = row.try_get("id")?;
            let provider_str: String = row.try_get("provider")?;
            let display_name: String = row.try_get("display_name")?;
            let sealed: Vec<u8> = row.try_get("sealed_creds")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

            let Some(id) = parse_account_id(&id_str) else {
                tracing::warn!(account_id = %id_str, "skipping persisted account: bad id");
                continue;
            };
            let Some(provider) = str_to_provider(&provider_str) else {
                tracing::warn!(provider = %provider_str, "skipping persisted account: unknown provider");
                continue;
            };
            let plaintext = match self.sealer.open(&sealed) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(account_id = %id_str, error = %e, "skipping persisted account: open failed");
                    continue;
                }
            };
            let credentials: Credentials = match serde_json::from_slice(&plaintext) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(account_id = %id_str, error = %e, "skipping persisted account: credentials decode failed");
                    continue;
                }
            };
            out.push(PersistedAccount {
                id,
                provider,
                display_name,
                credentials,
                created_at,
                updated_at,
            });
        }
        Ok(out)
    }

    /// Insert (or overwrite) a single account row.
    pub async fn insert(&self, account: &PersistedAccount) -> Result<(), Report> {
        let provider_tag = provider_to_str(&account.provider);
        let plaintext = serde_json::to_vec(&account.credentials)
            .map_err(|e| eyre!("encode credentials: {e}"))?;
        let sealed = self.sealer.seal(&plaintext);

        sqlx::query(
            r#"
            INSERT INTO submux_accounts (id, provider, display_name, sealed_creds, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (id) DO UPDATE SET
                provider     = EXCLUDED.provider,
                display_name = EXCLUDED.display_name,
                sealed_creds = EXCLUDED.sealed_creds,
                updated_at   = EXCLUDED.updated_at
            "#,
        )
        .bind(account.id.to_string())
        .bind(provider_tag)
        .bind(&account.display_name)
        .bind(&sealed)
        .bind(account.created_at)
        .bind(account.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| eyre!("submux_accounts insert failed: {e}"))?;
        Ok(())
    }

    /// Rewrite just the sealed credentials for an existing account, e.g.
    /// after a successful refresh that rotated the access token.
    pub async fn update_credentials(
        &self,
        id: AccountId,
        credentials: &Credentials,
    ) -> Result<(), Report> {
        let plaintext =
            serde_json::to_vec(credentials).map_err(|e| eyre!("encode credentials: {e}"))?;
        let sealed = self.sealer.seal(&plaintext);
        let result = sqlx::query(
            r#"UPDATE submux_accounts
                  SET sealed_creds = $1, updated_at = now()
                WHERE id = $2"#,
        )
        .bind(&sealed)
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| eyre!("submux_accounts update_credentials failed: {e}"))?;
        if result.rows_affected() == 0 {
            return Err(eyre!("no such account `{}`", id));
        }
        Ok(())
    }

    /// Delete an account by id. No error if it didn't exist.
    pub async fn delete(&self, id: AccountId) -> Result<(), Report> {
        sqlx::query(r#"DELETE FROM submux_accounts WHERE id = $1"#)
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| eyre!("submux_accounts delete failed: {e}"))?;
        Ok(())
    }
}

fn parse_account_id(s: &str) -> Option<AccountId> {
    ulid::Ulid::from_string(s).ok().map(AccountId)
}

/// Stable string tag for a provider kind. Must round-trip through
/// [`str_to_provider`]. Kept independent of serde so the column format is
/// stable even if the enum's serde repr changes.
pub fn provider_to_str(p: &ProviderKind) -> &'static str {
    match p {
        ProviderKind::AnthropicSubscription => "anthropic_subscription",
        ProviderKind::OpenAiSubscription => "openai_subscription",
        ProviderKind::AnthropicApiKey => "anthropic_api_key",
        ProviderKind::OpenAiApiKey => "openai_api_key",
        ProviderKind::Custom => "custom",
    }
}

pub fn str_to_provider(s: &str) -> Option<ProviderKind> {
    Some(match s {
        "anthropic_subscription" => ProviderKind::AnthropicSubscription,
        "openai_subscription" => ProviderKind::OpenAiSubscription,
        "anthropic_api_key" => ProviderKind::AnthropicApiKey,
        "openai_api_key" => ProviderKind::OpenAiApiKey,
        "custom" => ProviderKind::Custom,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn fixture_creds() -> Credentials {
        Credentials::AnthropicOAuth {
            access_token: "at-xyz".into(),
            refresh_token: "rt-xyz".into(),
            expires_at: Utc::now(),
            scopes: vec!["user:inference".into()],
            device_id: Uuid::new_v4(),
            account_uuid: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn provider_tag_roundtrip() {
        for p in [
            ProviderKind::AnthropicSubscription,
            ProviderKind::OpenAiSubscription,
            ProviderKind::AnthropicApiKey,
            ProviderKind::OpenAiApiKey,
            ProviderKind::Custom,
        ] {
            let s = provider_to_str(&p);
            assert_eq!(str_to_provider(s), Some(p));
        }
        assert!(str_to_provider("bogus").is_none());
    }

    #[test]
    fn sealed_creds_roundtrip_simulates_db_column() {
        let sealer = Sealer::from_key_bytes(&[3u8; 32]).unwrap();
        let creds = fixture_creds();
        let plaintext = serde_json::to_vec(&creds).unwrap();
        let sealed = sealer.seal(&plaintext);
        let opened = sealer.open(&sealed).unwrap();
        let back: Credentials = serde_json::from_slice(&opened).unwrap();
        match (creds, back) {
            (
                Credentials::AnthropicOAuth {
                    access_token: a,
                    refresh_token: ra,
                    ..
                },
                Credentials::AnthropicOAuth {
                    access_token: b,
                    refresh_token: rb,
                    ..
                },
            ) => {
                assert_eq!(a, b);
                assert_eq!(ra, rb);
            }
            _ => panic!("credential kind changed across roundtrip"),
        }
    }

    #[test]
    fn account_id_string_roundtrip() {
        let id = AccountId::new();
        let s = id.to_string();
        let parsed = parse_account_id(&s).expect("parse must succeed");
        assert_eq!(parsed.0, id.0);
        assert!(parse_account_id("not-a-ulid").is_none());
    }
}
