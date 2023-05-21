//! Backends are used to communicate with the cache and implement the most basic low level functions
//! like [`get`](AsyncBackend::get) and [`put`](AsyncBackend::put).

use std::{fmt::Debug, time::Duration};

use async_trait::async_trait;

pub use async_redis::AsyncRedisBackend;

pub mod async_redis;

/// A cache backend that can be used asynchronously.
#[async_trait]
pub trait AsyncBackend<T>: Debug {
    /// The error to return if a cache operation fails.
    type Error: std::error::Error;

    /// Get a cached value.
    async fn get(&self, key: &str) -> Result<Option<T>, Self::Error>;

    /// Set a cached value.
    ///
    /// The value is automatically invalidated after the given `ttl` has elapsed. It can also be
    /// invalidated manually by using one of the [`pop_key`](Self::pop_key),
    /// [`pop_tag`](Self::pop_tag) or [`pop_tags`](Self::pop_tags) functions.
    async fn put(
        &self,
        key: &str,
        value: &T,
        tags: &[&str],
        ttl: Duration,
    ) -> Result<(), Self::Error>;

    /// Remove a cached value using its key.
    async fn pop_key(&self, key: &str) -> Result<(), Self::Error>;

    /// Remove all cached values that are associated with the given tag.
    async fn pop_tag(&self, tag: &str) -> Result<(), Self::Error>;

    /// Remove all cached values that are associated with ALL given tags.
    ///
    /// If `tags` is empty, all cached values (in the given namespace) are removed.
    async fn pop_tags(&self, tags: &[&str]) -> Result<(), Self::Error>;
}
