//! 吉祥物贴在岛外时的落点。岛上槽位不经过这里。
use crate::app::{BAR_H, WIN_W};
use crate::model::{Dock, PetPlace};
use eframe::egui::{Pos2, Rect, Vec2};

pub(crate) const SPRITE_W: f32 = 48.0;
pub(crate) const SPRITE_H: f32 = 30.0;
const OVERLAP: f32 = 6.0;
const FLIP_ROOM: f32 = 48.0;
const RELEASE_ROOM: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End { Head, Tail }

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PetSpot {
    pub center: Pos2,
    /// 因屏幕边缘翻到另一端后锁住。面板挡住所造成的临时换端不写入这里。
    pub flipped: bool,
}

pub(crate) fn sprite_rect(center: Pos2) -> Rect {
    Rect::from_center_size(center, Vec2::new(SPRITE_W, SPRITE_H))
}

/// 最终落在岛的左端时，窗口向左加的边距。右端和左右贴边为 0。
pub(crate) fn gutter(dock: Dock, center: Pos2, island: Rect, island_width: f32) -> f32 {
    if dock != Dock::Top || center.x >= island.center().x { return 0.0; }
    (42.0 - (WIN_W - island_width) / 2.0).clamp(0.0, 56.0)
}

pub(crate) fn resolve(dock: Dock, place: PetPlace, island: Rect, panel: Rect, panel_open: bool, work: Rect, notice: Option<Rect>, flipped: bool) -> PetSpot {
    let preferred = match place {
        PetPlace::Head => End::Head,
        PetPlace::Tail => End::Tail,
        PetPlace::Island | PetPlace::Auto => auto_end(dock, island, work, notice),
    };
    let (end, latch) = pick(preferred, dock, island, work, notice, flipped);
    let end = if panel_open { avoid_panel(dock, end, island, panel) } else { end };
    PetSpot { center: center_at(dock, end, island), flipped: latch }
}

fn avoid_panel(dock: Dock, end: End, island: Rect, panel: Rect) -> End {
    let blocked = |end: End| sprite_rect(center_at(dock, end, island)).intersects(panel.expand(2.0));
    if blocked(end) {
        let other = opposite(end);
        if !blocked(other) { other } else { end }
    } else { end }
}

fn pick(preferred: End, dock: Dock, island: Rect, work: Rect, notice: Option<Rect>, flipped: bool) -> (End, bool) {
    let preferred_room = room(dock, preferred, island, work, notice);
    let other = opposite(preferred);
    let other_room = room(dock, other, island, work, notice);
    if flipped {
        if preferred_room >= RELEASE_ROOM { (preferred, false) } else { (other, true) }
    } else if preferred_room < FLIP_ROOM && other_room >= preferred_room {
        (other, true)
    } else { (preferred, false) }
}

fn auto_end(dock: Dock, island: Rect, work: Rect, notice: Option<Rect>) -> End {
    let head = room(dock, End::Head, island, work, notice);
    let tail = room(dock, End::Tail, island, work, notice);
    if tail > head { End::Tail } else { End::Head }
}

fn room(dock: Dock, end: End, island: Rect, work: Rect, notice: Option<Rect>) -> f32 {
    match (dock, end) {
        (Dock::Top, End::Head) => island.left() - work.left(),
        (Dock::Top, End::Tail) => {
            let edge = notice.filter(|notice| notice.left() > island.center().x).map(|notice| notice.left()).unwrap_or(work.right());
            edge.min(work.right()) - island.right()
        }
        (_, End::Head) => island.top() - work.top(),
        (_, End::Tail) => work.bottom() - island.bottom(),
    }
}

fn opposite(end: End) -> End {
    match end { End::Head => End::Tail, End::Tail => End::Head }
}

fn center_at(dock: Dock, end: End, island: Rect) -> Pos2 {
    let along = SPRITE_W / 2.0 - OVERLAP;
    let across = SPRITE_H / 2.0 - OVERLAP;
    match (dock, end) {
        (Dock::Top, End::Head) => Pos2::new(island.left() - along, island.top() + BAR_H / 2.0),
        (Dock::Top, End::Tail) => Pos2::new(island.right() + along, island.top() + BAR_H / 2.0),
        (Dock::Left, End::Head) => Pos2::new(island.right() + along, island.top() + across),
        (Dock::Left, End::Tail) => Pos2::new(island.right() + along, island.bottom() - across),
        (Dock::Right, End::Head) => Pos2::new(island.left() - along, island.top() + across),
        (Dock::Right, End::Tail) => Pos2::new(island.left() - along, island.bottom() - across),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn island() -> Rect { Rect::from_min_max(Pos2::new(200.0, 0.0), Pos2::new(500.0, BAR_H)) }
    fn wide_work() -> Rect { Rect::from_min_max(Pos2::new(-400.0, -400.0), Pos2::new(1400.0, 1400.0)) }

    #[test]
    fn top_head_and_tail_sit_outside_the_bar() {
        let island = island();
        let head = resolve(Dock::Top, PetPlace::Head, island, island, false, wide_work(), None, false);
        let tail = resolve(Dock::Top, PetPlace::Tail, island, island, false, wide_work(), None, false);
        assert!((head.center.x - (island.left() - 18.0)).abs() < 0.01);
        assert!((head.center.y - BAR_H / 2.0).abs() < 0.01);
        assert!((tail.center.x - (island.right() + 18.0)).abs() < 0.01);
        assert!(!head.flipped && !tail.flipped);
    }

    #[test]
    fn side_docks_place_the_pet_on_the_inner_face() {
        let island = Rect::from_min_max(Pos2::new(0.0, 200.0), Pos2::new(36.0, 360.0));
        let work = wide_work();
        let left_head = resolve(Dock::Left, PetPlace::Head, island, island, false, work, None, false);
        let left_tail = resolve(Dock::Left, PetPlace::Tail, island, island, false, work, None, false);
        assert!((left_head.center.x - (island.right() + 18.0)).abs() < 0.01);
        assert!((left_head.center.y - (island.top() + 9.0)).abs() < 0.01);
        assert!((left_tail.center.y - (island.bottom() - 9.0)).abs() < 0.01);
        let right = Rect::from_min_max(Pos2::new(464.0, 200.0), Pos2::new(500.0, 360.0));
        let right_head = resolve(Dock::Right, PetPlace::Head, right, right, false, work, None, false);
        assert!((right_head.center.x - (right.left() - 18.0)).abs() < 0.01);
        assert!((right_head.center.y - (right.top() + 9.0)).abs() < 0.01);
    }

    #[test]
    fn flips_below_48_and_returns_only_after_72() {
        let island = island();
        let tight = Rect::from_min_max(Pos2::new(island.left() - 40.0, -100.0), Pos2::new(island.right() + 200.0, 400.0));
        let flipped = resolve(Dock::Top, PetPlace::Head, island, island, false, tight, None, false);
        assert!(flipped.flipped && flipped.center.x > island.right(), "左侧不足 48 点应翻到岛尾");
        let still = Rect::from_min_max(Pos2::new(island.left() - 60.0, -100.0), Pos2::new(island.right() + 200.0, 400.0));
        let held = resolve(Dock::Top, PetPlace::Head, island, island, false, still, None, true);
        assert!(held.flipped, "空档不足 72 点时保持翻边");
        let open = Rect::from_min_max(Pos2::new(island.left() - 80.0, -100.0), Pos2::new(island.right() + 200.0, 400.0));
        let back = resolve(Dock::Top, PetPlace::Head, island, island, false, open, None, true);
        assert!(!back.flipped && back.center.x < island.left(), "空档达到 72 点后回到岛头");
    }

    #[test]
    fn open_panel_moves_the_pet_without_latching_the_flip() {
        let island = island();
        let panel = Rect::from_min_max(Pos2::new(island.left() - 50.0, -10.0), Pos2::new(island.right() - 10.0, 160.0));
        let spot = resolve(Dock::Top, PetPlace::Head, island, panel, true, wide_work(), None, false);
        assert!(spot.center.x > island.right(), "面板挡住岛头时这一帧改到岛尾");
        assert!(!spot.flipped, "面板让位不写入翻边锁");
    }

    #[test]
    fn auto_prefers_the_roomier_end_and_keeps_clear_of_a_notice() {
        let island = island();
        let work = wide_work();
        let notice = Rect::from_min_max(Pos2::new(island.right() + 8.0, 0.0), Pos2::new(island.right() + 40.0, BAR_H));
        let spot = resolve(Dock::Top, PetPlace::Auto, island, island, false, work, Some(notice), false);
        assert!(spot.center.x < island.left(), "通知占住右端时空档更大的是岛头");
    }

    #[test]
    fn wide_island_asks_for_a_left_gutter_and_a_narrow_one_does_not() {
        let wide = Rect::from_min_max(Pos2::new(16.0, 0.0), Pos2::new(624.0, BAR_H));
        let head = resolve(Dock::Top, PetPlace::Head, wide, wide, false, wide_work(), None, false);
        assert!((gutter(Dock::Top, head.center, wide, 608.0) - 26.0).abs() < 0.01);
        let narrow = island();
        let left = resolve(Dock::Top, PetPlace::Head, narrow, narrow, false, wide_work(), None, false);
        assert_eq!(gutter(Dock::Top, left.center, narrow, 300.0), 0.0);
        let tail = resolve(Dock::Top, PetPlace::Tail, wide, wide, false, wide_work(), None, false);
        assert_eq!(gutter(Dock::Top, tail.center, wide, 608.0), 0.0);
        assert_eq!(gutter(Dock::Left, head.center, wide, 608.0), 0.0);
    }

    #[test]
    fn beside_layout_drops_the_pet_slot_width() {
        let with = crate::bar_layout::Layout::new(true, 1, true, false, false, true, true, false, false, false, false, false);
        let without = crate::bar_layout::Layout::new(true, 1, true, false, false, false, true, false, false, false, false, false);
        assert!(with.x[crate::bar_layout::PET].is_some());
        assert!(without.x[crate::bar_layout::PET].is_none(), "旁置时布局里不再留下吉祥物槽");
        assert!(with.width > without.width + 30.0, "岛上槽位要占宽度");
    }
}
