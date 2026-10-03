//! Small artwork for lists, keyed by the thumbnail URL a listing carries. Fetching is bounded
//! (a few at a time) and off the UI thread; a URL that fails stays failed for the session, so
//! a broken image is never retried on every repaint.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui::{Context, RenderImage};

use crate::art::DecodedArt;

/// Fetches and decodes one image. Blocking; the store runs it on a background thread.
pub trait ThumbSource: Send + Sync {
    fn fetch(&self, url: &str) -> Option<DecodedArt>;
}

/// Stands in when the backend could not start: every image is missing.
pub struct NoThumbs;

impl ThumbSource for NoThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

/// Downloads in flight at once.
const MAX_ACTIVE: usize = 6;
/// Decoded images kept; the oldest go first.
const MAX_CACHED: usize = 128;

#[derive(Clone)]
pub enum Thumb {
    /// Not requested, or still downloading.
    Pending,
    /// Failed to download or decode: show the placeholder tile.
    Missing,
    Image(Arc<RenderImage>),
}

enum Slot {
    Pending,
    Missing,
    Image(Arc<RenderImage>),
}

pub struct ThumbStore {
    source: Option<Arc<dyn ThumbSource>>,
    slots: HashMap<String, Slot>,
    /// Requested before the source existed or while the pool was full.
    waiting: VecDeque<String>,
    /// Finished images, oldest first, for eviction.
    finished: VecDeque<String>,
    active: usize,
}

impl ThumbStore {
    pub fn new() -> Self {
        Self {
            source: None,
            slots: HashMap::new(),
            waiting: VecDeque::new(),
            finished: VecDeque::new(),
            active: 0,
        }
    }

    pub fn install(&mut self, source: Arc<dyn ThumbSource>, cx: &mut Context<Self>) {
        self.source = Some(source);
        self.pump(cx);
    }

    pub fn get(&self, url: &str) -> Thumb {
        match self.slots.get(url) {
            None | Some(Slot::Pending) => Thumb::Pending,
            Some(Slot::Missing) => Thumb::Missing,
            Some(Slot::Image(img)) => Thumb::Image(img.clone()),
        }
    }

    /// Ask for an image. A URL already requested is left alone.
    pub fn request(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.slots.contains_key(url) {
            return;
        }
        self.slots.insert(url.to_owned(), Slot::Pending);
        self.waiting.push_back(url.to_owned());
        self.pump(cx);
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        while self.active < MAX_ACTIVE {
            let Some(url) = self.waiting.pop_front() else {
                break;
            };
            self.active += 1;
            let source = source.clone();
            cx.spawn(async move |this, cx| {
                let fetched = url.clone();
                let image = cx
                    .background_executor()
                    .spawn(async move { source.fetch(&fetched).map(|art| art.render_image()) })
                    .await;
                let _ = this.update(cx, |store, cx| store.finish(url, image, cx));
            })
            .detach();
        }
    }

    fn finish(&mut self, url: String, image: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
        self.active = self.active.saturating_sub(1);
        self.slots.insert(
            url.clone(),
            match image {
                Some(img) => Slot::Image(img),
                None => Slot::Missing,
            },
        );
        self.finished.push_back(url);
        while self.finished.len() > MAX_CACHED {
            if let Some(old) = self.finished.pop_front() {
                self.slots.remove(&old);
            }
        }
        self.pump(cx);
        cx.notify();
    }
}

impl Default for ThumbStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{AppContext, TestAppContext};

    use super::*;
    use crate::art::SyntheticArt;

    struct Counting(AtomicUsize);

    impl ThumbSource for Counting {
        fn fetch(&self, url: &str) -> Option<DecodedArt> {
            self.0.fetch_add(1, Ordering::SeqCst);
            (!url.contains("broken"))
                .then(|| DecodedArt::from_rgba(SyntheticArt::Hue(10.0).render(16)))
        }
    }

    #[gpui::test]
    fn a_url_is_fetched_once_and_a_failure_is_remembered(cx: &mut TestAppContext) {
        let source = Arc::new(Counting(AtomicUsize::new(0)));
        let store = cx.new(|_| ThumbStore::new());
        store.update(cx, |s, cx| {
            // Requested before the source exists: waits for it.
            s.request("https://t/a", cx);
            s.install(source.clone(), cx);
            s.request("https://t/a", cx);
            s.request("https://t/broken", cx);
            s.request("https://t/broken", cx);
        });
        cx.run_until_parked();
        assert_eq!(source.0.load(Ordering::SeqCst), 2);
        store.read_with(cx, |s, _| {
            assert!(matches!(s.get("https://t/a"), Thumb::Image(_)));
            assert!(matches!(s.get("https://t/broken"), Thumb::Missing));
            assert!(matches!(s.get("https://t/never-asked"), Thumb::Pending));
        });
    }

    #[gpui::test]
    fn the_cache_is_bounded_and_drops_the_oldest(cx: &mut TestAppContext) {
        let source = Arc::new(Counting(AtomicUsize::new(0)));
        let store = cx.new(|_| ThumbStore::new());
        store.update(cx, |s, cx| s.install(source.clone(), cx));
        for i in 0..MAX_CACHED + 5 {
            store.update(cx, |s, cx| s.request(&format!("https://t/{i}"), cx));
            cx.run_until_parked();
        }
        store.read_with(cx, |s, _| {
            assert_eq!(s.slots.len(), MAX_CACHED);
            assert!(
                matches!(s.get("https://t/0"), Thumb::Pending),
                "oldest evicted"
            );
            assert!(matches!(
                s.get(&format!("https://t/{}", MAX_CACHED + 4)),
                Thumb::Image(_)
            ));
        });
    }
}
