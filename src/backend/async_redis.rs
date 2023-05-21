//! Async cache backed by redis.

use std::{
    fmt::Debug,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use redis::{AsyncCommands, FromRedisValue, RedisError, ToRedisArgs};
use thiserror::Error;

use crate::backend::AsyncBackend;

/// Async cache backend for [Redis](https://redis.io/).
#[derive(Clone)]
pub struct AsyncRedisBackend<C> {
    conn: C,
    namespace: String,
}

impl<C> AsyncRedisBackend<C> {
    /// Create a new [`AsyncRedisBackend`].
    pub fn new(conn: C, namespace: String) -> Self {
        Self { conn, namespace }
    }
}

impl<C> std::fmt::Debug for AsyncRedisBackend<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncRedisBackend")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<C, T> AsyncBackend<T> for AsyncRedisBackend<C>
where
    C: AsyncCommands + Clone + Sync,
    T: FromRedisValue + ToRedisArgs + Sync,
{
    type Error = redis::RedisError;

    async fn get(&self, key: &str) -> Result<Option<T>, Self::Error> {
        self.conn
            .clone()
            .get(format!("{}:cache:{}", self.namespace, key))
            .await
    }

    async fn put(
        &self,
        key: &str,
        value: &T,
        tags: &[&str],
        ttl: Duration,
    ) -> Result<(), Self::Error> {
        let mut pipe = redis::pipe();
        pipe.set_ex(
            format!("{}:cache:{}", self.namespace, key),
            value,
            ttl.as_secs() as usize,
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for tag in tags {
            let k = format!("{}:tag:{}", self.namespace, tag);
            pipe.zrembyscore(&k, "-inf", now);
            pipe.zadd(&k, key, now + ttl.as_secs());
            pipe.cmd("EXPIRE").arg(&k).arg(ttl.as_secs()).arg("NX");
            pipe.cmd("EXPIRE").arg(&k).arg(ttl.as_secs()).arg("GT");
        }

        pipe.query_async(&mut self.conn.clone()).await
    }

    async fn pop_key(&self, key: &str) -> Result<(), Self::Error> {
        self.conn
            .clone()
            .del(format!("{}:cache:{}", self.namespace, key))
            .await
    }

    async fn pop_tag(&self, tag: &str) -> Result<(), Self::Error> {
        let mut conn = self.conn.clone();
        let k = format!("{}:tag:{}", self.namespace, tag);
        let keys: Vec<String> = conn.zrange(&k, 0, -1).await?;
        let mut pipe = redis::pipe();
        for key in keys {
            pipe.del(format!("{}:cache:{}", self.namespace, key));
        }
        pipe.del(k);
        pipe.query_async(&mut conn).await
    }

    async fn pop_tags(&self, tags: &[&str]) -> Result<(), Self::Error> {
        let mut conn = self.conn.clone();
        let mut pipe = redis::pipe();
        if tags.is_empty() {
            let keys: Vec<String> = conn.keys(format!("{}:cache:*", self.namespace)).await?;
            for key in keys {
                pipe.del(key);
            }
        } else {
            let mut cmd = redis::cmd("ZINTER");
            cmd.arg(tags.len());
            for tag in tags {
                cmd.arg(format!("{}:tag:{}", self.namespace, tag));
            }
            let keys: Vec<String> = cmd.query_async(&mut conn).await?;
            for key in keys {
                pipe.del(format!("{}:cache:{}", self.namespace, key));
            }
        };

        pipe.query_async(&mut conn).await
    }
}

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum Error {
    #[error("redis error: {0}")]
    RedisError(#[from] RedisError),
    #[error("postcard error: {0}")]
    PostcardError(#[from] postcard::Error),
}
