//! Simple caching library that supports cache invalidation via tags.
//!
//! #### Example
//! ```no_run
//! use std::time::Duration;
//!
//! use fnct::async_redis::AsyncRedisCache;
//! use redis::{aio::ConnectionManager, Client};
//!
//! struct Application {
//!     cache: AsyncRedisCache<ConnectionManager>,
//! }
//!
//! impl Application {
//!     async fn cached_function(&self, a: i32, b: i32) -> i32 {
//!         self.cache.cached((a, b), &["sum"], async move {
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
//! let conn = ConnectionManager::new(client).await.unwrap();
//! let app = Application {
//!     cache: AsyncRedisCache::new(conn, "my_application".to_owned(), Duration::from_secs(600)),
//! };
//! assert_eq!(app.cached_function(1, 2).await, 3); // running expensive computation and filling cache
//! assert_eq!(app.cached_function(1, 2).await, 3); // loading result from cache
//! app.cache.invalidate_key((1, 2)).await.unwrap(); // invalidate cache by key
//! app.cache.invalidate_tag("sum").await.unwrap(); // invalidate cache by tag
//! # };
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::dbg_macro, clippy::use_debug)]
#![warn(missing_docs, missing_debug_implementations)]

use base64::{engine::general_purpose, Engine};
use serde::Serialize;

pub mod async_redis;

fn make_key(key: impl Serialize) -> Result<String, postcard::Error> {
    Ok(general_purpose::STANDARD.encode(postcard::to_stdvec(&key)?))
}
