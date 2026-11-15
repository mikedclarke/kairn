use std::{collections::HashMap, sync::Arc};

use futures::FutureExt;
use gpui::{
    AnyImageCache, App, AppContext, Asset, ElementId, ImageAssetLoader, ImageCache,
    ImageCacheError, ImageCacheItem, RenderImage, Resource, Window,
};
use image::{Frame, ImageBuffer, Rgba};

pub(crate) struct Thumbnails(pub ElementId);

impl gpui::ImageCacheProvider for Thumbnails {
    fn provide(&mut self, window: &mut Window, cx: &mut App) -> AnyImageCache {
        window.with_global_id(self.0.clone(), |id, window| {
            window.with_element_state(id, |cache, _| {
                let cache = cache.unwrap_or_else(|| {
                    let cache = cx.new(|_| ThumbnailCache::default());
                    cx.observe_release(&cache, |cache, cx| {
                        for (_, mut item) in cache.0.drain() {
                            if let Some(Ok(image)) = item.get() {
                                cx.drop_image(image, None);
                            }
                        }
                    })
                    .detach();
                    cache
                });
                (AnyImageCache::from(cache.clone()), cache)
            })
        })
    }
}

#[derive(Default)]
struct ThumbnailCache(HashMap<Resource, ImageCacheItem>);

impl ImageCache for ThumbnailCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        if let Some(item) = self.0.get_mut(resource) {
            return item.get();
        }

        let load = ImageAssetLoader::load(resource.clone(), cx);
        let task = cx
            .background_executor()
            .spawn(async move { load.await.map(|image| Arc::new(thumbnail(&image))) })
            .shared();
        self.0
            .insert(resource.clone(), ImageCacheItem::Loading(task.clone()));
        let view = window.current_view();
        window
            .spawn(cx, async move |cx| {
                _ = task.await;
                cx.on_next_frame(move |_, cx| cx.notify(view));
            })
            .detach();
        None
    }
}

fn thumbnail(image: &RenderImage) -> RenderImage {
    let size = image.size(0);
    let pixels = ImageBuffer::<Rgba<u8>, _>::from_raw(
        size.width.0 as u32,
        size.height.0 as u32,
        image.as_bytes(0).expect("decoded image has a first frame"),
    )
    .expect("decoded image dimensions match its pixels");
    let scale = (208.0 / pixels.width() as f64)
        .min(156.0 / pixels.height() as f64)
        .min(1.0);
    let width = (pixels.width() as f64 * scale).round().max(1.0) as u32;
    let height = (pixels.height() as f64 * scale).round().max(1.0) as u32;
    let buffer = image::imageops::thumbnail(&pixels, width, height);
    RenderImage::new(vec![Frame::new(buffer)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_bound_decoded_memory_and_keep_aspect_ratio() {
        for (width, height, expected) in [
            (1600, 1000, (208, 130)),
            (1000, 1600, (98, 156)),
            (1, 2000, (1, 156)),
        ] {
            let image = RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
                width,
                height,
                Rgba([30, 60, 90, 255]),
            ))]);
            let thumbnail = thumbnail(&image);
            assert_eq!(
                (thumbnail.size(0).width.0, thumbnail.size(0).height.0),
                expected
            );
            assert!(thumbnail.as_bytes(0).unwrap().len() <= 208 * 156 * 4);
            assert_eq!(&thumbnail.as_bytes(0).unwrap()[..4], &[30, 60, 90, 255]);
        }
    }

    #[test]
    fn thumbnails_keep_small_images_small_and_only_retain_one_frame() {
        let image = RenderImage::new(vec![
            Frame::new(ImageBuffer::from_pixel(20, 10, Rgba([30, 60, 90, 255]))),
            Frame::new(ImageBuffer::from_pixel(20, 10, Rgba([90, 60, 30, 255]))),
        ]);
        let thumbnail = thumbnail(&image);
        assert_eq!(thumbnail.frame_count(), 1);
        assert_eq!(
            (thumbnail.size(0).width.0, thumbnail.size(0).height.0),
            (20, 10)
        );
        assert_eq!(&thumbnail.as_bytes(0).unwrap()[..4], &[30, 60, 90, 255]);
    }
}
