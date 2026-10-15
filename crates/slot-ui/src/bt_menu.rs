use crate::draw::{Canvas, Draw, TexId, OUT_H};
use crate::plate::{centred_hints, HINT_H, LEGEND_GAP};
use crate::power_menu::MENU_PAD;
use crate::quick_menu::{QUICK_EDGE, QUICK_PITCH};
use crate::slot_chrome::{edge, opening};

/// The quick menu's rows, for lists slot only learns at run time: paired headphones,
/// headphones found by a scan.
pub type Face = (TexId, u32, u32);

pub const BT_ROWS_MAX: usize = 6;
const BAR_INSET: f32 = 4.0;
const TYPE_DROP: f32 = 4.0;
const STATUS_Y: f32 = 380.0;
const LEGEND_Y: f32 = 427.0;
const GAME_SCRIM: f32 = 0.65;

pub struct BtRowFaces {
    pub label: Face,
    pub value: Option<[Face; 2]>,
}

pub struct BtMenu<'a> {
    pub rows: &'a [BtRowFaces],
    pub row: usize,
    pub status: Option<Face>,
    pub legend: &'a [(TexId, u32)],
    pub canvas: Canvas,
    /// Drawn from the in-game menu: the paused game shows dimmed behind the rows.
    pub over_game: bool,
}

impl BtMenu<'_> {
    pub fn draw(&self, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: self.canvas.w,
            h: OUT_H as f32,
            colour: match self.over_game {
                true => [0.0, 0.0, 0.0, GAME_SCRIM],
                false => opening(),
            },
        });
        let n = self.rows.len().min(BT_ROWS_MAX);
        if n > 0 {
            out.push(Draw::Rect {
                x: 0.0,
                y: bt_row_top(self.row.min(n - 1), n) + BAR_INSET,
                w: self.canvas.w,
                h: QUICK_PITCH - 2.0 * BAR_INSET,
                colour: edge(),
            });
        }
        let (right, pad) = (self.canvas.w - QUICK_EDGE, MENU_PAD as f32);
        for (i, r) in self.rows.iter().take(n).enumerate() {
            let y = bt_row_top(i, n) + TYPE_DROP;
            let (tex, w, h) = r.label;
            push(out, tex, QUICK_EDGE - pad, y, w, h);
            if let Some(value) = r.value {
                let (tex, w, h) = value[(i == self.row) as usize];
                push(out, tex, right + pad - w as f32, y, w, h);
            }
        }
        if let Some((tex, w, h)) = self.status {
            push(out, tex, (self.canvas.w - w as f32) / 2.0, STATUS_Y, w, h);
        }
        for (tex, w, x) in centred_hints(self.canvas, self.legend, LEGEND_GAP) {
            push(out, tex, x, LEGEND_Y, w, HINT_H);
        }
    }
}

pub fn bt_row_top(row: usize, rows: usize) -> f32 {
    let top = (OUT_H as f32 - QUICK_PITCH * rows as f32) / 2.0;
    top + QUICK_PITCH * row as f32
}

fn push(out: &mut Vec<Draw>, tex: TexId, x: f32, y: f32, w: u32, h: u32) {
    out.push(Draw::Tex {
        x: x.round(),
        y: y.round(),
        w: w as f32,
        h: h as f32,
        tex,
        alpha: 1.0,
    });
}
