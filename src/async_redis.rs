//! Async cache backed by redis.

use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use redis::{AsyncCommands, FromRedisValue, RedisError, RedisResult, ToRedisArgs};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use crate::make_key;

/// Async cache backed by redis.
#[derive(Clone)]
pub struct AsyncRedisCache<C> {
    conn: C,
    namespace: String,
    default_ttl: Duration,
}

impl<C> std::fmt::Debug for AsyncRedisCache<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncRedisCache")
            .field("namespace", &self.namespace)
            .field("default_ttl", &self.default_ttl)
            .finish_non_exhaustive()
    }
}

impl<C> AsyncRedisCache<C>
where
    C: AsyncCommands + Clone,
{
    /// Create a new [`AsyncRedisCache`].
    pub fn new(conn: C, namespace: String, default_ttl: Duration) -> Self {
        Self {
            conn,
            namespace,
            default_ttl,
        }
    }

    /// Wrap a function to add a caching layer using redis.
    ///
    /// This is just a wrapper for the [`cached()`] function in this module.
    pub async fn cached<T, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<T, AsyncRedisCacheError>
    where
        F: Future<Output = T>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        cached(
            &mut self.conn.clone(),
            &self.namespace,
            key,
            tags,
            ttl.unwrap_or(self.default_ttl),
            func,
        )
        .await
    }

    /// Get a specific cache entry by its key.
    pub async fn get<T, K>(&self, key: K) -> Result<Option<T>, AsyncRedisCacheError>
    where
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        match get::<Vec<u8>>(&mut self.conn.clone(), &self.namespace, &make_key(key)?).await? {
            Some(serialized) => Ok(Some(postcard::from_bytes(&serialized)?)),
            None => Ok(None),
        }
    }

    /// Insert a new or update an existing cache entry.
    pub async fn put<T, K>(
        &self,
        key: K,
        value: T,
        tags: &[&str],
        ttl: Option<Duration>,
    ) -> Result<(), AsyncRedisCacheError>
    where
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        let serialized = postcard::to_stdvec(&value)?;
        Ok(put(
            &mut self.conn.clone(),
            &self.namespace,
            &make_key(key)?,
            &serialized,
            tags,
            ttl.unwrap_or(self.default_ttl),
        )
        .await?)
    }

    /// Invalidate a specific cache entry by its key.
    pub async fn pop_key<K>(&self, key: K) -> Result<(), AsyncRedisCacheError>
    where
        K: Serialize,
    {
        Ok(pop_key(&mut self.conn.clone(), &self.namespace, &make_key(key)?).await?)
    }

    /// Invalidate all cache entries that are associated with the given tag.
    pub async fn pop_tag(&self, tag: &str) -> Result<(), AsyncRedisCacheError> {
        Ok(pop_tag(&mut self.conn.clone(), &self.namespace, tag).await?)
    }

    /// Invalidate all cache entries that are associated with ALL of the given tags.
    pub async fn pop_tags(&self, tags: &[&str]) -> Result<(), AsyncRedisCacheError> {
        Ok(pop_tags(&mut self.conn.clone(), &self.namespace, tags).await?)
    }
}

/// Wrap a function to add a caching layer using the functions in this module.
pub async fn cached<T, K, F>(
    redis: &mut impl AsyncCommands,
    namespace: &str,
    key: K,
    tags: &[&str],
    ttl: Duration,
    func: F,
) -> Result<T, AsyncRedisCacheError>
where
    F: Future<Output = T>,
    T: Serialize + DeserializeOwned,
    K: Serialize,
{
    let key = make_key(key)?;
    if let Some(value) = get::<Vec<u8>>(redis, namespace, &key).await? {
        return Ok(postcard::from_bytes(&value)?);
    }
    let value = func.await;
    let serialized = postcard::to_stdvec(&value)?;
    put(redis, namespace, &key, &serialized, tags, ttl).await?;
    Ok(value)
}

/// Get a cached value.
pub async fn get<T: FromRedisValue>(
    conn: &mut impl AsyncCommands,
    namespace: &str,
    key: &str,
) -> RedisResult<Option<T>> {
    conn.get(format!("{namespace}:cache:{key}")).await
}

/// Set a cached value.
///
/// The value is automatically invalidated after the given `ttl` has elapsed. It can also be
/// invalidated manually by using one of the [`pop_key`], [`pop_tag`] or [`pop_tags`] functions.
pub async fn put<T: ToRedisArgs>(
    conn: &mut impl AsyncCommands,
    namespace: &str,
    key: &str,
    value: &T,
    tags: &[&str],
    ttl: Duration,
) -> RedisResult<()> {
    let mut pipe = redis::pipe();
    pipe.set_ex(
        format!("{namespace}:cache:{key}"),
        value,
        ttl.as_secs() as usize,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for tag in tags {
        let k = format!("{namespace}:tag:{tag}");
        pipe.zrembyscore(&k, "-inf", now);
        pipe.zadd(&k, key, now + ttl.as_secs());
        pipe.cmd("EXPIRE").arg(&k).arg(ttl.as_secs()).arg("NX");
        pipe.cmd("EXPIRE").arg(&k).arg(ttl.as_secs()).arg("GT");
    }

    pipe.query_async(conn).await
}

/// Remove a cached value using its key.
pub async fn pop_key(conn: &mut impl AsyncCommands, namespace: &str, key: &str) -> RedisResult<()> {
    conn.del(format!("{namespace}:cache:{key}")).await
}

/// Remove all cached values that are associated with the given tag.
pub async fn pop_tag(conn: &mut impl AsyncCommands, namespace: &str, tag: &str) -> RedisResult<()> {
    let k = format!("{namespace}:tag:{tag}");
    let keys: Vec<String> = conn.zrange(&k, 0, -1).await?;
    let mut pipe = redis::pipe();
    for key in keys {
        pipe.del(format!("{namespace}:cache:{key}"));
    }
    pipe.del(k);
    pipe.query_async(conn).await
}

/// Remove all cached values that are associated with ALL given tags.
///
/// If `tags` is empty, all cached values (in the given namespace) are removed.
pub async fn pop_tags(
    conn: &mut impl AsyncCommands,
    namespace: &str,
    tags: &[&str],
) -> RedisResult<()> {
    let mut pipe = redis::pipe();
    if tags.is_empty() {
        let keys: Vec<String> = conn.keys(format!("{namespace}:cache:*")).await?;
        for key in keys {
            pipe.del(key);
        }
    } else {
        let mut cmd = redis::cmd("ZINTER");
        cmd.arg(tags.len());
        for tag in tags {
            cmd.arg(format!("{namespace}:tag:{tag}"));
        }
        let keys: Vec<String> = cmd.query_async(conn).await?;
        for key in keys {
            pipe.del(format!("{namespace}:cache:{key}"));
        }
    };

    pipe.query_async(conn).await
}

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum AsyncRedisCacheError {
    #[error("redis error: {0}")]
    RedisError(#[from] RedisError),
    #[error("postcard error: {0}")]
    PostcardError(#[from] postcard::Error),
}
