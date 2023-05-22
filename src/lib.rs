//! Simple caching library that supports cache invalidation via tags.
//!
//! #### Example
//! ```
//! use std::time::Duration;
//!
//! use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, keyfn, AsyncCache};
//! use redis::{aio::MultiplexedConnection, Client};
//!
//! struct Application {
//!     cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter>,
//! }
//!
//! keyfn!(my_cache_key(a: i32, b: i32));
//! impl Application {
//!     async fn test(&self, a: i32, b: i32) -> i32 {
//!         self.cache
//!             .cached(my_cache_key(a, b), &["sum"], None, async {
//!                 // expensive computation
//!                 a + b
//!             })
//!             .await
//!             .unwrap()
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//! # if let Ok(REDIS_SERVER) = std::env::var("REDIS_SERVER") {
//!     let client = Client::open(REDIS_SERVER).unwrap();
//!     let conn = client.get_multiplexed_async_connection().await.unwrap();
//!     let app = Application {
//!         cache: AsyncCache::new(
//!             AsyncRedisBackend::new(conn, "my_application".to_owned()),
//!             PostcardFormatter,
//!             Duration::from_secs(600),
//!         ),
//!     };
//!     assert_eq!(app.test(1, 2).await, 3); // run expensive computation and fill cache
//!     assert_eq!(app.test(1, 2).await, 3); // load result from cache
//!     app.cache.pop_key(my_cache_key(1, 2)).await.unwrap(); // invalidate cache by key
//!     app.cache.pop_tag("sum").await.unwrap(); // invalidate cache by tag
//! # }
//! }
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
///
/// #### Example
/// ```
/// # use fnct::{AsyncCache, backend::AsyncRedisBackend, format::PostcardFormatter};
/// # use redis::{Client, aio::MultiplexedConnection};
/// # use std::time::Duration;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # if let Ok(REDIS_SERVER) = std::env::var("REDIS_SERVER") {
/// # let client = Client::open(REDIS_SERVER)?;
/// # let conn = client.get_multiplexed_async_connection().await?;
/// # let backend = AsyncRedisBackend::new(conn, "test".to_owned());
/// # let formatter = PostcardFormatter;
/// let cache = AsyncCache::new(backend, formatter, Duration::from_secs(10));
///
/// assert!(cache.get::<i32, _>("foo").await?.is_none()); // cache is empty initially
/// cache
///     .put("foo", 42, &[], Some(Duration::from_secs(1)))
///     .await?; // store a value in the cache
/// assert_eq!(cache.get::<i32, _>("foo").await?.unwrap(), 42); // retrieve the value
/// tokio::time::sleep(Duration::from_secs(1)).await; // wait 1 second
/// assert!(cache.get::<i32, _>("foo").await?.is_none()); // the value expired
///
/// cache.put("foo", 1337, &["my_tag"], None).await?; // store value with a tag
/// assert_eq!(cache.get::<i32, _>("foo").await?.unwrap(), 1337); // retrieve the value
/// cache.pop_tag("my_tag").await?; // delete value by tag
/// assert!(cache.get::<i32, _>("foo").await?.is_none());
///
/// cache.put("foo", 42, &["1", "2", "3"], None).await?; // store values with different tags
/// cache.put("bar", 1337, &["1", "2", "4"], None).await?;
/// assert_eq!(cache.get::<i32, _>("foo").await?.unwrap(), 42);
/// assert_eq!(cache.get::<i32, _>("bar").await?.unwrap(), 1337);
/// cache.pop_tags(&["2", "3"]).await?; // delete values tagged with both "2" and "3"
/// assert!(cache.get::<i32, _>("foo").await?.is_none()); // deleted (was tagged with "2" and "3")
/// assert_eq!(cache.get::<i32, _>("bar").await?.unwrap(), 1337); // kept (not tagged with "3")
/// # }
/// # Ok(())
/// # }
/// ```
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
    ///
    /// #### Example
    /// ```no_run
    /// # use fnct::{AsyncCache, backend::AsyncRedisBackend, format::PostcardFormatter};
    /// # use std::time::Duration;
    /// # let conn: redis::aio::MultiplexedConnection = todo!();
    /// let cache = AsyncCache::new(
    ///     AsyncRedisBackend::new(conn, "my_namespace".into()),
    ///     PostcardFormatter,
    ///     Duration::from_secs(600),
    /// );
    /// ```
    pub fn new(backend: B, formatter: S, default_ttl: Duration) -> Self {
        Self {
            backend,
            formatter,
            default_ttl,
        }
    }

    /// Create a new cache that uses a different formatter.
    ///
    /// #### Example
    /// ```no_run
    /// # use fnct::{backend::AsyncRedisBackend, AsyncCache, format::{PostcardFormatter, JsonFormatter}};
    /// # use redis::aio::MultiplexedConnection;
    /// type MyCache<F> = AsyncCache<AsyncRedisBackend<MultiplexedConnection>, F>;
    /// let postcard_cache: MyCache<PostcardFormatter> = todo!();
    /// let json_cache: MyCache<JsonFormatter> = postcard_cache.with_formatter(JsonFormatter);
    /// ```
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
    ///
    /// #### Example
    /// ```no_run
    /// # use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, AsyncCache};
    /// # use redis::aio::MultiplexedConnection;
    /// let cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter> = todo!();
    /// # async {
    /// let result = cache
    ///     .cached("my_cache_key", &["tag1", "tag2"], None, async {
    ///         (1..=1000000).sum::<u64>()
    ///     })
    ///     .await
    ///     .unwrap();
    /// assert_eq!(result, 500000500000);
    /// # };
    /// ```
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
    ///
    /// #### Example
    /// ```no_run
    /// # use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, AsyncCache};
    /// # use redis::aio::MultiplexedConnection;
    /// let cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter> = todo!();
    /// # async {
    /// let result = cache
    ///     .cached_option("my_cache_key", &["tag1", "tag2"], None, async {
    ///         if todo!() {
    ///             Some((1..=1000000).sum::<u64>()) // cached
    ///         } else {
    ///             None // not cached
    ///         }
    ///     })
    ///     .await
    ///     .unwrap();
    /// assert_eq!(result, Some(500000500000));
    /// # };
    /// ```
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
    ///
    /// #### Example
    /// ```no_run
    /// # use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, AsyncCache};
    /// # use redis::aio::MultiplexedConnection;
    /// let cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter> = todo!();
    /// # async {
    /// let result = cache
    ///     .cached_result("my_cache_key", &["tag1", "tag2"], None, async {
    ///         if todo!() {
    ///             Ok((1..=1000000).sum::<u64>()) // cached
    ///         } else {
    ///             Err("test") // not cached
    ///         }
    ///     })
    ///     .await
    ///     .unwrap();
    /// assert_eq!(result, Ok(500000500000));
    /// # };
    /// ```
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
    ///
    /// The value is automatically invalidated after the given `ttl` has elapsed. It can also be
    /// invalidated manually by using one of the [`pop_key`](Self::pop_key),
    /// [`pop_tag`](Self::pop_tag) or [`pop_tags`](Self::pop_tags) functions.
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
    ///
    /// If `tags` is empty, all cached values (in the given namespace) are invalidated.
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

/// Create a cache key that includes the source location where the macro was invoked and the crate
/// name and version.
///
/// #### Example
/// ```
/// # use fnct::key;
/// let k1 = key!();
/// let k2 = key!();
/// assert_ne!(k1, k2); // each invocation yields a different key
/// ```
#[macro_export]
macro_rules! key {
    ($($x:expr),*$(,)?) => {
        (
            ::std::env!("CARGO_PKG_NAME"),
            ::std::env!("CARGO_PKG_VERSION"),
            ::std::module_path!(),
            ::std::file!(),
            ::std::line!(),
            ::std::column!(),
            $($x),*
        )
    };
}

/// Create a function that generates a cache key using the [`key!`] macro. Useful if you need shared
/// access to the same cache key.
///
/// #### Example
/// ```
/// # use fnct::keyfn;
/// keyfn!(test1(a: i32, b: i32));
/// keyfn!(test2(a: i32, b: i32));
/// let k1 = test1(1, 2);
/// let k2 = test1(1, 2);
/// let k3 = test1(1, 3);
/// let k4 = test2(1, 2);
/// assert_eq!(k1, k2); // same function + same arguments      -> same key
/// assert_ne!(k1, k3); // same function + different arguments -> different key
/// assert_ne!(k1, k4); // different function                  -> different key
/// ```
///
/// `keyfn!(pub my_cache_key(a: i32, b: i32));` expands roughly to
/// ```ignore
/// pub fn my_cache_key(a: i32, b: i32) -> ... {
///     key!(a, b)
/// }
/// ```
#[macro_export]
macro_rules! keyfn {
    ($vis:vis $name:ident($($arg:ident: $ty:ty),*$(,)?)) => {
        $vis fn $name($($arg: $ty),*) -> (
            &'static str, &'static str, &'static str, &'static str, u32, u32, $($ty),*
        ) {
            $crate::key!($($arg),*)
        }
    };
}
