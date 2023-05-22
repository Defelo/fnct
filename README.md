[![check](https://github.com/Defelo/fnct/actions/workflows/check.yml/badge.svg)](https://github.com/Defelo/fnct/actions/workflows/check.yml)
[![test](https://github.com/Defelo/fnct/actions/workflows/test.yml/badge.svg)](https://github.com/Defelo/fnct/actions/workflows/test.yml)
[![codecov](https://codecov.io/gh/Defelo/fnct/branch/develop/graph/badge.svg?token=812FG7CG7B)](https://codecov.io/gh/Defelo/fnct)
![Version](https://img.shields.io/github/v/tag/Defelo/fnct?include_prereleases&label=version)
[![dependency status](https://deps.rs/repo/github/Defelo/fnct/status.svg)](https://deps.rs/repo/github/Defelo/fnct)

# fnct
Simple caching library for Rust that supports cache invalidation via tags

## Example
```rust
use std::time::Duration;

use fnct::{backend::AsyncRedisBackend, format::PostcardFormatter, keyfn, AsyncCache};
use redis::{aio::MultiplexedConnection, Client};

struct Application {
    cache: AsyncCache<AsyncRedisBackend<MultiplexedConnection>, PostcardFormatter>,
}

keyfn!(my_cache_key(a: i32, b: i32));
impl Application {
    async fn test(&self, a: i32, b: i32) -> i32 {
        self.cache
            .cached(my_cache_key(a, b), &["sum"], None, || async {
                // expensive computation
                a + b
            })
            .await
            .unwrap()
    }
}

#[tokio::main]
async fn main() {
    let Ok(redis_server) = std::env::var("REDIS_SERVER") else { return; };
    let client = Client::open(redis_server).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();
    let app = Application {
        cache: AsyncCache::new(
            AsyncRedisBackend::new(conn, "my_application".to_owned()),
            PostcardFormatter,
            Duration::from_secs(600),
        ),
    };
    assert_eq!(app.test(1, 2).await, 3); // run expensive computation and fill cache
    assert_eq!(app.test(1, 2).await, 3); // load result from cache
    app.cache.pop_key(my_cache_key(1, 2)).await.unwrap(); // invalidate cache by key
    app.cache.pop_tag("sum").await.unwrap(); // invalidate cache by tag
}
```
