//! The Stats tab's state: the report for the chosen period, worked out from the local play log
//! off the UI thread. Lives under the model so it can reach its private state.

use std::sync::Arc;

use gpui::Context;

use super::{LibraryModel, Load};
use crate::page_cache::PageCache;
use crate::stats::report::{self, Period, StatsReport};
use crate::stats::PlayLog;

impl LibraryModel {
    pub fn stats(&self) -> &Load<Arc<StatsReport>> {
        &self.stats
    }

    pub fn stats_period(&self) -> Period {
        self.stats_period
    }

    /// How many rows each of the three lists (tracks, artists, albums) has on screen.
    pub fn stats_lens(&self) -> [usize; 3] {
        match &self.stats {
            Load::Ready(r) => [r.tracks.len(), r.artists.len(), r.albums.len()],
            _ => [0; 3],
        }
    }

    /// Rows the keyboard can select on the Stats tab.
    pub fn stats_row_count(&self) -> usize {
        self.stats_lens().iter().sum()
    }

    /// Chooses the period and works the report out again; what is on screen stays until it is.
    pub fn set_stats_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if self.stats_period == period {
            return;
        }
        self.stats_period = period;
        self.load_stats(cx);
        cx.notify();
    }

    /// Reads the play log for the period. Local and quick: with no log or no account the report
    /// is simply empty.
    pub(super) fn load_stats(&mut self, cx: &mut Context<Self>) {
        let period = self.stats_period;
        self.stats_rev += 1;
        let rev = self.stats_rev;
        let log = PlayLog::try_global(cx);
        let account = PageCache::try_global(cx).and_then(|c| c.account());
        let (Some(log), Some(account)) = (log, account) else {
            self.stats = Load::Ready(Arc::new(StatsReport::default()));
            cx.notify();
            return;
        };
        if !matches!(self.stats, Load::Ready(_)) {
            self.stats = Load::Loading;
        }
        cx.spawn(async move |this, cx| {
            let asked = account.clone();
            let built = cx
                .background_executor()
                .spawn(async move {
                    let now = apollo_services_common::unix_now() as i64;
                    report::build(&log.totals(&asked, period.since(now)))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let current = PageCache::try_global(cx).and_then(|c| c.account());
                if m.stats_rev != rev || current.as_deref() != Some(account.as_str()) {
                    return;
                }
                m.stats = Load::Ready(Arc::new(built));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
