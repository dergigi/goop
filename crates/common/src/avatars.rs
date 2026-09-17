//! Shared, size-aware avatar thumbnails. All decoding and resizing runs off the UI thread.
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use async_lock::Semaphore;
use futures::{FutureExt, future::Either};
use gpui::{
    App, AppContext, Asset, AssetLogger, Context, Entity, Global, ImageAssetLoader, ImageCache,
    ImageCacheError, RenderImage, Resource, Window, hash,
};
use image::{
    Frame, RgbaImage,
    imageops::{self, FilterType},
};
use smallvec::SmallVec;

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY: Duration = Duration::from_secs(30);
type Key = (u64, u32);
type Pair = [Arc<RenderImage>; 2];

struct Avatars {
    store: Entity<ThumbnailStore>,
    adapters: HashMap<(u32, bool), Entity<AvatarImageCache>>,
}
impl Global for Avatars {}

/// Two logical size classes, scaled to physical pixels for the current display.
/// Both sizes share one fetch/decode task and one cache entry.
pub fn avatar_cache(logical_size: f32, scale: f32, cx: &mut App) -> Entity<AvatarImageCache> {
    if cx.try_global::<Avatars>().is_none() {
        let store = cx.new(|cx| {
            cx.on_release(|store: &mut ThumbnailStore, cx| store.clear(cx))
                .detach();
            ThumbnailStore::default()
        });
        cx.set_global(Avatars {
            store,
            adapters: HashMap::new(),
        });
    }
    // Quantizing avoids cache proliferation from floating-point scale factors.
    let scale = (scale.clamp(1., 4.) * 4.).ceil() as u32;
    let large = logical_size > 32.;
    let key = (scale, large);
    if let Some(adapter) = cx.global::<Avatars>().adapters.get(&key) {
        return adapter.clone();
    }
    let store = cx.global::<Avatars>().store.clone();
    let adapter = cx.new(|_| AvatarImageCache {
        store,
        scale,
        large,
    });
    cx.global_mut::<Avatars>()
        .adapters
        .insert(key, adapter.clone());
    adapter
}

pub struct AvatarImageCache {
    store: Entity<ThumbnailStore>,
    scale: u32,
    large: bool,
}
impl ImageCache for AvatarImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.store.update(cx, |store, cx| {
            store.load(resource, self.scale, self.large, cx)
        })
    }
}

enum Entry {
    Loading,
    Ready {
        result: Result<Pair, ImageCacheError>,
        at: Instant,
        bytes: usize,
    },
}

struct ThumbnailStore {
    entries: HashMap<Key, Entry>,
    usage: VecDeque<Key>,
    bytes: usize,
    workers: Arc<Semaphore>,
}
impl Default for ThumbnailStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            usage: VecDeque::new(),
            bytes: 0,
            workers: Arc::new(Semaphore::new(2)),
        }
    }
}
impl ThumbnailStore {
    fn remove(&mut self, key: Key, cx: &mut App) {
        self.usage.retain(|item| *item != key);
        if let Some(Entry::Ready { result, bytes, .. }) = self.entries.remove(&key) {
            self.bytes -= bytes;
            if let Ok(images) = result {
                for image in images {
                    cx.drop_image(image, None);
                }
            }
        }
    }

    fn clear(&mut self, cx: &mut App) {
        for key in self.usage.clone() {
            self.remove(key, cx);
        }
    }

    fn evict_oldest_ready(&mut self, keep: Option<Key>, cx: &mut App) -> bool {
        let key = self.usage.iter().rev().copied().find(|key| {
            Some(*key) != keep && matches!(self.entries.get(key), Some(Entry::Ready { .. }))
        });
        if let Some(key) = key {
            self.remove(key, cx);
            true
        } else {
            false
        }
    }

    fn load(
        &mut self,
        resource: &Resource,
        scale: u32,
        large: bool,
        cx: &mut Context<Self>,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let key = (hash(resource), scale);
        if let Some(entry) = self.entries.get(&key) {
            match entry {
                Entry::Loading => return None,
                Entry::Ready { result, at, .. }
                    if at.elapsed() < if result.is_ok() { TTL } else { RETRY } =>
                {
                    self.usage.retain(|item| *item != key);
                    self.usage.push_front(key);
                    return Some(
                        result
                            .as_ref()
                            .map(|images| images[usize::from(large)].clone())
                            .map_err(Clone::clone),
                    );
                }
                _ => self.remove(key, cx),
            }
        }
        if self.entries.len() >= MAX_ENTRIES && !self.evict_oldest_ready(None, cx) {
            // All slots are loading. Completion refreshes the UI and admits more work.
            return None;
        }
        let load = AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx);
        let workers = self.workers.clone();
        let executor = cx.background_executor().clone();
        let work = cx.background_spawn(async move {
            let _permit = workers.acquire().await;
            let timeout = executor.timer(Duration::from_secs(30));
            let source = match futures::future::select(load.boxed(), timeout.boxed()).await {
                Either::Left((result, _)) => result?,
                Either::Right(_) => {
                    return Err(ImageCacheError::Asset("Avatar request timed out".into()));
                }
            };
            // The full-resolution decoded image is released after resizing.
            thumbnails(&source, [8 * scale, 16 * scale])
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let failed = result.is_err();
            let _ = this.update(cx, |store, cx| {
                let bytes = result.as_ref().map_or(0, |images| {
                    images
                        .iter()
                        .map(|image| {
                            (0..image.frame_count())
                                .map(|frame| image.as_bytes(frame).map_or(0, <[u8]>::len))
                                .sum::<usize>()
                        })
                        .sum()
                });
                store.bytes += bytes;
                store.entries.insert(
                    key,
                    Entry::Ready {
                        result,
                        at: Instant::now(),
                        bytes,
                    },
                );
                while store.bytes > MAX_BYTES && store.evict_oldest_ready(Some(key), cx) {}
                cx.refresh_windows();
            });
            if failed {
                cx.background_executor()
                    .timer(RETRY + Duration::from_secs(1))
                    .await;
                let _ = cx.update(|cx| cx.refresh_windows());
            }
        })
        .detach();
        self.entries.insert(key, Entry::Loading);
        self.usage.push_front(key);
        None
    }
}

fn thumbnails(source: &RenderImage, sizes: [u32; 2]) -> Result<Pair, ImageCacheError> {
    let mut frames: [SmallVec<[Frame; 1]>; 2] = Default::default();
    // Keep unusually large animations from consuming the entire thumbnail cache.
    // Ordinary animations retain every frame and their original timing.
    let bytes = (0..source.frame_count()).fold(0usize, |total, index| {
        let size = source.size(index);
        let side = (size.width.0 as u32).min(size.height.0 as u32);
        total.saturating_add(
            sizes
                .iter()
                .map(|edge| {
                    let edge = (*edge).min(side) as usize;
                    edge * edge * 4
                })
                .sum::<usize>(),
        )
    });
    let frame_count = if bytes > MAX_BYTES / 4 {
        1
    } else {
        source.frame_count()
    };
    for index in 0..frame_count {
        let size = source.size(index);
        let pixels = source
            .as_bytes(index)
            .ok_or_else(|| ImageCacheError::Asset("Missing avatar frame".into()))?;
        let image = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
            size.width.0 as u32,
            size.height.0 as u32,
            pixels,
        )
        .ok_or_else(|| ImageCacheError::Asset("Invalid avatar dimensions".into()))?;
        for (frames, edge) in frames.iter_mut().zip(sizes) {
            frames.push(Frame::from_parts(
                resize(&image, edge),
                0,
                0,
                source.delay(index),
            ));
        }
    }
    Ok(frames.map(|frames| Arc::new(RenderImage::new(frames))))
}

fn resize(source: &impl image::GenericImageView<Pixel = image::Rgba<u8>>, edge: u32) -> RgbaImage {
    let side = source.width().min(source.height());
    let left = (source.width() - side) / 2;
    let top = (source.height() - side) / 2;
    let mut square = RgbaImage::from_fn(side, side, |x, y| source.get_pixel(left + x, top + y));
    if side <= edge {
        return square;
    } // Never enlarge low-resolution originals.
    // Premultiply before filtering to keep transparent edges free of color halos.
    // GPUI's decoded pixels are BGRA; filtering treats all three color channels alike.
    for pixel in square.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = ((u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255) as u8;
        }
    }
    let mut result = imageops::resize(&square, edge, edge, FilterType::Lanczos3);
    for pixel in result.pixels_mut() {
        if pixel[3] != 0 {
            for channel in 0..3 {
                pixel[channel] = ((u32::from(pixel[channel]) * 255 + u32::from(pixel[3]) / 2)
                    / u32::from(pixel[3]))
                .min(255) as u8;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Rgba};

    #[test]
    fn detailed_original_is_filtered_instead_of_aliasing() {
        let image = RgbaImage::from_fn(512, 512, |x, y| {
            let value = if (x + y) % 2 == 0 { 0 } else { 255 };
            Rgba([value, value, value, 255])
        });
        let result = resize(&image, 48);
        assert_eq!(result.dimensions(), (48, 48));
        for pixel in result.pixels() {
            assert!((120..=135).contains(&pixel[0]));
        }
    }

    #[test]
    fn preserves_animation_timing_color_and_does_not_upscale() {
        let delay = Delay::from_numer_denom_ms(150, 1);
        let source = RenderImage::new(smallvec::smallvec![
            Frame::from_parts(
                RgbaImage::from_pixel(256, 128, Rgba([12, 34, 56, 255])),
                0,
                0,
                delay
            ),
            Frame::from_parts(
                RgbaImage::from_pixel(256, 128, Rgba([90, 80, 70, 255])),
                0,
                0,
                delay
            ),
        ]);
        let images = thumbnails(&source, [64, 256]).unwrap();
        assert_eq!(images[0].size(0).width.0, 64);
        assert_eq!(images[1].size(0).width.0, 128);
        for image in images {
            assert_eq!(image.frame_count(), 2);
            assert_eq!(image.delay(1), delay);
            assert_eq!(&image.as_bytes(0).unwrap()[..4], &[12, 34, 56, 255]);
            assert_eq!(&image.as_bytes(1).unwrap()[..4], &[90, 80, 70, 255]);
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_their_color_into_edges() {
        let image = RgbaImage::from_fn(128, 128, |x, _| {
            if x < 64 {
                Rgba([0, 0, 255, 255])
            } else {
                Rgba([255, 0, 0, 0])
            }
        });
        for pixel in resize(&image, 32).pixels() {
            if pixel[3] > 10 {
                assert_eq!(pixel[0], 0);
                assert!(pixel[2] > 245);
            }
        }
    }

    #[test]
    fn crops_the_center_without_stretching() {
        let image = RgbaImage::from_fn(160, 80, |x, _| {
            if (40..120).contains(&x) {
                Rgba([20, 40, 60, 255])
            } else {
                Rgba([255, 0, 0, 255])
            }
        });
        let result = resize(&image, 40);
        assert!(
            result
                .pixels()
                .all(|pixel| *pixel == Rgba([20, 40, 60, 255]))
        );
    }

    #[test]
    fn oversized_animations_cannot_exhaust_the_thumbnail_cache() {
        let source = RenderImage::new(
            (0..100)
                .map(|_| Frame::new(RgbaImage::from_pixel(128, 128, Rgba([10, 20, 30, 255]))))
                .collect::<SmallVec<[Frame; 1]>>(),
        );
        let result = thumbnails(&source, [128, 256]).unwrap();
        assert_eq!(result[0].frame_count(), 1);
        assert_eq!(result[1].frame_count(), 1);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod cache_tests {
    use super::*;

    #[gpui::test]
    fn sizes_share_a_load_and_original_is_not_retained(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("avatar.png");
        std::fs::write(&path, include_bytes!("../../../assets/brand/avatar.png")).unwrap();
        let resource = Resource::Path(path.into());
        let store = cx.new(|_| ThumbnailStore::default());
        store.update(cx, |store, cx| {
            assert!(store.load(&resource, 8, false, cx).is_none());
            assert!(store.load(&resource, 8, true, cx).is_none());
            assert_eq!(store.entries.len(), 1);
        });
        cx.run_until_parked();
        store.update(cx, |store, cx| {
            let small = store.load(&resource, 8, false, cx).unwrap().unwrap();
            let large = store.load(&resource, 8, true, cx).unwrap().unwrap();
            assert_eq!(small.size(0).width.0, 64);
            assert_eq!(large.size(0).width.0, 128);
            let again = store.load(&resource, 8, false, cx).unwrap().unwrap();
            assert!(Arc::ptr_eq(&small, &again));
            assert_eq!(store.entries.len(), 1);
            assert_eq!(store.bytes, (64 * 64 + 128 * 128) * 4);
            store.clear(cx);
            assert!(store.entries.is_empty());
            assert_eq!(store.bytes, 0);
        });
    }

    #[gpui::test]
    fn errors_are_cached_and_retry_after_cooldown(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let resource = Resource::Path(directory.path().join("missing.png").into());
        let store = cx.new(|_| ThumbnailStore::default());
        store.update(cx, |store, cx| {
            assert!(store.load(&resource, 8, false, cx).is_none());
        });
        cx.run_until_parked();
        store.update(cx, |store, cx| {
            assert!(store.load(&resource, 8, true, cx).unwrap().is_err());
            let key = (hash(&resource), 8);
            if let Some(Entry::Ready { at, .. }) = store.entries.get_mut(&key) {
                *at = Instant::now() - RETRY;
            } else {
                panic!("Expected cached error");
            }
            assert!(store.load(&resource, 8, true, cx).is_none());
            assert_eq!(store.entries.len(), 1);
        });
        cx.run_until_parked();
    }
}
