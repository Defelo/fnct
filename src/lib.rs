//! Simple caching library that supports cache invalidation via tags.
//!
//! #### Example
//! ```no_run
//! use std::time::Duration;
//!
//! use fnct::async_redis::AsyncRedisCache;
//! use redis::{aio::MultiplexedConnection, Client};
//!
//! struct Application {
//!     cache: AsyncRedisCache<MultiplexedConnection>,
//! }
//!
//! impl Application {
//!     async fn cached_function(&self, a: i32, b: i32) -> i32 {
//!         self.cache.cached((a, b), &["sum"], None, async move {
//!             // expensive computation
//!             a + b
//!         })
//!         .await
//!         .unwrap()
//!     }
//! }
//!
//! # async {
//! let client = Client::open("redis://localhost:6379/0").unwrap();
//! let conn = client.get_multiplexed_async_connection().await.unwrap();
//! let app = Application {
//!     cache: AsyncRedisCache::new(conn, "my_application".to_owned(), Duration::from_secs(600)),
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

use serde::Serialize;
use sha2::{Digest, Sha512};

pub mod async_redis;

fn make_key(key: impl Serialize) -> Result<String, postcard::Error> {
    Ok(format!(
        "{:x}",
        Sha512::new()
            .chain_update(postcard::to_stdvec(&key)?)
            .finalize()
    ))
}
