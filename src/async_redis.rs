//! Async cache backed by redis.

use std::{
    fmt::Debug,
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use redis::{AsyncCommands, FromRedisValue, RedisError, RedisResult, ToRedisArgs};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use crate::{format::Formatter, make_key};

/// Async cache backed by redis.
#[derive(Clone)]
pub struct AsyncRedisCache<C, S> {
    conn: C,
    namespace: String,
    default_ttl: Duration,
    formatter: S,
}

impl<C, S> std::fmt::Debug for AsyncRedisCache<C, S>
where
    S: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncRedisCache")
            .field("namespace", &self.namespace)
            .field("default_ttl", &self.default_ttl)
            .field("formatter", &self.formatter)
            .finish_non_exhaustive()
    }
}

impl<C, S> AsyncRedisCache<C, S>
where
    C: AsyncCommands + Clone,
    S: Formatter,
    S::Serialized: FromRedisValue + ToRedisArgs,
{
    /// Create a new [`AsyncRedisCache`].
    pub fn new(conn: C, namespace: String, default_ttl: Duration, formatter: S) -> Self {
        Self {
            conn,
            namespace,
            default_ttl,
            formatter,
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
    ) -> Result<T, AsyncRedisCacheError<S::Error>>
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
            &self.formatter,
        )
        .await
    }
    /// Wrap a function to add a caching layer using redis.
    /// Cache [`Option::Some`] variants only.
    ///
    /// This is just a wrapper for the [`cached_option()`] function in this module.
    pub async fn cached_option<T, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<Option<T>, AsyncRedisCacheError<S::Error>>
    where
        F: Future<Output = Option<T>>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        cached_option(
            &mut self.conn.clone(),
            &self.namespace,
            key,
            tags,
            ttl.unwrap_or(self.default_ttl),
            func,
            &self.formatter,
        )
        .await
    }

    /// Wrap a function to add a caching layer using redis.
    /// Cache [`Result::Ok`] variants only.
    ///
    /// This is just a wrapper for the [`cached_result()`] function in this module.
    pub async fn cached_result<T, E, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<Result<T, E>, AsyncRedisCacheError<S::Error>>
    where
        F: Future<Output = Result<T, E>>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        cached_result(
            &mut self.conn.clone(),
            &self.namespace,
            key,
            tags,
            ttl.unwrap_or(self.default_ttl),
            func,
            &self.formatter,
        )
        .await
    }

    /// Get a specific cache entry by its key.
    pub async fn get<T, K>(&self, key: K) -> Result<Option<T>, AsyncRedisCacheError<S::Error>>
    where
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        get(&mut self.conn.clone(), &self.namespace, &make_key(key)?)
            .await?
            .map(|serialized| self.deserialize(&serialized))
            .transpose()
    }

    /// Insert a new or update an existing cache entry.
    pub async fn put<T, K>(
        &self,
        key: K,
        value: T,
        tags: &[&str],
        ttl: Option<Duration>,
    ) -> Result<(), AsyncRedisCacheError<S::Error>>
    where
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        Ok(put(
            &mut self.conn.clone(),
            &self.namespace,
            &make_key(key)?,
            &self.serialize(&value)?,
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

    fn serialize<T: Serialize>(
        &self,
        value: &T,
    ) -> Result<S::Serialized, AsyncRedisCacheError<S::Error>> {
        self.formatter
            .serialize(value)
            .map_err(AsyncRedisCacheError::CacheFormatError)
    }

    fn deserialize<T: DeserializeOwned>(
        &self,
        value: &S::Serialized,
    ) -> Result<T, AsyncRedisCacheError<S::Error>> {
        self.formatter
            .deserialize(value)
            .map_err(AsyncRedisCacheError::CacheFormatError)
    }
}

/// Wrap a function to add a caching layer using the functions in this module.
pub async fn cached<T, K, F, S>(
    redis: &mut impl AsyncCommands,
    namespace: &str,
    key: K,
    tags: &[&str],
    ttl: Duration,
    func: F,
    formatter: &S,
) -> Result<T, AsyncRedisCacheError<S::Error>>
where
    F: Future<Output = T>,
    T: Serialize + DeserializeOwned,
    K: Serialize,
    S: Formatter,
    S::Serialized: FromRedisValue + ToRedisArgs,
{
    let key = make_key(key)?;
    if let Some(value) = get(redis, namespace, &key).await? {
        return formatter
            .deserialize(&value)
            .map_err(AsyncRedisCacheError::CacheFormatError);
    }
    let value = func.await;
    let serialized = formatter
        .serialize(&value)
        .map_err(AsyncRedisCacheError::CacheFormatError)?;
    put(redis, namespace, &key, &serialized, tags, ttl).await?;
    Ok(value)
}

/// Wrap a function to add a caching layer using the functions in this module.
/// Cache [`Option::Some`] variants only.
pub async fn cached_option<T, K, F, S>(
    redis: &mut impl AsyncCommands,
    namespace: &str,
    key: K,
    tags: &[&str],
    ttl: Duration,
    func: F,
    formatter: &S,
) -> Result<Option<T>, AsyncRedisCacheError<S::Error>>
where
    F: Future<Output = Option<T>>,
    T: Serialize + DeserializeOwned,
    K: Serialize,
    S: Formatter,
    S::Serialized: FromRedisValue + ToRedisArgs,
{
    Ok(cached_result(
        redis,
        namespace,
        key,
        tags,
        ttl,
        async { func.await.ok_or(()) },
        formatter,
    )
    .await?
    .ok())
}

/// Wrap a function to add a caching layer using the functions in this module.
/// Cache [`Result::Ok`] variants only.
pub async fn cached_result<T, E, K, F, S>(
    redis: &mut impl AsyncCommands,
    namespace: &str,
    key: K,
    tags: &[&str],
    ttl: Duration,
    func: F,
    formatter: &S,
) -> Result<Result<T, E>, AsyncRedisCacheError<S::Error>>
where
    F: Future<Output = Result<T, E>>,
    T: Serialize + DeserializeOwned,
    K: Serialize,
    S: Formatter,
    S::Serialized: FromRedisValue + ToRedisArgs,
{
    let key = make_key(key)?;
    if let Some(value) = get(redis, namespace, &key).await? {
        return Ok(Ok(formatter
            .deserialize(&value)
            .map_err(AsyncRedisCacheError::CacheFormatError)?));
    }
    match func.await {
        Ok(value) => {
            let serialized = formatter
                .serialize(&value)
                .map_err(AsyncRedisCacheError::CacheFormatError)?;
            put(redis, namespace, &key, &serialized, tags, ttl).await?;
            Ok(Ok(value))
        }
        Err(err) => Ok(Err(err)),
    }
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
pub enum AsyncRedisCacheError<E = ()> {
    #[error("redis error: {0}")]
    RedisError(#[from] RedisError),
    #[error("postcard error: {0}")]
    PostcardError(#[from] postcard::Error),
    #[error("cache format error: {0}")]
    CacheFormatError(E),
}
