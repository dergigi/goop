use std::collections::{HashMap, VecDeque};
use std::mem::take;

use futures::FutureExt;
use gpui::{
    App, AppContext, Asset, AssetLogger, ElementId, Entity, Global, ImageAssetLoader, ImageCache,
    ImageCacheItem, ImageCacheProvider, ImageSource, Resource, hash,
};
use instant::{Duration, Instant};

/// Avatars share one decoded image and one in-flight request across every view.
struct AvatarCache(Entity<GoopImageCache>);
impl Global for AvatarCache {}

pub fn avatar_cache(cx: &mut App) -> Entity<GoopImageCache> {
    if let Some(cache) = cx.try_global::<AvatarCache>() {
        return cache.0.clone();
    }
    let cache = GoopImageCache::new(256, cx);
    cx.set_global(AvatarCache(cache.clone()));
    cache
}

const RETRY_DELAY: Duration = Duration::from_secs(30);
const IMAGE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn goop_cache(id: impl Into<ElementId>, max_items: usize) -> GoopImageCacheProvider {
    GoopImageCacheProvider {
        id: id.into(),
        max_items,
    }
}

pub struct GoopImageCacheProvider {
    id: ElementId,
    max_items: usize,
}

impl ImageCacheProvider for GoopImageCacheProvider {
    fn provide(&mut self, window: &mut gpui::Window, cx: &mut App) -> gpui::AnyImageCache {
        window
            .with_global_id(self.id.clone(), |id, window| {
                window.with_element_state(id, |cache, _| {
                    let cache = cache.unwrap_or_else(|| GoopImageCache::new(self.max_items, cx));
                    (cache.clone(), cache)
                })
            })
            .into()
    }
}

pub struct GoopImageCache {
    max_items: usize,
    usage_list: VecDeque<u64>,
    loaded_at: HashMap<u64, Instant>,
    failed_at: HashMap<u64, Instant>,
    cache: HashMap<u64, (ImageCacheItem, Resource)>,
}

impl GoopImageCache {
    pub fn new(max_items: usize, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            log::info!("Creating GoopImageCache");
            cx.on_release(|this: &mut Self, cx| {
                for (ix, (mut image, resource)) in take(&mut this.cache) {
                    if let Some(Ok(image)) = image.get() {
                        log::info!("Dropping image {ix}");
                        cx.drop_image(image, None);
                    }
                    ImageSource::Resource(resource).remove_asset(cx);
                }
            })
            .detach();

            GoopImageCache {
                max_items: max_items.max(1),
                loaded_at: HashMap::new(),
                failed_at: HashMap::new(),
                usage_list: VecDeque::with_capacity(max_items),
                cache: HashMap::with_capacity(max_items),
            }
        })
    }
}

impl ImageCache for GoopImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Result<std::sync::Arc<gpui::RenderImage>, gpui::ImageCacheError>> {
        let hash = hash(resource);

        if let Some(item) = self.cache.get_mut(&hash) {
            let result = item.0.get();
            let expired = match &result {
                Some(Err(_)) => {
                    self.failed_at
                        .entry(hash)
                        .or_insert_with(Instant::now)
                        .elapsed()
                        >= RETRY_DELAY
                }
                Some(Ok(_)) => {
                    self.loaded_at
                        .entry(hash)
                        .or_insert_with(Instant::now)
                        .elapsed()
                        >= IMAGE_TTL
                }
                None => false,
            };
            self.usage_list.retain(|key| *key != hash);
            if !expired {
                self.usage_list.push_front(hash);
                return result;
            }
            if let Some(Ok(image)) = result {
                cx.drop_image(image, None);
            }
            self.cache.remove(&hash);
            self.loaded_at.remove(&hash);
            self.failed_at.remove(&hash);
            ImageSource::Resource(resource.clone()).remove_asset(cx);
        }

        let load_future = AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx);
        let task = cx.background_executor().spawn(load_future).shared();

        if self.usage_list.len() >= self.max_items {
            log::info!("Image cache is full, evicting oldest item");

            if let Some(oldest) = self.usage_list.pop_back() {
                self.loaded_at.remove(&oldest);
                self.failed_at.remove(&oldest);
                let mut image = self
                    .cache
                    .remove(&oldest)
                    .expect("usage_list has an item cache doesn't");

                if let Some(Ok(image)) = image.0.get() {
                    log::info!("requesting image to be dropped");
                    cx.drop_image(image, None);
                }

                ImageSource::Resource(image.1).remove_asset(cx);
            }
        }

        self.cache.insert(
            hash,
            (
                gpui::ImageCacheItem::Loading(task.clone()),
                resource.clone(),
            ),
        );
        self.usage_list.push_front(hash);

        window
            .spawn(cx, async move |cx| {
                let failed = task.await.is_err();
                // Every consumer must redraw, not just the view that began the request.
                cx.on_next_frame(move |_, cx| cx.refresh_windows());
                if failed {
                    // The first redraw starts the cooldown. A small grace period ensures
                    // it has elapsed before the next render retries the failed request.
                    cx.background_executor()
                        .timer(RETRY_DELAY + Duration::from_secs(1))
                        .await;
                    cx.on_next_frame(move |_, cx| cx.refresh_windows());
                }
            })
            .detach();

        None
    }
}
