//! Simple caching library that supports cache invalidation via tags.
//!
//! #### Example
//! ```no_run
//! use std::time::Duration;
//!
//! use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, key, AsyncCache};
//! use redis::{aio::MultiplexedConnection, Client};
//!
//! struct Application {
//!     cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter>,
//! }
//!
//! impl Application {
//!     async fn cached_function(&self, a: i32, b: i32) -> i32 {
//!         self.cache
//!             .cached(key!(a, b), &["sum"], None, async move {
//!                 // expensive computation
//!                 a + b
//!             })
//!             .await
//!             .unwrap()
//!     }
//! }
//!
//! # async {
//! let client = Client::open("redis://localhost:6379/0").unwrap();
//! let conn = client.get_multiplexed_async_connection().await.unwrap();
//! let app = Application {
//!     cache: AsyncCache::new(
//!         AsyncRedisBackend::new(conn, "my_application".to_owned()),
//!         PostcardFormatter,
//!         Duration::from_secs(600),
//!     ),
//! };
//! assert_eq!(app.cached_function(1, 2).await, 3); // run expensive computation and fill cache
//! assert_eq!(app.cached_function(1, 2).await, 3); // load result from cache
//! app.cache.pop_key((1, 2)).await.unwrap(); // invalidate cache by key
//! app.cache.pop_tag("sum").await.unwrap(); // invalidate cache by tag
//! # };
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::dbg_macro, clippy::use_debug)]
#![warn(missing_docs, missing_debug_implementations)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::{future::Future, time::Duration};

use backend::AsyncBackend;
use format::Formatter;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha512};

pub use postcard;
pub use serde;
#[cfg(feature = "serde_json")]
pub use serde_json;

pub mod backend;
pub mod format;

/// A cache that can be used asynchronously.
#[derive(Debug, Clone)]
pub struct AsyncCache<B, S> {
    backend: B,
    formatter: S,
    default_ttl: Duration,
}

impl<B, S> AsyncCache<B, S>
where
    B: AsyncBackend<S::Serialized>,
    S: Formatter,
{
    /// Create a new [`AsyncCache`].
    pub fn new(backend: B, formatter: S, default_ttl: Duration) -> Self {
        Self {
            backend,
            formatter,
            default_ttl,
        }
    }

    /// Create a new cache that uses a different formatter.
    pub fn with_formatter<T: Formatter>(&self, formatter: T) -> AsyncCache<B, T>
    where
        B: Clone,
    {
        AsyncCache {
            backend: self.backend.clone(),
            formatter,
            default_ttl: self.default_ttl,
        }
    }

    /// Wrap a function to add a caching layer.
    pub async fn cached<T, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<T, Error<B, S>>
    where
        F: Future<Output = T>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        if let Some(value) = self.get(&key).await? {
            return Ok(value);
        }
        let value = func.await;
        self.put(&key, &value, tags, ttl).await?;
        Ok(value)
    }

    /// Wrap a function to add a caching layer.
    /// Cache [`Option::Some`] variants only.
    pub async fn cached_option<T, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<Option<T>, Error<B, S>>
    where
        F: Future<Output = Option<T>>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        self.cached_result(key, tags, ttl, async { func.await.ok_or(()) })
            .await
            .map(Result::ok)
    }

    /// Wrap a function to add a caching layer.
    /// Cache [`Result::Ok`] variants only.
    pub async fn cached_result<T, E, K, F>(
        &self,
        key: K,
        tags: &[&str],
        ttl: Option<Duration>,
        func: F,
    ) -> Result<Result<T, E>, Error<B, S>>
    where
        F: Future<Output = Result<T, E>>,
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        if let Some(value) = self.get(&key).await? {
            return Ok(Ok(value));
        }
        let value = match func.await {
            Ok(x) => x,
            Err(err) => return Ok(Err(err)),
        };
        self.put(&key, &value, tags, ttl).await?;
        Ok(Ok(value))
    }

    /// Get a specific cache entry by its key.
    pub async fn get<T, K>(&self, key: K) -> Result<Option<T>, Error<B, S>>
    where
        T: Serialize + DeserializeOwned,
        K: Serialize,
    {
        self.backend
            .get(&make_key::<S>(key)?)
            .await
            .map_err(Error::BackendError)?
            .map(|x| self.formatter.deserialize(&x))
            .transpose()
            .map_err(Error::FormatterError)
    }

    /// Insert a new or update an existing cache entry.
    pub async fn put<T, K>(
        &self,
        key: K,
        value: T,
        tags: &[&str],
        ttl: Option<Duration>,
    ) -> Result<(), Error<B, S>>
    where
        T: Serialize,
        K: Serialize,
    {
        let serialized = self
            .formatter
            .serialize(&value)
            .map_err(Error::FormatterError)?;

        self.backend
            .put(
                &make_key::<S>(key)?,
                &serialized,
                tags,
                ttl.unwrap_or(self.default_ttl),
            )
            .await
            .map_err(Error::BackendError)
    }

    /// Invalidate a specific cache entry by its key.
    pub async fn pop_key<K>(&self, key: K) -> Result<(), Error<B, S>>
    where
        K: Serialize,
    {
        self.backend
            .pop_key(&make_key::<S>(key)?)
            .await
            .map_err(Error::BackendError)
    }

    /// Invalidate all cache entries that are associated with the given tag.
    pub async fn pop_tag(&self, tag: &str) -> Result<(), Error<B, S>> {
        self.backend.pop_tag(tag).await.map_err(Error::BackendError)
    }

    /// Invalidate all cache entries that are associated with ALL of the given tags.
    pub async fn pop_tags(&self, tags: &[&str]) -> Result<(), Error<B, S>> {
        self.backend
            .pop_tags(tags)
            .await
            .map_err(Error::BackendError)
    }
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum Error<B, S>
where
    B: AsyncBackend<S::Serialized>,
    S: Formatter,
{
    #[error("postcard error: {0}")]
    PostcardError(#[from] postcard::Error),
    #[error("backend error: {0}")]
    BackendError(B::Error),
    #[error("formatter error: {0}")]
    FormatterError(S::Error),
}

fn make_key<F: Formatter>(key: impl Serialize) -> Result<String, postcard::Error> {
    Ok(format!(
        "{:x}:{}",
        Sha512::new()
            .chain_update(postcard::to_stdvec(&key)?)
            .finalize(),
        F::ID
    ))
}

/// Create a cache key that includes the source location where the macro was invoked.
#[macro_export]
macro_rules! key {
    ($($x:expr),*$(,)?) => {
        (
            ::std::module_path!(),
            ::std::file!(),
            ::std::line!(),
            ::std::column!(),
            $($x),*
        )
    };
}
