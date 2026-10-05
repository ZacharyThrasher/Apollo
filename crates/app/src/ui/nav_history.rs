//! Back and Forward: a browser-style history of the places the window has shown.
//!
//! A [`Place`] is a page (Home, a playlist, an artist, the Library's Albums tab, Now Playing);
//! what the person did on it (where it was scrolled, what the search field held, what the
//! filter held) is its [`PlaceState`], kept up to date while the page is on screen and put back
//! when it is returned to. Going to a new place from the middle of the list drops the places
//! ahead (the browser's rule), the list holds [`CAP`] places at most, and nothing is saved: a
//! new run starts with an empty history.
//!
//! The shell drives it from `render`: it works out which place is on screen and calls
//! [`NavHistory::observe`], so every way of getting somewhere (the nav rail, a card, a tab, a
//! page's own Back button) is recorded without each one having to say so.

use gpui::{Pixels, Point};

use crate::library::model::{Tab, Target};
use crate::ui::nav::Route;

/// The most places kept; the oldest is forgotten first.
pub const CAP: usize = 50;

/// A page, as far as history is concerned. Two of them are the same place when they compare
/// equal: opening the page you are already on does not add an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Home,
    Explore,
    Search,
    /// The lists of the Library, on a tab.
    Library(Tab),
    /// An album or playlist page, by its library key.
    Collection(String),
    /// An artist page, by its browse id.
    Artist(String),
    Crate,
    Settings,
    /// Now Playing, over whatever page it was opened from.
    NowPlaying,
}

/// What is remembered about a place besides which one it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaceState {
    /// What a card would open, to open the page again (Collection and Artist places).
    pub target: Option<Target>,
    /// How far the page was scrolled.
    pub scroll: Option<Point<Pixels>>,
    /// Search: what the field held.
    pub query: Option<String>,
    /// A playlist, album or the Songs tab: what the filter field held.
    pub filter: Option<String>,
    /// The nav item that stays lit while a page opened from another route shows (see
    /// `Shell::nav_route`).
    pub origin: Option<Route>,
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub place: Place,
    pub state: PlaceState,
}

#[derive(Debug, Default)]
pub struct NavHistory {
    entries: Vec<HistoryEntry>,
    /// The entry on screen (meaningless while `entries` is empty).
    cursor: usize,
}

impl NavHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (a different account: its pages are not this one's).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn can_back(&self) -> bool {
        !self.entries.is_empty() && self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    pub fn current(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.cursor)
    }

    /// The state of the place on screen, to be kept up to date.
    pub fn current_state_mut(&mut self) -> Option<&mut PlaceState> {
        self.entries.get_mut(self.cursor).map(|e| &mut e.state)
    }

    /// The window is showing `place`. The same place as the current one changes nothing; a new
    /// one becomes current and drops everything ahead of the old one. Closing Now Playing goes
    /// back to the place it was opened over instead of adding that place again. Returns
    /// whether the current entry changed.
    pub fn observe(&mut self, place: Place, state: PlaceState) -> bool {
        if let Some(current) = self.entries.get(self.cursor) {
            if current.place == place {
                return false;
            }
            if current.place == Place::NowPlaying
                && self.cursor > 0
                && self.entries[self.cursor - 1].place == place
            {
                self.cursor -= 1;
                return true;
            }
            self.entries.truncate(self.cursor + 1);
        }
        self.entries.push(HistoryEntry { place, state });
        if self.entries.len() > CAP {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
        true
    }

    /// Step to the previous place and return it (to be shown).
    pub fn back(&mut self) -> Option<HistoryEntry> {
        if !self.can_back() {
            return None;
        }
        self.cursor -= 1;
        self.entries.get(self.cursor).cloned()
    }

    /// Step to the next place and return it (to be shown).
    pub fn forward(&mut self) -> Option<HistoryEntry> {
        if !self.can_forward() {
            return None;
        }
        self.cursor += 1;
        self.entries.get(self.cursor).cloned()
    }

    /// The places in order, oldest first, with the index of the current one (tests).
    pub fn places(&self) -> (Vec<Place>, usize) {
        (
            self.entries.iter().map(|e| e.place.clone()).collect(),
            self.cursor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(h: &mut NavHistory, place: Place) -> bool {
        h.observe(place, PlaceState::default())
    }

    #[test]
    fn a_new_place_is_added_and_the_same_place_is_not() {
        let mut h = NavHistory::new();
        assert!(seen(&mut h, Place::Home));
        assert!(!seen(&mut h, Place::Home));
        assert!(seen(&mut h, Place::Explore));
        assert_eq!(h.places(), (vec![Place::Home, Place::Explore], 1));
        assert!(h.can_back());
        assert!(!h.can_forward());
    }

    #[test]
    fn back_and_forward_walk_the_list() {
        let mut h = NavHistory::new();
        seen(&mut h, Place::Home);
        seen(&mut h, Place::Search);
        seen(&mut h, Place::Crate);
        assert_eq!(h.back().unwrap().place, Place::Search);
        assert_eq!(h.back().unwrap().place, Place::Home);
        assert!(h.back().is_none());
        assert_eq!(h.forward().unwrap().place, Place::Search);
        // Showing the place it just went to is not news.
        assert!(!seen(&mut h, Place::Search));
        assert!(h.can_forward());
    }

    #[test]
    fn a_new_place_from_the_middle_drops_the_forward_entries() {
        let mut h = NavHistory::new();
        seen(&mut h, Place::Home);
        seen(&mut h, Place::Search);
        seen(&mut h, Place::Crate);
        h.back();
        h.back();
        seen(&mut h, Place::Settings);
        assert_eq!(h.places(), (vec![Place::Home, Place::Settings], 1));
        assert!(!h.can_forward());
    }

    #[test]
    fn the_list_holds_fifty_places() {
        let mut h = NavHistory::new();
        for n in 0..(CAP + 10) {
            seen(&mut h, Place::Collection(format!("PL{n}")));
        }
        assert_eq!(h.len(), CAP);
        let (places, cursor) = h.places();
        assert_eq!(cursor, CAP - 1);
        assert_eq!(places[0], Place::Collection("PL10".into()));
        assert_eq!(places[CAP - 1], Place::Collection(format!("PL{}", CAP + 9)));
    }

    #[test]
    fn closing_now_playing_goes_back_instead_of_adding_the_page_again() {
        let mut h = NavHistory::new();
        seen(&mut h, Place::Home);
        seen(&mut h, Place::Search);
        seen(&mut h, Place::NowPlaying);
        assert!(seen(&mut h, Place::Search));
        assert_eq!(h.places(), (vec![Place::Home, Place::Search, Place::NowPlaying], 1));
        // Forward reopens it.
        assert_eq!(h.forward().unwrap().place, Place::NowPlaying);
        // Leaving it for some other page is a new place, as ever.
        assert!(seen(&mut h, Place::Crate));
        assert_eq!(
            h.places(),
            (
                vec![Place::Home, Place::Search, Place::NowPlaying, Place::Crate],
                3
            )
        );
    }

    #[test]
    fn what_a_place_remembers_comes_back_with_it() {
        let mut h = NavHistory::new();
        seen(&mut h, Place::Search);
        h.current_state_mut().unwrap().query = Some("daft".into());
        seen(&mut h, Place::Home);
        let back = h.back().unwrap();
        assert_eq!(back.place, Place::Search);
        assert_eq!(back.state.query.as_deref(), Some("daft"));
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut h = NavHistory::new();
        seen(&mut h, Place::Home);
        seen(&mut h, Place::Search);
        h.clear();
        assert!(h.is_empty());
        assert!(!h.can_back() && !h.can_forward());
        assert!(h.back().is_none());
    }
}
