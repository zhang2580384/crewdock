use crate::{
    app::AppState,
    auth::hash_token,
    error::{ApiError, ApiResult},
};
use redis::{Script, aio::MultiplexedConnection};

const WINDOW_SECONDS: u64 = 15 * 60;
const ACCOUNT_LIMIT: u64 = 10;
const IP_LIMIT: u64 = 60;

// Reserve atomically before database/password work. Rejected attempts never
// extend the cooldown, preventing an attacker from keeping it alive forever.
const RESERVE_SCRIPT: &str = r#"
local ip_count = tonumber(redis.call('GET', KEYS[1]) or '0')
if ip_count >= tonumber(ARGV[1]) then
  return math.max(1, redis.call('TTL', KEYS[1]))
end
local ip_next = redis.call('INCR', KEYS[1])
if ip_next == 1 then redis.call('EXPIRE', KEYS[1], ARGV[3]) end
local account_count = tonumber(redis.call('GET', KEYS[2]) or '0')
if account_count >= tonumber(ARGV[2]) then
  return math.max(1, redis.call('TTL', KEYS[2]))
end
local account_next = redis.call('INCR', KEYS[2])
if account_next == 1 then redis.call('EXPIRE', KEYS[2], ARGV[3]) end
return 0
"#;

pub struct LoginAttempt {
    redis: MultiplexedConnection,
    account_key: String,
}

fn keys(email: &str, ip: Option<&str>, secret: &str) -> (String, String) {
    (
        format!(
            "rate:login:ip:{}",
            hash_token(ip.unwrap_or("unknown"), secret)
        ),
        format!(
            "rate:login:account:{}",
            hash_token(&email.trim().to_ascii_lowercase(), secret)
        ),
    )
}

pub async fn reserve(state: &AppState, email: &str, ip: Option<&str>) -> ApiResult<LoginAttempt> {
    let (ip_key, account_key) = keys(email, ip, &state.config.session_secret);
    let mut redis = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| ApiError::Unavailable("login rate limiter"))?;
    let retry_after: u64 = Script::new(RESERVE_SCRIPT)
        .key(ip_key)
        .key(&account_key)
        .arg(IP_LIMIT)
        .arg(ACCOUNT_LIMIT)
        .arg(WINDOW_SECONDS)
        .invoke_async(&mut redis)
        .await
        .map_err(|_| ApiError::Unavailable("login rate limiter"))?;
    if retry_after > 0 {
        return Err(ApiError::RateLimitedFor(retry_after));
    }
    Ok(LoginAttempt { redis, account_key })
}

impl LoginAttempt {
    pub async fn succeeded(mut self) {
        // Keep the IP budget; clear account failures after a committed login.
        let result: redis::RedisResult<i64> = redis::cmd("DEL")
            .arg(&self.account_key)
            .query_async(&mut self.redis)
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "could not clear successful login attempt counter");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::keys;

    #[test]
    fn account_limits_are_shared_across_case_whitespace_and_client_addresses() {
        let first = keys(" User@Example.com ", Some("203.0.113.1"), "test-secret");
        let second = keys("user@example.com", Some("203.0.113.2"), "test-secret");
        assert_eq!(first.1, second.1);
        assert_ne!(first.0, second.0);
        assert!(!first.1.contains("example.com"));
        assert_eq!(
            first.0,
            keys("other@example.com", Some("203.0.113.1"), "test-secret").0
        );
    }
}
