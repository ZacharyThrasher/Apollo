//! Small artwork for queue rows, loaded only for rows that are on screen.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::RenderImage;

enum Slot {
    Loading,
    /// `None` = the track has no usable artwork.
    Ready(Option<Arc<RenderImage>>),
}

#[derive(Default)]
pub struct Thumbs {
    slots: HashMap<String, Slot>,
}

impl Thumbs {
    pub fn get(&self, video_id: &str) -> Option<Arc<RenderImage>> {
        match self.slots.get(video_id) {
            Some(Slot::Ready(image)) => image.clone(),
            _ => None,
        }
    }

    /// Marks every id not seen before as loading and returns those, each once.
    pub fn take_missing(&mut self, ids: Vec<String>) -> Vec<String> {
        let mut missing = Vec::new();
        for id in ids {
            if !self.slots.contains_key(&id) {
                self.slots.insert(id.clone(), Slot::Loading);
                missing.push(id);
            }
        }
        missing
    }

    pub fn finish(&mut self, video_id: String, image: Option<Arc<RenderImage>>) {
        self.slots.insert(video_id, Slot::Ready(image));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_id_is_requested_once_and_missing_art_stays_none() {
        let mut t = Thumbs::default();
        assert_eq!(
            t.take_missing(vec!["a".into(), "b".into(), "a".into()]),
            ["a", "b"]
        );
        assert!(t.take_missing(vec!["a".into()]).is_empty());
        assert!(t.get("a").is_none());
        t.finish("a".into(), None);
        assert!(t.get("a").is_none());
        assert!(t.take_missing(vec!["a".into()]).is_empty());
    }
}
