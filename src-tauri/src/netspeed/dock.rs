//! Where the speed bar sits on the Windows taskbar: in a free stretch between
//! the taskbar's buttons, as close as it can be to where the person put it.
//! That spot is kept as a distance from the notification area (or from the
//! taskbar's left end, whichever it was dropped nearer), so it stays beside
//! the clock when tray icons come and go or the screen changes. Pure, so it
//! is tested on every platform.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::{Deserialize, Serialize};

/// Which end of the taskbar the bar keeps its distance from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    /// From the taskbar's left end to the bar's left edge.
    Left,
    /// From the bar's right edge to the notification area.
    Tray,
}

/// Where the person put the bar, in logical pixels from its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dock {
    pub anchor: Anchor,
    pub px: i32,
}

/// Beside the clock: this many logical pixels from the notification area.
pub const BESIDE_THE_CLOCK: Dock = Dock { anchor: Anchor::Tray, px: 4 };

/// A stretch of the taskbar, `lo..hi` in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub lo: i32,
    pub hi: i32,
}

/// The taskbar as far as placing the bar goes, in physical pixels.
#[derive(Debug, Clone)]
pub struct Layout {
    pub left: i32,
    pub right: i32,
    /// Where the notification area starts.
    pub tray_left: i32,
    /// What must not be covered: every button, the notification area too.
    pub occupied: Vec<Span>,
    /// The bar's width.
    pub w: i32,
    /// Kept clear on either side of the bar.
    pub margin: i32,
    /// Physical pixels per logical one.
    pub scale: f64,
}

impl Layout {
    fn physical(&self, logical: i32) -> i32 {
        (logical.max(0) as f64 * self.scale).round() as i32
    }

    fn logical(&self, physical: i32) -> i32 {
        (physical.max(0) as f64 / self.scale).round() as i32
    }

    /// Where the person wants it, before anything is avoided.
    pub fn preferred_x(&self, dock: Option<Dock>) -> i32 {
        let d = dock.unwrap_or(BESIDE_THE_CLOCK);
        let x = match d.anchor {
            Anchor::Tray => self.tray_left - self.physical(d.px) - self.w,
            Anchor::Left => self.left + self.physical(d.px),
        };
        x.clamp(self.left, (self.right - self.w).max(self.left))
    }

    /// The free stretches of the taskbar, left to right.
    pub fn gaps(&self) -> Vec<Span> {
        let mut taken: Vec<Span> = self.occupied.iter().copied().filter(|s| s.hi > s.lo).collect();
        taken.sort_by_key(|s| s.lo);
        let mut gaps = Vec::new();
        let mut at = self.left;
        for s in taken {
            if s.lo > at {
                gaps.push(Span { lo: at, hi: s.lo.min(self.right) });
            }
            at = at.max(s.hi);
        }
        if at < self.right {
            gaps.push(Span { lo: at, hi: self.right });
        }
        gaps.retain(|g| g.hi > g.lo);
        gaps
    }

    /// The x nearest to `want` where the bar fits in a free stretch with its
    /// margins; None when it fits nowhere.
    pub fn place(&self, want: i32) -> Option<i32> {
        self.gaps()
            .iter()
            .filter_map(|g| {
                let (lo, hi) = (g.lo + self.margin, g.hi - self.margin - self.w);
                (hi >= lo).then(|| want.clamp(lo, hi))
            })
            .min_by_key(|x| (x - want).abs())
    }

    /// Whether the bar fits at `x`: inside one free stretch, with its margins.
    pub fn fits(&self, x: i32) -> bool {
        self.gaps().iter().any(|g| x >= g.lo + self.margin && x + self.w <= g.hi - self.margin)
    }

    /// Where the bar goes now, given where it is (`at`): where it was put if
    /// that is free; otherwise, once moved aside, it stays put while it still
    /// fits (the icons slide back and forth as tray icons come and go), and
    /// moves to the nearest free spot only when something needs its place.
    /// The bool: there is room nowhere (it stays put and lets clicks through).
    pub fn settle(&self, want: i32, at: Option<i32>) -> (i32, bool) {
        match self.place(want) {
            Some(x) if x == want => (x, false),
            Some(x) => (at.filter(|&a| self.fits(a)).unwrap_or(x), false),
            None => (want, true),
        }
    }

    /// The dock for a bar dropped at `x`, kept from the nearer end.
    pub fn dock_at(&self, x: i32) -> Dock {
        if x + self.w / 2 < (self.left + self.tray_left) / 2 {
            Dock { anchor: Anchor::Left, px: self.logical(x - self.left) }
        } else {
            Dock { anchor: Anchor::Tray, px: self.logical(self.tray_left - (x + self.w)) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(lo: i32, hi: i32) -> Span {
        Span { lo, hi }
    }

    /// This PC's taskbar on 2026-11-06 (1920 px at 125 %), as UI Automation
    /// reported it: Widgets far left; Start, Search and 18 apps centred; the
    /// notification area from 1526.
    fn crowded() -> Layout {
        let mut occupied = vec![span(13, 68), span(408, 463), span(464, 519)];
        occupied.extend((0..17).map(|i| span(519 + i * 55, 574 + i * 55)));
        occupied.push(span(1526, 1920));
        Layout { left: 0, right: 1920, tray_left: 1526, occupied, w: 110, margin: 5, scale: 1.25 }
    }

    #[test]
    fn beside_the_clock_when_there_is_room() {
        let mut l = crowded();
        l.occupied.retain(|s| s.hi <= 1000 || s.lo >= 1526);
        let want = l.preferred_x(None);
        assert_eq!(want, 1526 - 5 - 110, "4 logical px from the notification area");
        assert_eq!(l.place(want), Some(want));
    }

    #[test]
    fn app_icons_under_it_send_it_to_the_nearest_free_stretch() {
        let l = crowded();
        let want = l.preferred_x(None);
        // (The 1 px seam between Start and Search is a stretch too: nothing fits it.)
        assert_eq!(l.gaps(), vec![span(0, 13), span(68, 408), span(463, 464), span(1454, 1526)]);
        // 72 px between the last app and the tray is too narrow for 110 + margins:
        // the nearest room is just left of Start, clear of Widgets.
        assert_eq!(l.place(want), Some(408 - 5 - 110));
        // Dropped left of Start, it stays where it was put.
        assert_eq!(l.place(200), Some(200));
        // Dropped on Widgets, it moves just clear of it.
        assert_eq!(l.place(20), Some(68 + 5));
    }

    #[test]
    fn moved_aside_it_stays_put_while_it_fits_and_comes_back_when_it_can() {
        let l = crowded();
        let want = l.preferred_x(None);
        assert!(l.fits(293) && !l.fits(294) && l.fits(73) && !l.fits(72) && !l.fits(want), "inside a free stretch, margins kept");
        assert_eq!(l.settle(want, None), (293, false), "first time: the nearest free spot");
        assert_eq!(l.settle(want, Some(200)), (200, false), "still free where it is: it stays");
        // A tray icon appears: the crowded icons slide 40 px left, Start onto the bar.
        let slid = Layout { occupied: l.occupied.iter().map(|s| if s.lo >= 1526 { span(1486, 1920) } else { span(s.lo - 40, s.hi - 40) }).collect(), tray_left: 1486, ..crowded() };
        assert_eq!(slid.settle(slid.preferred_x(None), Some(293)), (253, false), "Start needs its place: it moves");
        // The icon goes again: the bar doesn't chase Start back.
        assert_eq!(l.settle(want, Some(253)), (253, false));
        // Room beside the clock again: it goes back there.
        let roomy = Layout { occupied: vec![span(408, 1000), span(1526, 1920)], ..crowded() };
        assert_eq!(roomy.settle(roomy.preferred_x(None), Some(253)), (want, false));
        // Room nowhere: where it was put, letting clicks through.
        let full = Layout { occupied: vec![span(0, 1920)], ..crowded() };
        assert_eq!(full.settle(want, Some(253)), (want, true));
    }

    #[test]
    fn nowhere_to_go_is_said() {
        let l = Layout { occupied: vec![span(0, 1920)], ..crowded() };
        assert_eq!(l.place(500), None);
        assert!(l.gaps().is_empty());
    }

    #[test]
    fn overlapping_and_outside_buttons_merge_into_one_stretch() {
        let l = Layout { left: 100, right: 1000, tray_left: 900, occupied: vec![span(300, 400), span(350, 500), span(50, 120), span(900, 1200)], w: 50, margin: 0, scale: 1.0 };
        assert_eq!(l.gaps(), vec![span(120, 300), span(500, 900)]);
    }

    #[test]
    fn a_drop_is_kept_from_the_nearer_end_and_comes_back_where_it_was() {
        let l = crowded();
        let d = l.dock_at(200);
        assert_eq!(d, Dock { anchor: Anchor::Left, px: 160 });
        assert_eq!(l.preferred_x(Some(d)), 200);
        let d = l.dock_at(1300);
        assert_eq!(d.anchor, Anchor::Tray);
        assert!((l.preferred_x(Some(d)) - 1300).abs() <= 1, "within rounding");
        // A tray that grows moves a tray-anchored bar with it.
        let wider = Layout { tray_left: 1400, ..crowded() };
        assert_eq!(wider.preferred_x(Some(d)), l.preferred_x(Some(d)) - 126);
        // Dropped over the tray: right beside it.
        assert_eq!(l.dock_at(1600), Dock { anchor: Anchor::Tray, px: 0 });
    }

    #[test]
    fn the_preferred_spot_stays_on_the_taskbar() {
        let l = crowded();
        assert_eq!(l.preferred_x(Some(Dock { anchor: Anchor::Left, px: 99_999 })), 1920 - 110);
        assert_eq!(l.preferred_x(Some(Dock { anchor: Anchor::Tray, px: 99_999 })), 0);
        assert_eq!(l.preferred_x(Some(Dock { anchor: Anchor::Left, px: -50 })), 0);
    }

    #[test]
    fn a_dock_is_saved_as_plain_words() {
        let j = serde_json::to_string(&BESIDE_THE_CLOCK).unwrap();
        assert_eq!(j, r#"{"anchor":"tray","px":4}"#);
        let back: Dock = serde_json::from_str(r#"{"anchor":"left","px":160}"#).unwrap();
        assert_eq!(back, Dock { anchor: Anchor::Left, px: 160 });
    }
}
