use std::time::{Duration, Instant};

use fnct::async_redis::AsyncRedisCache;
use redis::{aio::MultiplexedConnection, Client};
use tokio::sync::OnceCell;

#[tokio::test]
async fn test_cached() {
    let cache = get_cache("test_cached").await;

    struct App {
        cache: AsyncRedisCache<MultiplexedConnection>,
    }

    impl App {
        async fn expensive_computation(&self, a: i32, b: i32) -> i32 {
            self.cache
                .cached((a, b), &[], None, async {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    a + b
                })
                .await
                .unwrap()
        }
    }

    let app = App { cache };

    let now = Instant::now();
    assert_eq!(app.expensive_computation(1, 2).await, 3);
    assert!(now.elapsed() >= Duration::from_secs(3));

    let now = Instant::now();
    assert_eq!(app.expensive_computation(1, 2).await, 3);
    assert_eq!(app.expensive_computation(1, 2).await, 3);
    assert_eq!(app.expensive_computation(1, 2).await, 3);
    assert!(now.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn test_basic_insert_expire() {
    let cache = get_cache("test_basic_insert_expire").await;
    assert_eq!(cache.get::<String, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<String, _>("asdf").await.unwrap(), None);
    cache
        .put("foo", "bar".to_owned(), &[], Some(Duration::from_secs(1)))
        .await
        .unwrap();
    cache
        .put("asdf", "baz".to_owned(), &[], Some(Duration::from_secs(2)))
        .await
        .unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some("bar".to_owned()));
    assert_eq!(cache.get("asdf").await.unwrap(), Some("baz".to_owned()));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(cache.get::<String, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get("asdf").await.unwrap(), Some("baz".to_owned()));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(cache.get::<String, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<String, _>("asdf").await.unwrap(), None);
}

#[tokio::test]
async fn test_delete_by_key() {
    let cache = get_cache("test_delete_by_key").await;
    cache.put("foo", "bar".to_owned(), &[], None).await.unwrap();
    cache
        .put("asdf", "baz".to_owned(), &[], None)
        .await
        .unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some("bar".to_owned()));
    assert_eq!(cache.get("asdf").await.unwrap(), Some("baz".to_owned()));
    cache.pop_key("asdf").await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some("bar".to_owned()));
    assert_eq!(cache.get::<String, _>("asdf").await.unwrap(), None);
    cache.pop_key("foo").await.unwrap();
    assert_eq!(cache.get::<String, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<String, _>("asdf").await.unwrap(), None);
}

#[tokio::test]
async fn test_delete_by_tag() {
    let cache = get_cache("test_delete_by_tag").await;
    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tag("t3").await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);
    cache.pop_tag("t2").await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("bar").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);

    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tag("t1").await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("bar").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);
}

#[tokio::test]
async fn test_delete_by_tags_intersect() {
    let cache = get_cache("test_delete_by_tags_intersect").await;
    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tags(&["t2", "t3"]).await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tags(&["t1", "t2"]).await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("bar").await.unwrap(), None);
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));

    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tags(&["t1", "t3"]).await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);

    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    cache.put("xy", 4, &["t4"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tags(&["t1"]).await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("bar").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);
    assert_eq!(cache.get("xy").await.unwrap(), Some(4));
}

#[tokio::test]
async fn test_delete_by_tags_all() {
    let cache = get_cache("test_delete_by_tags_all").await;

    cache
        .put("foo", 1, &["t1", "t2", "t3"], None)
        .await
        .unwrap();
    cache.put("bar", 2, &["t1", "t2"], None).await.unwrap();
    cache.put("baz", 3, &["t1", "t3"], None).await.unwrap();
    assert_eq!(cache.get("foo").await.unwrap(), Some(1));
    assert_eq!(cache.get("bar").await.unwrap(), Some(2));
    assert_eq!(cache.get("baz").await.unwrap(), Some(3));
    cache.pop_tags(&[]).await.unwrap();
    assert_eq!(cache.get::<i32, _>("foo").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("bar").await.unwrap(), None);
    assert_eq!(cache.get::<i32, _>("baz").await.unwrap(), None);
}

async fn get_cache(namespace: &str) -> AsyncRedisCache<MultiplexedConnection> {
    let client =
        Client::open(std::env::var("REDIS_SERVER").unwrap_or("redis://127.0.0.1:6379/0".into()))
            .unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();

    static ONCE: OnceCell<()> = OnceCell::const_new();
    ONCE.get_or_init(|| async {
        redis::cmd("FLUSHDB")
            .query_async::<_, ()>(&mut conn)
            .await
            .unwrap();
    })
    .await;

    AsyncRedisCache::new(conn, namespace.into(), Duration::from_secs(20))
}
