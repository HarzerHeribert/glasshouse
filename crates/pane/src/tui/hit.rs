//! Where a click lands, recorded by the renderer that drew it.
//!
//! **The mouse is a second route to an action the keyboard already has, never
//! the only one** (the user's ruling, 2026-09-17: *"Either full keyboard or
//! user can use mouse to navigate and click surfaces"*). Every variant of
//! [`Hit`] names an action that a key or a slash command already performs, and
//! `tests/tui_look.rs` holds a pair per surface: the click does it, and the
//! documented key does the same. A surface with no keyboard twin does not
//! belong here.
//!
//! **The hit map is built by the draw, not beside it.** Each recorder below is
//! called from the same code path that renders the thing, with the rectangle
//! it actually drew into, which is what keeps the map from drifting away from
//! the screen as the layout changes. `the_hit_map_comes_from_the_same_regions_
//! the_screen_draws` is the guard.

use ratatui::layout::Rect;

use super::controls::{PanelGeometry, PanelHit};

/// A status-line field a click reaches, and the input it stands for.
///
/// Only fields with a keyboard twin are here: the model opens the picker as
/// `/model` does, and the mode cycles as Shift-Tab does. `effort`, the
/// supervisor and `net:` are shown in the same line and are deliberately
/// **not** clickable — nothing changes them from the keyboard either, so a
/// click would be the only route and the invariant above forbids that.
///
/// The mouse marker is not here for a different reason: it is drawn **only
/// while capture is released**, and a released pointer cannot click anything.
/// Its routes are `/mouse` and Ctrl-G, which is why both exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusField {
    /// The served model, left of the first status row. Twin: `/model`.
    Model,
    /// The request mode. Twin: Shift-Tab.
    Mode,
}

/// What lies under a column and row of the drawn screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    /// A row of the open panel, exactly as before this module existed.
    Panel(PanelHit),
    /// A cell's own header or one of its region headers. Twin: `/cell <n>`.
    Cell(usize),
    /// A status-line field. Twin: each field's own, per [`StatusField`].
    Status(StatusField),
    /// Inside the composer, at this row and column **of the wrapped text**,
    /// with the composer's own scroll already added, so a caller turns it
    /// into a cursor offset without knowing how the box was drawn. Twin: the
    /// arrow keys, Home and End.
    Composer { row: usize, column: u16 },
}

/// Every clickable rectangle of one drawn frame.
///
/// Empty by default, which is what an un-rendered screen must hit as: a
/// geometry nobody filled answers `None` everywhere rather than guessing.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScreenGeometry {
    /// The open panel's own rows. Kept as its own type because the picker was
    /// clickable before the rest of the screen was, and its hit test is
    /// already tested where it lives.
    pub(crate) panel: PanelGeometry,
    cells: Vec<(Rect, usize)>,
    status: Vec<(Rect, StatusField)>,
    /// The composer's text area, and how many wrapped rows are scrolled off
    /// its top -- both from the draw, because a click on the first visible
    /// row of a scrolled composer is not the first row of the text.
    composer: Option<(Rect, usize)>,
}

impl ScreenGeometry {
    /// One row of a cell block that reaches that cell when clicked.
    pub(crate) fn record_cell(&mut self, area: Rect, cell: usize) {
        self.cells.push((area, cell));
    }

    /// A status field's own span, not the whole row: the row also carries
    /// figures that are not controls.
    pub(crate) fn record_status(&mut self, area: Rect, field: StatusField) {
        self.status.push((area, field));
    }

    /// The composer's text area, inside its border, and the rows scrolled
    /// off its top.
    pub(crate) fn record_composer(&mut self, area: Rect, skip: usize) {
        self.composer = Some((area, skip));
    }

    /// What a click at `column`, `row` reaches.
    ///
    /// **The panel is asked first because it is drawn over the transcript**:
    /// while it is open its rows occupy rectangles a cell also claims, and the
    /// thing on top is the thing a person meant to click. Everything else is
    /// disjoint by layout, so the remaining order is only for determinism.
    pub(crate) fn hit(&self, column: u16, row: u16) -> Option<Hit> {
        if let Some(panel) = self.panel.hit(column, row) {
            return Some(Hit::Panel(panel));
        }
        if let Some((_, field)) = self
            .status
            .iter()
            .find(|(area, _)| contains(*area, column, row))
        {
            return Some(Hit::Status(*field));
        }
        if let Some((area, skip)) = self
            .composer
            .filter(|(area, _)| contains(*area, column, row))
        {
            return Some(Hit::Composer {
                row: usize::from(row - area.y) + skip,
                column: column - area.x,
            });
        }
        self.cells
            .iter()
            .find(|(area, _)| contains(*area, column, row))
            .map(|(_, cell)| Hit::Cell(*cell))
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    area.width > 0
        && area.height > 0
        && column >= area.x
        && column < area.right()
        && row >= area.y
        && row < area.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> ScreenGeometry {
        let mut geometry = ScreenGeometry::default();
        geometry.record_cell(Rect::new(0, 4, 40, 1), 7);
        geometry.record_status(Rect::new(1, 20, 12, 1), StatusField::Model);
        geometry.record_status(Rect::new(60, 20, 7, 1), StatusField::Mode);
        geometry.record_composer(Rect::new(1, 15, 40, 3), 0);
        geometry
    }

    #[test]
    fn an_empty_geometry_hits_nothing() {
        assert_eq!(ScreenGeometry::default().hit(3, 3), None);
    }

    #[test]
    fn a_click_on_dead_space_is_not_a_hit() {
        assert_eq!(geometry().hit(39, 9), None);
    }

    #[test]
    fn each_surface_answers_for_its_own_rectangle() {
        let geometry = geometry();
        assert_eq!(geometry.hit(10, 4), Some(Hit::Cell(7)));
        assert_eq!(geometry.hit(2, 20), Some(Hit::Status(StatusField::Model)));
        assert_eq!(geometry.hit(61, 20), Some(Hit::Status(StatusField::Mode)));
        assert_eq!(
            geometry.hit(5, 16),
            Some(Hit::Composer { row: 1, column: 4 })
        );
    }

    #[test]
    fn a_composer_hit_is_relative_to_its_own_area() {
        let mut geometry = ScreenGeometry::default();
        geometry.record_composer(Rect::new(10, 30, 20, 2), 3);
        assert_eq!(
            geometry.hit(10, 30),
            // The third row is scrolled off the top, so the first visible row
            // is the fourth row of the text.
            Some(Hit::Composer { row: 3, column: 0 })
        );
        assert_eq!(geometry.hit(30, 30), None);
    }
}
