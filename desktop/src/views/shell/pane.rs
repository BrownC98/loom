use api::settings::{ClosedPanes, PaneWidths};

pub const RAIL: f32 = 40.0;
const MIN_CANVAS: f32 = 120.0;
const SIDE_MAX: f32 = 480.0;
const DOCK_MAX: f32 = 800.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Side,
    Dock,
}

impl Pane {
    pub fn initial(self) -> f32 {
        match self {
            Pane::Side => 260.0,
            Pane::Dock => 320.0,
        }
    }

    fn min(self) -> f32 {
        match self {
            Pane::Side => 180.0,
            Pane::Dock => 320.0,
        }
    }

    pub fn grow(self) -> f32 {
        match self {
            Pane::Side => 1.0,
            Pane::Dock => -1.0,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Pane::Side => "side",
            Pane::Dock => "dock",
        }
    }

    fn saved(self, widths: &PaneWidths) -> Option<u16> {
        match self {
            Pane::Side => widths.side,
            Pane::Dock => widths.dock,
        }
    }

    fn closed(self, closed: &ClosedPanes) -> bool {
        match self {
            Pane::Side => closed.side,
            Pane::Dock => closed.dock,
        }
    }
}

pub struct Widths {
    pub saved: PaneWidths,
    pub closed: ClosedPanes,
    pub window: f32,
    pub drag: Option<(Pane, f32)>,
}

impl Widths {
    pub fn open(&self, pane: Pane) -> bool {
        !pane.closed(&self.closed)
    }

    pub fn clamp(&self, pane: Pane, width: f32) -> f32 {
        let max = match pane {
            Pane::Side => SIDE_MAX,
            Pane::Dock => {
                (self.window - self.shown(Pane::Side) - MIN_CANVAS).clamp(pane.min(), DOCK_MAX)
            }
        };
        width.round().clamp(pane.min(), max)
    }

    pub fn width(&self, pane: Pane) -> f32 {
        if let Some((dragged, width)) = self.drag
            && dragged == pane
        {
            return width;
        }
        let wanted = pane.saved(&self.saved).map_or(pane.initial(), f32::from);
        self.clamp(pane, wanted)
    }

    pub fn shown(&self, pane: Pane) -> f32 {
        if self.open(pane) {
            self.width(pane)
        } else {
            RAIL
        }
    }
}
