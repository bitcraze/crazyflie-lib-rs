/// Example demonstrating a simple in-memory TOC cache implementation
/// and measuring connection times with and without TOC caching.
use crazyflie_lib::{Crazyflie, TocCache};
use crazyflie_link::LinkContext;
use std::{collections::HashMap, sync::Arc, sync::RwLock};
use tokio::time::{sleep, Duration};

#[derive(Clone)]
struct InMemoryTocCache {
  toc: Arc<RwLock<HashMap<u32, String>>>,
}

impl InMemoryTocCache {
  fn new() -> Self {
    InMemoryTocCache {
      toc: Arc::new(RwLock::new(HashMap::new())),
    }
  }
}

impl TocCache for InMemoryTocCache {
  fn get_toc(&self, crc32: u32) -> Option<String> {
    self.toc.read().ok()?.get(&crc32).cloned()
  }

  fn store_toc(&self, crc32: u32, toc: &str) {
    if let Ok(mut lock) = self.toc.write() {
      lock.insert(crc32, toc.to_string());
    }
  }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let uri = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "radio://0/80/2M/E7E7E7E7E7".to_string());
    let toc_cache = InMemoryTocCache::new();

    let context = LinkContext::new();
    print!("1st connection ...");
    let start = std::time::Instant::now();

    let cf = Crazyflie::connect_from_uri(&context, &uri, toc_cache.clone()).await?;

    println!(" {:?}", start.elapsed());
    drop(cf);
    sleep(Duration::from_millis(500)).await;

    print!("2nd connection ...");
    let start = std::time::Instant::now();

    let cf = Crazyflie::connect_from_uri(&context, &uri, toc_cache.clone()).await?;

    println!(" {:?}", start.elapsed());
    drop(cf);
    sleep(Duration::from_millis(500)).await;

    print!("3rd connection ...");
    let start = std::time::Instant::now();

    let _cf = Crazyflie::connect_from_uri(&context, &uri, toc_cache.clone()).await?;

    println!(" {:?}", start.elapsed());

    Ok(())
}
